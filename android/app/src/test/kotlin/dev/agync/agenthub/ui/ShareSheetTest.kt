package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import dev.agync.agenthub.share.ShareSheet
import dev.agync.agenthub.ui.theme.HubTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class ShareSheetTest {
  @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

  private val five = listOf("alpha", "beta", "gamma", "delta", "epsilon")

  @Test fun `every project can be chosen, not only the first three`() {
    var captured: String? = null
    compose.setContent { HubTheme { ShareSheet("home hub", "https://example.org", "A page", { five }, { p, _ -> captured = p; true }, {}, {}) } }
    compose.waitUntil(5_000) { compose.onAllNodesWithText5() }
    compose.onNodeWithText("epsilon").performClick()
    compose.onNodeWithText("Capture").performClick()
    compose.waitUntil(5_000) { captured != null }
    assertEquals("epsilon", captured)
  }

  @Test fun `a capture the hub did not take says so and keeps the sheet open`() {
    var done = false
    compose.setContent { HubTheme { ShareSheet("home hub", "https://example.org", "A page", { five }, { _, _ -> false }, { done = true }, {}) } }
    compose.waitUntil(5_000) { compose.onAllNodesWithText5() }
    compose.onNodeWithText("Capture").performClick()
    compose.waitUntil(5_000) { compose.onAllNodes(androidx.compose.ui.test.hasText("Could not save", substring = true)).fetchSemanticsNodes().isNotEmpty() }
    assertEquals(false, done)
  }

  private fun androidx.compose.ui.test.junit4.ComposeTestRule.onAllNodesWithText5() =
    onAllNodes(androidx.compose.ui.test.hasText("epsilon")).fetchSemanticsNodes().isNotEmpty()

  @Test fun `a page is loaded into the view once, and again only when it changes`() {
    val view = android.webkit.WebView(androidx.test.core.app.ApplicationProvider.getApplicationContext())
    assertEquals(true, dev.agync.agenthub.ui.screens.loadIfChanged(view, "<p>one</p>"))
    assertEquals(false, dev.agync.agenthub.ui.screens.loadIfChanged(view, "<p>one</p>"))
    assertEquals(true, dev.agync.agenthub.ui.screens.loadIfChanged(view, "<p>two</p>"))
  }
}
