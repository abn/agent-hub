package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Text
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.getUnclippedBoundsInRoot
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.unit.dp
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import dev.agync.agenthub.ui.theme.ActionBar
import dev.agync.agenthub.ui.theme.Foot
import dev.agync.agenthub.ui.theme.HubTheme
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** With the keyboard up, the buttons at the foot of a screen must stay above it (edge to edge ignores adjustResize). */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class KeyboardTest {
  @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

  private fun showKeyboard(heightPx: Int) {
    compose.runOnUiThread {
      val view = compose.activity.window.decorView
      val insets = WindowInsetsCompat.Builder().setInsets(WindowInsetsCompat.Type.ime(), Insets.of(0, 0, 0, heightPx)).setVisible(WindowInsetsCompat.Type.ime(), true).build()
      ViewCompat.dispatchApplyWindowInsets(view, insets)
    }
    compose.waitForIdle()
  }

  private fun assertClearOfKeyboard(label: String) {
    val density = compose.density.density
    val top = compose.onNodeWithText(label).getUnclippedBoundsInRoot().bottom
    val keyboardTop = 915.dp - (400 / density).dp
    assertTrue("$label ends at $top but the keyboard starts at $keyboardTop", top <= keyboardTop)
  }

  @Test fun `a foot rises above the keyboard`() {
    compose.setContent { HubTheme { Box(Modifier.fillMaxSize()) { Foot(Modifier.align(Alignment.BottomCenter)) { Text("Continue") } } } }
    showKeyboard(400)
    assertClearOfKeyboard("Continue")
  }

  @Test fun `an action bar rises above the keyboard`() {
    compose.setContent { HubTheme { Box(Modifier.fillMaxSize()) { ActionBar(Modifier.align(Alignment.BottomCenter)) { Text("Send") } } } }
    showKeyboard(400)
    assertClearOfKeyboard("Send")
  }
}
