package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.assertTextEquals
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubTheme
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** adb input text, and a fast thumb, deliver characters faster than the view model echoes them back. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class FieldTest {
  @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

  @Test fun `characters typed faster than the value is echoed back are not lost`() {
    compose.mainClock.autoAdvance = false
    compose.setContent {
      HubTheme {
        var value by remember { mutableStateOf("") }
        val scope = rememberCoroutineScope()
        HubField(value, { new -> scope.launch { delay(50); value = new } }, description = "Code")
      }
    }
    val field = compose.onNodeWithContentDescription("Code")
    for (ch in "TND4F") {
      field.performTextInput(ch.toString())
      compose.mainClock.advanceTimeBy(30)
    }
    compose.mainClock.advanceTimeBy(500)
    field.assertTextEquals("TND4F")
  }
}
