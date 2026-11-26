package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onRoot
import com.github.takahirom.roborazzi.captureRoboImage
import dev.agync.agenthub.MainDispatcherRule
import java.util.Locale
import java.util.TimeZone
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.ParameterizedRobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.ParameterizedRobolectricTestRunner.Parameters
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/**
 * Draws every board frame the app has, light and dark, to build/screenshots. With
 * -Proborazzi.test.record=true the images are written; otherwise the screens are only composed,
 * which still proves each one draws without throwing.
 */
@RunWith(ParameterizedRobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class BoardFramesTest(private val name: String, private val dark: Boolean) {
  private val dispatcher = MainDispatcherRule()
  private val compose = createAndroidComposeRule<ComponentActivity>()

  @get:Rule val chain: RuleChain = RuleChain.outerRule(dispatcher).around(compose)

  @Before fun fixLocale() {
    Locale.setDefault(Locale.UK)
    TimeZone.setDefault(TimeZone.getTimeZone("UTC"))
  }

  @Test fun frame() {
    val f = Frames.build(name)
    if (f.tablet) RuntimeEnvironment.setQualifiers("w1280dp-h800dp-xhdpi")
    compose.setContent {
      if (f.tablet) TabletFrame(dark) { f.content() } else PhoneFrame(dark, f.time, f.fontScale, f.nav, f.badge) { f.content() }
    }
    compose.waitForIdle()
    compose.onRoot().captureRoboImage("build/screenshots/$name-${if (dark) "dark" else "light"}.png")
  }

  companion object {
    @JvmStatic
    @Parameters(name = "{0} dark={1}")
    fun frames(): List<Array<Any>> = Frames.names.flatMap { n -> listOf(arrayOf<Any>(n, false), arrayOf<Any>(n, true)) }
  }
}
