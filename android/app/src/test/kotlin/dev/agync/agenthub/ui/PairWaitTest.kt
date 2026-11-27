package dev.agync.agenthub.ui

import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.model.PairWait
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.ui.screens.PairActions
import dev.agync.agenthub.ui.screens.PairScreen
import dev.agync.agenthub.ui.screens.animatorDurationScale
import dev.agync.agenthub.ui.screens.pulseAllowed
import dev.agync.agenthub.viewmodel.PairStep
import dev.agync.agenthub.viewmodel.StopReason
import java.time.Duration
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** The waiting screen and the screens either side of it: what it says, what it announces, what it lets the person do. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class PairWaitTest {
  private val compose = createAndroidComposeRule<ComponentActivity>()

  @get:Rule val chain: RuleChain = RuleChain.outerRule(MainDispatcherRule()).around(compose)

  private val hs = Handshake("h", "home hub", "k7m2 qwxa 4fbn r3td", "LAN")
  private val clock = Fixtures.clock()
  private var continued = 0
  private var startedAgain = 0

  private val actions = PairActions({}, {}, {}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, { continued++ }, { startedAgain++ })

  private fun show(step: PairStep) {
    compose.setContent { PhoneFrame(dark = false) { PairScreen(step, DevicePrefs(), Fixtures.fmt(), actions, clock = clock) {} } }
    compose.waitForIdle()
  }

  private fun waiting(seconds: Long, checkedAgo: Long? = null): PairStep.CheckString {
    val since = Fixtures.now.minusSeconds(seconds)
    return PairStep.CheckString(hs, wait = PairWait(since, since.plus(Duration.ofMinutes(15)), checkedAgo?.let { Fixtures.now.minusSeconds(it) }))
  }

  private val polite = SemanticsMatcher.expectValue(SemanticsProperties.LiveRegion, LiveRegionMode.Polite)

  @Test fun `the first frame keeps the steps, the check string and the new copy`() {
    show(waiting(42))
    compose.onNodeWithText("Confirm on the hub").assertExists()
    compose.onNodeWithText("Waiting for you to confirm on the hub").assertExists()
    compose.onNodeWithText("waiting 0:42").assertExists()
    compose.onNodeWithText("k7m2 qwxa 4fbn r3td").assertExists()
    compose.onNodeWithText(
      "On the hub, Settings, Phones shows the same check string. Confirm there if they match. This usually takes under a minute, and this screen moves on by itself.",
    ).assertExists()
    compose.onNodeWithText("If they differ, refuse on the hub and cancel here. Something stands between this phone and the hub.").assertExists()
    compose.onNodeWithText("Cancel pairing").assertExists()
    compose.onNodeWithText("Still waiting.", substring = true).assertDoesNotExist()
    compose.assertEveryControlIsNamedAndBigEnough()
  }

  @Test fun `after two minutes the line says when the hub was last asked, and the note gives the deadline`() {
    show(waiting(125, checkedAgo = 2))
    compose.onNodeWithText("waiting 2:05 · checked just now").assertExists()
    compose.onNodeWithText("Still waiting. If Add a phone on the hub shows no confirm button, reload that page.").assertExists()
    compose.onNodeWithText("The pairing stays open until 14:44.").assertExists()
    compose.assertEveryControlIsNamedAndBigEnough()
  }

  @Test fun `an older check is named in seconds`() {
    show(waiting(300, checkedAgo = 7))
    compose.onNodeWithText("waiting 5:00 · checked 7s ago").assertExists()
  }

  @Test fun `the status is a polite live region that names the state, and the ticking line is not one`() {
    show(waiting(42))
    compose.onNodeWithContentDescription("Waiting for you to confirm on the hub").assert(polite)
    compose.onNodeWithText("waiting 0:42").assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.LiveRegion))
  }

  @Test fun `the announcement changes when the state changes and not with the seconds`() {
    show(waiting(125, checkedAgo = 2))
    compose.onNodeWithContentDescription("Still waiting for you to confirm on the hub").assert(polite)
    compose.onNodeWithContentDescription("Waiting for you to confirm on the hub").assertDoesNotExist()
  }

  @Test fun `the elapsed time counts up with the clock`() {
    show(waiting(42))
    clock.at = clock.at.plusSeconds(3)
    compose.mainClock.advanceTimeBy(3_000)
    compose.waitForIdle()
    compose.onNodeWithText("waiting 0:45").assertExists()
    compose.onNodeWithContentDescription("Waiting for you to confirm on the hub").assertExists()
  }

  @Test fun `a typed code that still needs the person keeps its own copy and buttons`() {
    show(PairStep.CheckString(hs, needsYou = true))
    compose.onNodeWithText("They match").assertExists()
    compose.onNodeWithText("They differ").assertExists()
    compose.onNodeWithText("waiting", substring = true).assertDoesNotExist()
  }

  @Test fun `confirmed shows all three steps done and continues on request`() {
    show(PairStep.Confirmed(hs))
    compose.onNodeWithText("Confirmed on the hub").assertExists()
    compose.onNodeWithText("Paired. Next, choose how alerts reach you.").assertExists()
    compose.onNodeWithText("Waiting for you to confirm on the hub").assertDoesNotExist()
    compose.assertEveryControlIsNamedAndBigEnough()
    compose.onNodeWithText("Continue").performClick()
    assertEquals(1, continued)
  }

  @Test fun `a refusal says so, explains the differing strings and offers a fresh start`() {
    show(PairStep.Stopped(StopReason.Refused))
    compose.onNodeWithText("Pairing stopped").assertExists()
    compose.onNodeWithText("The hub refused this phone.").assertExists()
    compose.onNodeWithText("If you did not refuse it, the check strings differed: something may stand between this phone and the hub. Start again from a new code.").assertExists()
    compose.assertEveryControlIsNamedAndBigEnough()
    compose.onNodeWithText("Start again").performClick()
    assertEquals(1, startedAgain)
  }

  @Test fun `running out names the time and offers a fresh start`() {
    show(PairStep.Stopped(StopReason.RanOut(Fixtures.now.plus(Duration.ofMinutes(15)))))
    compose.onNodeWithText("Pairing stopped").assertExists()
    compose.onNodeWithText("The pairing ran out at 14:47 before it was confirmed on the hub.").assertExists()
    compose.onNodeWithText("The hub refused this phone.").assertDoesNotExist()
    compose.onNodeWithText("Start again").performClick()
    assertEquals(1, startedAgain)
  }

  @Test fun `the ring pulses only while animations are on`() {
    assertTrue(pulseAllowed(1f))
    assertTrue(pulseAllowed(0.5f))
    assertFalse(pulseAllowed(0f))
    val context = RuntimeEnvironment.getApplication()
    Settings.Global.putFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
    assertEquals(0f, animatorDurationScale(context), 0f)
    Settings.Global.putFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
    assertEquals(1f, animatorDurationScale(context), 0f)
  }
}
