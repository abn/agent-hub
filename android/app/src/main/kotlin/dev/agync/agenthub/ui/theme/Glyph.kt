package dev.agync.agenthub.ui.theme

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** The glyph names the board uses. */
object G {
  const val Now = "now"; const val Folder = "folder"; const val Doc = "doc"; const val Hub = "hub"
  const val Search = "search"; const val Back = "back"; const val Close = "close"; const val Check = "check"
  const val Send = "send"; const val Clock = "clock"; const val Lock = "lock"; const val Key = "key"
  const val Finger = "finger"; const val Qr = "qr"; const val Bell = "bell"; const val BellOff = "belloff"
  const val Offline = "offline"; const val Refresh = "refresh"; const val ChevR = "chevr"; const val ChevD = "chevd"
  const val Laptop = "laptop"; const val Agent = "agent"; const val Branch = "branch"; const val Sub = "sub"
  const val Comments = "comments"; const val Copy = "copy"; const val Plus = "plus"; const val Gear = "gear"
  const val Shield = "shield"; const val Phone = "phone"; const val Link = "link"; const val Filter = "filter"
  const val Idea = "idea"; const val Queue = "queue"; const val Open = "open"; const val Eye = "eye"
  const val EyeOff = "eyeoff"; const val Storage = "storage"; const val Wifi = "wifi"; const val Tile = "tile"
  const val Camera = "camera"; const val Kb = "kb"; const val Sun = "sun"; const val Swap = "swap"
  const val Dot3 = "dot3"; const val Warn = "warn"; const val Reply = "reply"; const val Unlock = "unlock"
  const val Trash = "trash"; const val History = "history"; const val Globe = "globe"; const val Play = "play"
  const val Pause = "pause"; const val Share = "share"; const val Cloud = "cloud"; const val Snooze = "snooze"
  const val ChevU = "chevu"
}

/** Turns the board's inner SVG (path, circle, ellipse, rect) into path data and fill flags. */
internal object GlyphParser {
  data class Shape(val path: String, val filled: Boolean)

  private val element = Regex("<(path|circle|ellipse|rect)\\s([^>]*?)/>")
  private val attr = Regex("([a-z-]+)=\"([^\"]*)\"")

  fun parse(svg: String): List<Shape> =
    element.findAll(svg).map { m ->
      val a = attr.findAll(m.groupValues[2]).associate { it.groupValues[1] to it.groupValues[2] }
      val filled = a["fill"] == "currentColor"
      fun f(k: String) = a[k]?.toFloat() ?: 0f
      val d =
        when (m.groupValues[1]) {
          "path" -> a.getValue("d")
          "circle" -> ellipse(f("cx"), f("cy"), f("r"), f("r"))
          "ellipse" -> ellipse(f("cx"), f("cy"), f("rx"), f("ry"))
          else -> rect(f("x"), f("y"), f("width"), f("height"), f("rx"))
        }
      Shape(d, filled)
    }.toList()

  private fun ellipse(cx: Float, cy: Float, rx: Float, ry: Float) =
    "M${cx - rx} ${cy}a$rx $ry 0 1 0 ${2 * rx} 0a$rx $ry 0 1 0 ${-2 * rx} 0Z"

  private fun rect(x: Float, y: Float, w: Float, h: Float, r: Float): String =
    if (r <= 0f) {
      "M$x $y h$w v$h h${-w}Z"
    } else {
      "M${x + r} ${y}h${w - 2 * r}a$r $r 0 0 1 $r $r v${h - 2 * r}a$r $r 0 0 1 ${-r} $r h${-(w - 2 * r)}" +
        "a$r $r 0 0 1 ${-r} ${-r} v${-(h - 2 * r)}a$r $r 0 0 1 $r ${-r}Z"
    }
}

private val vectorCache = HashMap<String, ImageVector>()

/** The glyph as a 24 unit vector, stroked 1.8 with round caps and joins, tinted at draw time. */
fun glyphVector(name: String): ImageVector =
  synchronized(vectorCache) {
    vectorCache.getOrPut(name) {
      val svg = GLYPH_SVG[name] ?: error("no glyph named $name")
      val builder = ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f)
      for (shape in GlyphParser.parse(svg)) {
        val nodes = PathParser().parsePathString(shape.path).toNodes()
        if (shape.filled) {
          builder.addPath(nodes, pathFillType = PathFillType.NonZero, fill = SolidColor(Color.Black))
        } else {
          builder.addPath(
            nodes,
            stroke = SolidColor(Color.Black),
            strokeLineWidth = 1.8f,
            strokeLineCap = StrokeCap.Round,
            strokeLineJoin = StrokeJoin.Round,
          )
        }
      }
      builder.build()
    }
  }

/**
 * A decorative or labelled glyph. [description] is the `contentDescription`; a glyph with no
 * description is hidden from TalkBack, so only pass null when words beside it say the same.
 */
@Composable
fun Glyph(
  name: String,
  modifier: Modifier = Modifier,
  size: Dp = 24.dp,
  tint: Color = Hub.colors.ink,
  description: String? = null,
) {
  val vector = remember(name) { glyphVector(name) }
  Icon(
    imageVector = vector,
    contentDescription = description,
    modifier = modifier.size(size),
    tint = tint,
  )
}

/** The hub mark: a triangle of three nodes. The nodes take copper unless told otherwise. */
@Composable
fun HubMark(
  modifier: Modifier = Modifier,
  size: Dp = 40.dp,
  color: Color = Hub.colors.ink,
  node: Color = Hub.colors.accent,
) {
  Canvas(modifier.size(size).clearAndSetSemantics {}) {
    val k = this.size.width / 24f
    fun p(x: Float, y: Float) = Offset(x * k, y * k)
    val path = androidx.compose.ui.graphics.Path().apply {
      moveTo(12f * k, 6.5f * k); lineTo(6f * k, 17f * k); lineTo(18f * k, 17f * k); close()
    }
    drawPath(path, color, style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.6f * k, join = StrokeJoin.Round))
    drawCircle(node, 2.6f * k, p(12f, 6f))
    drawCircle(node, 2.6f * k, p(5.8f, 17.3f))
    drawCircle(node, 2.6f * k, p(18.2f, 17.3f))
  }
}
