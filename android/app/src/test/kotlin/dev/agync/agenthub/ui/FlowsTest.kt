package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.printToString
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.click
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performImeAction
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.data.AppContainer
import dev.agync.agenthub.data.HubClientFactory
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.Refusal
import dev.agync.agenthub.data.SeededHub
import dev.agync.agenthub.model.AskStatus
import dev.agync.agenthub.nav.Navigator
import dev.agync.agenthub.nav.Route
import dev.agync.agenthub.pairing.SeededPairing
import java.util.Locale
import java.util.TimeZone
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

private class App(val clock: Fixtures.MutableClock = Fixtures.clock(), val hub: SeededHub = SeededHub(clock)) {
  val platform = FakePlatform()
  val navigator = Navigator()
  val container = AppContainer(clock, MemoryStore(), SeededPairing(clock, 0, 0), HubClientFactory { hub }, Fixtures.fmt(), "pixel-phone")

  fun paired() = apply { container.registry.add(Fixtures.hub) }

  val repo get() = container.repository(Fixtures.hub)
}

/** The main flows, driven through the real screens and the real view models against the seeded hub. */
@OptIn(ExperimentalTestApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class FlowsTest {
  private val dispatcher = MainDispatcherRule()
  private val compose = createAndroidComposeRule<ComponentActivity>()

  @get:Rule val chain: RuleChain = RuleChain.outerRule(dispatcher).around(compose)

  @Before fun locale() {
    Locale.setDefault(Locale.UK)
    TimeZone.setDefault(TimeZone.getTimeZone("UTC"))
  }

  private fun show(app: App) = compose.setContent { AppRoot(app.container, app.platform, app.navigator) }

  private fun waitFor(text: String) =
    try {
      compose.waitUntil(5_000) { compose.onAllNodes(hasText(text, substring = true)).fetchSemanticsNodes().isNotEmpty() }
    } catch (e: Throwable) {
      throw AssertionError("no node with '$text'. The screen was:\n" + compose.onRoot().printToString(), e)
    }

  private fun waitForDesc(text: String) =
    try {
      compose.waitUntil(5_000) { compose.onAllNodes(hasContentDescription(text)).fetchSemanticsNodes().isNotEmpty() }
    } catch (e: Throwable) {
      throw AssertionError("no node described '$text'. The screen was:\n" + compose.onRoot().printToString(), e)
    }

  @Test fun `pairing by typing the code goes from welcome to Now`() {
    val app = App()
    show(app)
    compose.onNodeWithText("Connect this phone to your hub").assertExists()
    compose.onNodeWithText("Type the code").performClick()
    compose.onNodeWithContentDescription("Code").performTextInput("TND-4F9K-22XQ-8N3P")
    compose.onNodeWithContentDescription("Hub address").performTextInput("hub.example.ts.net")
    compose.onNodeWithText("Continue").performClick()
    compose.onNodeWithText("Pair with home hub?").assertExists()
    compose.onNodeWithText("Pair", useUnmergedTree = false).performClick()
    waitFor("Compare the check string")
    compose.onNodeWithText("They match").performClick()
    waitFor("How should alerts reach you?")
    compose.onNodeWithText("Not now").performClick()
    waitForDesc("Now, 4 need you")
    assertEquals(1, app.container.registry.state.value.hubs.size)
    assertEquals(false, app.container.prefs.state.value.stayConnected)
  }

  @Test fun `the check string is shown while the hub has yet to confirm`() {
    val app = App()
    val pairing = SeededPairing(app.clock, 0, 60_000)
    val container = AppContainer(app.clock, MemoryStore(), pairing, HubClientFactory { app.hub }, Fixtures.fmt(), "pixel-phone")
    compose.setContent { AppRoot(container, app.platform, app.navigator) }
    compose.onNodeWithText("Type the code").performClick()
    compose.onNodeWithContentDescription("Code").performTextInput("TND-4F9K-22XQ-8N3P")
    compose.onNodeWithContentDescription("Hub address").performTextInput("hub.example.ts.net")
    compose.onNodeWithText("Continue").performClick()
    compose.onNodeWithText("Pair", useUnmergedTree = false).performClick()
    waitFor("Compare the check string")
    compose.onNodeWithText("k7m2 qwxa 4fbn r3td").assertExists()
    compose.onNodeWithText("They match").performClick()
    waitFor("Waiting for you to confirm on the hub")
    assertTrue(container.registry.state.value.hubs.isEmpty())
  }

  @Test fun `answering a question from its row clears it and drops the badge`() {
    val app = App().paired()
    show(app)
    waitForDesc("Now, 4 need you")
    compose.onNodeWithContentDescription("Answer nightly: Yes, tonight").performClick()
    waitForDesc("Now, 3 need you")
    assertEquals(AskStatus.Answered, runBlocking { app.hub.ask("9MX4TB") }.status)
  }

  @Test fun `approving from the item screen decides it and opens the next ask`() {
    val app = App().paired()
    app.navigator.push(Route.Item("7HQ2KD"))
    show(app)
    waitFor("WHAT APPROVING DOES")
    compose.onNodeWithText("Applies 0042_invoice_totals to billing_staging.", substring = true).assertExists()
    compose.onNodeWithContentDescription("Approve feat-invoices: Run the migration on staging").performClick()
    compose.waitUntil(5_000) { runBlocking { app.hub.ask("7HQ2KD") }.status == AskStatus.Approved }
    compose.waitUntil(5_000) { app.navigator.current is Route.Item && (app.navigator.current as Route.Item).askId != "7HQ2KD" }
  }

  @Test fun `with the fingerprint opt-in on, approving asks for it first and does nothing without it`() {
    val app = App().paired()
    app.platform.fingerprintOk = false
    app.container.prefs.update { it.copy(fingerprintBeforeApproving = true) }
    app.navigator.push(Route.Item("7HQ2KD"))
    show(app)
    waitFor("WHAT APPROVING DOES")
    compose.onNodeWithContentDescription("Approve feat-invoices: Run the migration on staging").performClick()
    compose.waitUntil(5_000) { app.platform.fingerprintAsked.isNotEmpty() }
    compose.waitForIdle()
    assertEquals(AskStatus.Waiting, runBlocking { app.hub.ask("7HQ2KD") }.status)
    app.platform.fingerprintOk = true
    compose.onNodeWithContentDescription("Approve feat-invoices: Run the migration on staging").performClick()
    compose.waitUntil(5_000) { runBlocking { app.hub.ask("7HQ2KD") }.status == AskStatus.Approved }
  }

  @Test fun `an answer the hub refuses shows what was decided and keeps my text to copy`() {
    val app = App().paired()
    runBlocking {
      app.repo.refresh()
      app.hub.reachable = false
      app.repo.answer(app.repo.state.value.snapshot!!.asks.first { it.id == "9MX4TB" }, "Yes, tonight. Start after 22:00.")
      app.hub.reachable = true
      app.hub.refuseNext = Refusal.AnsweredElsewhere(Fixtures.now, "Wait for the weekend")
      app.repo.refresh()
    }
    app.navigator.push(Route.Item("9MX4TB"))
    show(app)
    waitFor("Your answer was not sent.")
    compose.onNodeWithText("Wait for the weekend").assertExists()
    compose.onNodeWithText("Yes, tonight. Start after 22:00.").assertExists()
    compose.onNodeWithContentDescription("Copy your answer").performClick()
    assertEquals("Yes, tonight. Start after 22:00.", app.platform.copied.single().second)
    compose.onNodeWithText("Discard my answer").performClick()
    compose.waitUntil(5_000) { app.repo.state.value.outbox.isEmpty() }
  }

  @Test fun `a revoked phone sees it was unpaired, not a connection error`() {
    val app = App().paired()
    app.hub.revoked = true
    show(app)
    waitFor("This phone was unpaired")
    compose.onNodeWithText("Pair again").assertExists()
    compose.onNodeWithText("Cannot reach", substring = true).assertDoesNotExist()
  }

  @Test fun `an unreachable hub with nothing cached offers a fresh scan and says how it was last reached`() {
    val app = App().paired()
    app.hub.reachable = false
    show(app)
    waitFor("Cannot reach home hub")
    compose.onNodeWithText("Scan a fresh code").assertExists()
    compose.onNodeWithText("Try again").assertExists()
  }

  @Test fun `every control has a name and every target is at least 48 by 48`() {
    val app = App().paired()
    show(app)
    waitForDesc("Now, 4 need you")
    compose.assertEveryControlIsNamedAndBigEnough()
    app.navigator.push(Route.Item("7HQ2KD"))
    waitFor("WHAT APPROVING DOES")
    compose.assertEveryControlIsNamedAndBigEnough()
  }

  @Test fun `controls the hub refuses a phone are not shown`() {
    val app = App().paired()
    app.navigator.selectTab(Route.Hub)
    show(app)
    waitFor("MEMBERS")
    compose.onNodeWithText("Add").assertDoesNotExist()
    app.navigator.push(Route.Member("laptop"))
    waitFor("AGENTS SEEN IN ITS SESSIONS")
    compose.onNodeWithText("Reissue token", substring = true).assertDoesNotExist()
    compose.onNodeWithText("Revoke token", substring = true).assertDoesNotExist()
    app.navigator.selectTab(Route.Projects)
    waitFor("home-lab")
    compose.onNodeWithContentDescription("New project").assertDoesNotExist()
  }

  /** On the phone a tap beside the typed text did nothing: the field was only as wide as its text. */
  @Test fun `a tap anywhere in a text field focuses it`() {
    val app = App()
    show(app)
    compose.onNodeWithText("Type the code").performClick()
    val code = compose.onNodeWithContentDescription("Code")
    val width = code.fetchSemanticsNode().size.width
    assertTrue("the field spans its box, not just its text ($width px)", width > 600)
    code.performTouchInput { click(androidx.compose.ui.geometry.Offset(width - 20f, centerY)) }
    code.assertIsFocused()
  }

  @Test fun `an idea is resolved from its row and leaves Needs you`() {
    val app = App().paired()
    runBlocking { app.hub.captureIdea("home-lab", "Try Longhorn for the media volume", null) }
    show(app)
    waitForDesc("Now, 5 need you")
    compose.onNodeWithContentDescription("Resolve the idea: Try Longhorn for the media volume").performClick()
    waitForDesc("Now, 4 need you")
    val id = runBlocking { app.hub.now() }.asks.first { it.kind == dev.agync.agenthub.model.AskKind.Idea }.id
    assertEquals(AskStatus.Resolved, runBlocking { app.hub.ask(id) }.status)
  }

  @Test fun `an idea opens with Clear, Resolve and a note field, and a note leaves it open`() {
    val app = App().paired()
    runBlocking { app.hub.captureIdea("home-lab", "Try Longhorn for the media volume", null) }
    app.navigator.push(Route.Item(runBlocking { app.hub.now() }.asks.first { it.kind == dev.agync.agenthub.model.AskKind.Idea }.id))
    show(app)
    waitForDesc("Resolve the idea: Try Longhorn for the media volume")
    compose.onNodeWithContentDescription("Clear the idea: Try Longhorn for the media volume").assertExists()
    compose.onNodeWithContentDescription("Note on the idea").performTextInput("Check the disks first")
    compose.onNodeWithContentDescription("Add the note to the idea").performClick()
    compose.waitUntil(5_000) { app.hub.ideaNotes.isNotEmpty() }
    assertEquals("Check the disks first", app.hub.ideaNotes.single().second)
    compose.onNodeWithContentDescription("Resolve the idea: Try Longhorn for the media volume").assertExists()
  }

  @Test fun `pruning offers an undo that puts the sessions back`() {
    val app = App().paired()
    app.navigator.push(Route.Storage)
    show(app)
    waitFor("Prune all")
    val before = runBlocking { app.hub.storage() }.ended.size
    assertTrue(before > 0)
    compose.onNodeWithText("Prune all").performClick()
    compose.onNodeWithText("Prune", useUnmergedTree = false).performClick()
    waitFor("Pruned $before ended sessions")
    assertEquals(0, runBlocking { app.hub.storage() }.ended.size)
    compose.onNodeWithText("Undo").performClick()
    compose.waitUntil(5_000) { runBlocking { app.hub.storage() }.ended.size == before }
    waitFor("Prune all")
    compose.onNodeWithText("invoice-export").assertExists()
  }

  @Test fun `an ask's brain receipt opens that entry, and back returns to the ask`() {
    val app = App().paired()
    app.navigator.push(Route.Item("9MX4TB"))
    show(app)
    waitFor("kv/upgrade-plan")
    compose.onNodeWithText("kv/upgrade-plan", substring = true).performClick()
    waitFor("rollback: the 1.30 snapshot taken at 22:00")
    compose.onNodeWithText("/kv/upgrade-plan", substring = true).assertExists()
    compose.onNodeWithContentDescription("Back").performClick()
    waitFor("kv/upgrade-plan")
    assertEquals(Route.Item("9MX4TB"), app.navigator.current)
  }

  @Test fun `an ended session is pruned from its own screen with an undo, not ended again`() {
    val app = App().paired()
    app.navigator.push(Route.Session("01JDM1IE"))
    show(app)
    waitFor("Prune this session")
    compose.onNodeWithText("Prune this session", substring = true).performClick()
    waitFor("Prune invoice-export?")
    compose.onNodeWithText("Prune", useUnmergedTree = false).performClick()
    waitFor("Pruned invoice-export")
    assertTrue(runBlocking { app.hub.sessions("billing-svc") }.none { it.id == "01JDM1IE" })
    compose.onNodeWithText("Undo").performClick()
    compose.waitUntil(5_000) { runBlocking { app.hub.sessions("billing-svc") }.any { it.id == "01JDM1IE" } }
  }

  @Test fun `search opens from Now, finds an artifact on the keyboard's search key and opens it`() {
    val app = App().paired()
    show(app)
    waitForDesc("Now, 4 need you")
    compose.onNodeWithContentDescription("Search").performClick()
    waitFor("Search every feed")
    compose.onNodeWithContentDescription("Search", useUnmergedTree = false).assertExists()
    compose.onNodeWithContentDescription("Search").performTextInput("backup")
    compose.onNodeWithContentDescription("Search").performImeAction()
    waitFor("ARTIFACTS")
    compose.onNodeWithText("Backup report").performClick()
    compose.waitUntil(5_000) { app.navigator.current == Route.Viewer("ar2") }
  }

  @Test fun `a search with no hits says so in words`() {
    val app = App().paired()
    app.navigator.push(Route.Search)
    show(app)
    waitFor("Search every feed")
    compose.onNodeWithContentDescription("Search").performTextInput("zzzz")
    compose.onNodeWithContentDescription("Run the search").performClick()
    waitFor("No results for")
  }

  @Test fun `the keyboard's send key sends a typed reply`() {
    val app = App().paired()
    app.navigator.push(Route.Item("9MX4TB"))
    show(app)
    waitForDesc("Reply to nightly")
    compose.onNodeWithContentDescription("Reply to nightly").performTextInput("Tonight at 22:00")
    compose.onNodeWithContentDescription("Reply to nightly").performImeAction()
    compose.waitUntil(5_000) { runBlocking { app.hub.ask("9MX4TB") }.status == AskStatus.Answered }
    assertEquals("answered nightly: Tonight at 22:00", runBlocking { app.hub.ask("9MX4TB") }.decidedWith)
  }

  @Test fun `a joining machine asks for projects and each is a Grant checkbox, with no name to change`() {
    val app = App().paired()
    app.navigator.push(Route.Item("JN3STU"))
    show(app)
    waitFor("Grant space-a")
    compose.onNodeWithText("Grant docs-site").assertExists()
    compose.onNodeWithContentDescription("Name").assertDoesNotExist()
    compose.onNodeWithText("Grant space-a").performClick()
    compose.onNodeWithText("Approve").performClick()
    compose.waitUntil(5_000) { runBlocking { app.hub.ask("JN3STU") }.status == AskStatus.Admitted }
    assertEquals(listOf("space-a"), runBlocking { app.hub.members() }.first { it.name == "studio-mac" }.grants.map { it.project })
  }

  @Test fun `an approval the hub reserves for the web app says so on the item`() {
    val app = App().paired()
    app.hub.refuseAdmitFromPhone = true
    app.navigator.push(Route.Item("JN3STU"))
    show(app)
    waitFor("Grant space-a")
    compose.onNodeWithText("Approve").performClick()
    waitFor("Approve this one from the web app")
    assertEquals(AskStatus.Waiting, runBlocking { app.hub.ask("JN3STU") }.status)
  }

  @Test fun `Open web app is listed only when the hub reported its address, and opens it`() {
    val app = App().paired()
    app.navigator.selectTab(Route.Hub)
    show(app)
    waitFor("MEMBERS")
    compose.onNodeWithText("Open web app").assertDoesNotExist()
  }

  @Test fun `with an address from the hub, Open web app sends the browser there`() {
    val app = App().paired()
    app.hub.webUrl = "https://hub.example:7318/"
    app.navigator.selectTab(Route.Hub)
    show(app)
    waitFor("Open web app")
    compose.onNodeWithText("Open web app").performClick()
    assertEquals(listOf("https://hub.example:7318/"), app.platform.openedWeb)
  }

  @Test fun `the typed-code screen asks for three groups and no longer mentions a two-group code`() {
    val app = App()
    show(app)
    compose.onNodeWithText("Type the code").performClick()
    compose.onNodeWithText("TND-XXXX-XXXX-XXXX").assertExists()
    compose.onNodeWithText("two-group", substring = true).assertDoesNotExist()
  }

  @Test fun `Continue stays off until the typed code has three groups`() {
    val app = App()
    show(app)
    compose.onNodeWithText("Type the code").performClick()
    compose.onNodeWithContentDescription("Code").performTextInput("TND-4F9K-22XQ")
    compose.onNodeWithContentDescription("Hub address").performTextInput("hub.example.ts.net")
    compose.onNodeWithText("Continue").performClick()
    compose.onNodeWithText("Pair with home hub?").assertDoesNotExist()
  }

  @Test fun `decrypting a protected artifact does its key stretching off the main thread`() {
    val app = App().paired()
    app.navigator.push(Route.Viewer("ar4"))
    show(app)
    waitForDesc("Password")
    compose.onNodeWithContentDescription("Password").performTextInput("not it")
    compose.onNodeWithText("Decrypt").performClick()
    compose.waitUntil(5_000) { app.platform.openedOnMain != null }
    assertEquals(false, app.platform.openedOnMain)
  }

  @Test fun `the event id is copied from a glyph on its row, not from a link`() {
    val app = App().paired()
    app.navigator.push(Route.Item("9MX4TB"))
    show(app)
    waitForDesc("Copy the event id")
    compose.onNodeWithContentDescription("Copy the event id").performClick()
    assertEquals("9MX4TB", app.platform.copied.last().second)
  }

  @Test fun `a hub that unpaired this phone can be forgotten, which removes it and what the phone kept`() {
    val app = App().paired()
    app.hub.revoked = true
    show(app)
    waitFor("This phone was unpaired")
    compose.onNodeWithText("Forget this hub").performClick()
    waitFor("Connect this phone to your hub")
    assertTrue(app.container.registry.state.value.hubs.isEmpty())
  }
}
