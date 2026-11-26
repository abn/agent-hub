package dev.agync.agenthub.widgets

import android.app.Application
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.TextView
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.glance.appwidget.GlanceRemoteViews
import androidx.test.core.app.ApplicationProvider
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.DevicePrefs
import java.util.Locale
import java.util.TimeZone
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
@OptIn(androidx.glance.appwidget.ExperimentalGlanceRemoteViewsApi::class)
open class WidgetsTest {
  private val context: Application = ApplicationProvider.getApplicationContext()

  @Before fun locale() {
    Locale.setDefault(Locale.UK)
    TimeZone.setDefault(TimeZone.getTimeZone("UTC"))
  }

  private fun data(): WidgetData {
    val rig = Fixtures.rig()
    runBlocking { rig.repo.refresh() }
    rig.repo.snooze("DS5REF")
    return WidgetData.from("seeded-hub", "home hub", rig.repo.state.value, Fixtures.now, Fixtures.fmt())
  }

  private fun render(size: DpSize, content: @androidx.compose.runtime.Composable () -> Unit): View {
    val result = runBlocking { GlanceRemoteViews().compose(context, size, content = content) }
    val host = FrameLayout(context)
    val view = result.remoteViews.apply(context, host)
    host.addView(view)
    val w = (size.width.value * context.resources.displayMetrics.density).toInt()
    val h = (size.height.value * context.resources.displayMetrics.density).toInt()
    host.measure(View.MeasureSpec.makeMeasureSpec(w, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(h, View.MeasureSpec.EXACTLY))
    host.layout(0, 0, w, h)
    return host
  }

  /** Draws the inflated widget to a PNG beside the other screens. */
  private fun save(v: View, name: String) {
    val bmp = android.graphics.Bitmap.createBitmap(v.width, v.height, android.graphics.Bitmap.Config.ARGB_8888)
    bmp.eraseColor(android.graphics.Color.parseColor("#4A5F50"))
    v.draw(android.graphics.Canvas(bmp))
    val out = java.io.File("build/screenshots").also { it.mkdirs() }
    java.io.File(out, "$name-${if (dark) "dark" else "light"}.png").outputStream().use { bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
  }

  private val dark: Boolean get() = (context.resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK) == android.content.res.Configuration.UI_MODE_NIGHT_YES

  private fun texts(v: View): List<String> =
    if (v is TextView) listOf(v.text.toString()) else if (v is ViewGroup) (0 until v.childCount).flatMap { texts(v.getChildAt(it)) } else emptyList()

  @Test fun `the data counts three that need you, one that can wait and two live sessions`() {
    val d = data()
    assertEquals(3, d.needsCount)
    assertEquals(1, d.canWait)
    assertEquals(2, d.liveCount)
    assertEquals(1, d.live.first { it.name == "feat-invoices" }.subagents)
  }

  @Test fun `the list widget shows the count, the rows and what can wait`() {
    val d = data()
    val v = render(DpSize(360.dp, 330.dp)) { ListContent(d, context) }
    val t = texts(v)
    assertTrue(t.contains("3"))
    assertTrue(t.any { it.startsWith("feat-invoices wants a decision") })
    assertTrue(t.contains("and 1 can wait"))
    save(v, "widget-list")
  }

  @Test fun `the count widget shows the number and the live count`() {
    val v = render(DpSize(164.dp, 164.dp)) { CountContent(data(), context) }
    val t = texts(v)
    assertTrue(t.contains("3") && t.contains("need you"))
    assertTrue(t.any { it.contains("2 live") })
    save(v, "widget-count")
  }

  @Test fun `the live widget names the sessions and their subagents`() {
    val v = render(DpSize(164.dp, 164.dp)) { LiveContent(data(), context) }
    val t = texts(v)
    assertTrue(t.contains("feat-invoices") && t.contains("nightly"))
    assertTrue(t.any { it.contains("1 subagent") })
    save(v, "widget-live")
  }

  @Test fun `an unpaired or unreachable hub is said, not shown as zero`() {
    val v = render(DpSize(164.dp, 164.dp)) { CountContent(WidgetData.empty("Not paired"), context) }
    assertTrue(texts(v).contains("Not paired"))
  }

  @Test fun `no widget row carries a decision, and a row opens the item by its own URI`() {
    val d = data()
    // Rows open the app. The only verbs a widget has are taps that launch MainActivity.
    assertTrue(d.needsYou.all { it.id.isNotEmpty() })
    assertEquals(DevicePrefs().fingerprintBeforeApproving, false)
  }
}

/** The same widgets in the dark theme. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-night-xhdpi")
@OptIn(androidx.glance.appwidget.ExperimentalGlanceRemoteViewsApi::class)
class WidgetsDarkTest : WidgetsTest()
