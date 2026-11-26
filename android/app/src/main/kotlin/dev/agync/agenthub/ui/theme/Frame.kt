package dev.agync.agenthub.ui.theme

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.offset
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.Density
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** A 1dp hairline across the bottom of the element. */
fun Modifier.hairlineBottom(color: Color, inset: Dp = 0.dp): Modifier =
  drawBehind { drawLine(color, Offset(inset.toPx(), size.height - 0.5f), Offset(size.width, size.height - 0.5f), 1.dp.toPx()) }

fun Modifier.hairlineTop(color: Color): Modifier =
  drawBehind { drawLine(color, Offset(0f, 0.5f), Offset(size.width, 0.5f), 1.dp.toPx()) }

/** The window under the status bar and over the gesture bar: the app draws under both. */
@Composable
fun Screen(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
  Box(modifier.fillMaxSize().background(Hub.colors.bg)) { content() }
}

@Composable
fun StatusBarSpace() = Box(Modifier.windowInsetsPadding(WindowInsets.statusBars))

/** The small top bar: 64 high, a 48 leading slot that holds in every state. */
@Composable
fun SmallTopBar(
  title: String,
  modifier: Modifier = Modifier,
  subtitle: String? = null,
  onBack: (() -> Unit)? = null,
  backDescription: String = "Back",
  titleMuted: Boolean = false,
  trailingText: String? = null,
  leadGlyph: String = G.Back,
  actions: @Composable RowScope.() -> Unit = {},
) {
  val c = Hub.colors
  Row(
    modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(64.dp).padding(horizontal = 4.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    if (onBack != null) HubIconButton(leadGlyph, backDescription, onBack)
    Column(Modifier.weight(1f).padding(start = if (onBack != null) 4.dp else 12.dp)) {
      Text(
        title,
        style = (if (titleMuted) Hub.type.meta.copy(fontSize = 15.sp, color = c.ink2, fontWeight = FontWeight.Medium) else Hub.type.section.copy(fontSize = 20.sp, color = c.ink)),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier.semantics { heading() },
      )
      if (subtitle != null) Text(subtitle, style = Hub.type.mono.copy(color = c.ink2), maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
    if (trailingText != null) Text(trailingText, style = Hub.type.mono.copy(color = c.ink2), modifier = Modifier.padding(end = 16.dp))
    actions()
  }
}

/** The hub switcher in the large bar: mark, name, chevron. */
@Composable
fun HubSwitcher(name: String, onClick: () -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Row(
    modifier.heightIn(min = 48.dp)
      .clickable(role = Role.Button, onClick = onClick)
      .semantics { contentDescription = "Switch hub, $name" }
      .padding(start = 12.dp, end = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    HubMark(size = 28.dp, color = c.ink, node = c.accent)
    Text(name, style = Hub.type.metaStrong.copy(color = c.ink))
    Glyph(G.ChevD, size = 20.dp, tint = c.ink)
  }
}

/** The top band of a top-level screen before it collapses: hub switcher, actions, big title, meta line. */
@Composable
fun LargeHeader(title: String, meta: String, hubName: String, onSwitchHub: () -> Unit, actions: @Composable RowScope.() -> Unit) {
  Column(Modifier.fillMaxWidth()) {
    Row(Modifier.windowInsetsPadding(WindowInsets.statusBars).height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
      HubSwitcher(hubName, onSwitchHub, Modifier.weight(1f, fill = false))
      Box(Modifier.weight(1f))
      actions()
    }
    TitleBlock(title, meta)
  }
}

/** The 28 title and its 13 mono meta line: the part that scrolls away. */
@Composable
fun TitleBlock(title: String, meta: String) {
  val c = Hub.colors
  Column(Modifier.fillMaxWidth()) {
    Text(title, style = Hub.type.page.copy(color = c.ink), modifier = Modifier.padding(horizontal = 16.dp).semantics { heading() })
    Text(meta, style = Hub.type.mono.copy(color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 2.dp, bottom = 12.dp))
  }
}

/** The collapsed bar: the title keeps the large title's left edge (16 from the frame). */
@Composable
fun CollapsedBar(title: String, actions: @Composable RowScope.() -> Unit) {
  val c = Hub.colors
  Row(
    Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(56.dp).padding(horizontal = 4.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Text(title, style = Hub.type.section.copy(fontSize = 20.sp, color = c.ink), modifier = Modifier.weight(1f).padding(start = 12.dp).semantics { heading() })
    actions()
  }
}

/** A horizontal tools row of chips: grows and scrolls, never clips. */
@Composable
fun ToolsRow(modifier: Modifier = Modifier, content: @Composable RowScope.() -> Unit) {
  Row(
    modifier.fillMaxWidth().heightIn(min = 48.dp).hairlineBottom(Hub.colors.line).horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp),
    horizontalArrangement = Arrangement.spacedBy(8.dp),
    verticalAlignment = Alignment.CenterVertically,
    content = content,
  )
}

/** Tabs under a bar: 48 high, ink underline on the selected one. */
@Composable
fun HubTabs(labels: List<Pair<String, Int?>>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Row(modifier.fillMaxWidth().height(48.dp).hairlineBottom(c.line).padding(horizontal = 8.dp)) {
    labels.forEachIndexed { i, (label, n) ->
      val on = i == selected
      Box(
        Modifier.weight(1f).fillMaxSize().selectable(on, role = Role.Tab, onClick = { onSelect(i) }).drawBehind {
          if (on) drawLine(c.ink, Offset(16.dp.toPx(), size.height - 1.dp.toPx()), Offset(size.width - 16.dp.toPx(), size.height - 1.dp.toPx()), 2.dp.toPx())
        },
        contentAlignment = Alignment.Center,
      ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
          Text(label, style = Hub.type.button.copy(color = if (on) c.ink else c.ink2, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium))
          if (n != null) Text(n.toString(), style = Hub.type.monoSmall.copy(color = c.ink3, fontWeight = FontWeight.Medium))
        }
      }
    }
  }
}

/** A section label: 12 caps, hairline under, copper for the section that needs you. */
@Composable
fun SectionLabel(
  text: String,
  modifier: Modifier = Modifier,
  count: Int? = null,
  accent: Boolean = false,
  note: String? = null,
  endText: String? = null,
  onEnd: (() -> Unit)? = null,
  endColor: Color = Hub.colors.ink,
  topPadding: Dp = 20.dp,
  rule: Boolean = true,
) {
  val c = Hub.colors
  Row(
    modifier.fillMaxWidth().then(if (rule) Modifier.hairlineBottom(if (accent) c.ink else c.line) else Modifier).padding(start = 16.dp, end = if (endText != null) 4.dp else 16.dp, top = topPadding, bottom = if (endText != null) 0.dp else 10.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Text(text.uppercase(), style = Hub.type.label.copy(color = if (accent) c.accent else c.ink2), modifier = Modifier.semantics { heading() })
    if (count != null) Text("· $count", style = Hub.type.monoSmall.copy(color = if (accent) c.accent else c.ink2, fontWeight = FontWeight.SemiBold))
    if (note != null) Text("· $note", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
    Box(Modifier.weight(1f))
    if (endText != null && onEnd != null) TextAction(endText, onEnd, color = endColor)
  }
}

/** A banner: errors are ink with words, never red. */
@Composable
fun Banner(glyph: String, modifier: Modifier = Modifier, actions: @Composable (RowScope.() -> Unit)? = null, content: @Composable () -> Unit) {
  val c = Hub.colors
  Row(
    modifier.fillMaxWidth().background(c.sunk).hairlineBottom(c.line).padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = if (actions != null) 6.dp else 14.dp),
    horizontalArrangement = Arrangement.spacedBy(16.dp),
  ) {
    Glyph(glyph, tint = c.ink)
    Column(Modifier.weight(1f)) {
      content()
      if (actions != null) Row(Modifier.padding(start = 0.dp, top = 6.dp).padding(start = 0.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) { actions() }
    }
  }
}

/** The bottom bar of the phone: four destinations, 64 over the gesture inset. */
@Composable
fun BottomNav(items: List<NavItem>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  NavTextScale {
    Row(
      modifier.fillMaxWidth().background(c.bg).hairlineTop(c.line).windowInsetsPadding(WindowInsets.navigationBars).height(IntrinsicSize.Min).defaultMinSize(minHeight = 64.dp),
    ) {
      items.forEachIndexed { i, item ->
        val on = i == selected
        Column(
          Modifier.weight(1f).fillMaxHeight().defaultMinSize(minHeight = 64.dp).selectable(on, role = Role.Tab, onClick = { onSelect(i) }).semantics {
            if (item.badge != null && item.badge > 0) contentDescription = "${item.label}, ${item.badge} need you"
          },
          horizontalAlignment = Alignment.CenterHorizontally,
          verticalArrangement = Arrangement.Center,
        ) {
          NavIcon(item, on, i)
          Text(item.label, style = Hub.type.monoSmall.copy(fontFamily = PlexSans, color = if (on) c.ink else c.ink2, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium))
        }
      }
    }
  }
}

/**
 * Bar and rail text follows the system font up to 130% and stops there: four labels share one row
 * (or a 96 wide rail), and at 200% two of them cannot fit side by side. The bar still grows to hold
 * what it shows, and every label stays above the 12sp floor. The rest of the app follows to 200%.
 */
@Composable
private fun NavTextScale(content: @Composable () -> Unit) {
  val d = LocalDensity.current
  CompositionLocalProvider(LocalDensity provides Density(d.density, minOf(d.fontScale, NAV_FONT_SCALE_CAP)), content = content)
}

private const val NAV_FONT_SCALE_CAP = 1.3f

/** The destination's glyph in its pill. The count sits beside the glyph, clear of it, never on top. */
@Composable
private fun NavIcon(item: NavItem, on: Boolean, index: Int) {
  val c = Hub.colors
  Box(Modifier.size(width = 56.dp, height = 30.dp).then(if (on) Modifier.background(c.sunk, RoundedCornerShape(15.dp)) else Modifier), contentAlignment = Alignment.Center) {
    Box(Modifier.testTag("nav-glyph-$index")) { Glyph(item.glyph, tint = if (on) c.ink else c.ink2) }
    if (item.badge != null && item.badge > 0) CountBadge(item.badge, Modifier.align(Alignment.TopEnd).offset(x = 14.dp, y = (-8).dp).testTag("nav-badge-$index"))
  }
}

data class NavItem(val label: String, val glyph: String, val badge: Int? = null)

/** The rail on large screens: 96 wide, the mark above the destinations. */
@Composable
fun NavRail(items: List<NavItem>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Column(
    modifier.fillMaxSize().hairlineRight(c.line).windowInsetsPadding(WindowInsets.statusBars).padding(top = 12.dp),
    horizontalAlignment = Alignment.CenterHorizontally,
    verticalArrangement = Arrangement.spacedBy(12.dp),
  ) {
    HubMark(Modifier.padding(top = 8.dp, bottom = 20.dp), size = 32.dp, color = c.ink, node = c.accent)
    items.forEachIndexed { i, item ->
      val on = i == selected
      NavTextScale {
      Column(
        Modifier.defaultMinSize(minHeight = 48.dp).selectable(on, role = Role.Tab, onClick = { onSelect(i) }).semantics {
          if (item.badge != null && item.badge > 0) contentDescription = "${item.label}, ${item.badge} need you"
        },
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        NavIcon(item, on, i)
        Text(item.label, style = Hub.type.monoSmall.copy(fontFamily = PlexSans, color = if (on) c.ink else c.ink2, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Medium))
      }
      }
    }
  }
}

fun Modifier.hairlineRight(color: Color): Modifier =
  drawBehind { drawLine(color, Offset(size.width - 0.5f, 0f), Offset(size.width - 0.5f, size.height), 1.dp.toPx()) }

/** The sticky thumb bar on an item: hairline above, content padded to the gesture inset. */
@Composable
fun ActionBar(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
  Column(
    modifier.fillMaxWidth().background(Hub.colors.bg).hairlineTop(Hub.colors.line).windowInsetsPadding(WindowInsets.navigationBars).imePadding().padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
  ) { content() }
}

/** A foot of two full-width actions or one: onboarding and full-screen notes. */
@Composable
fun Foot(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
  Column(
    modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.navigationBars).imePadding().padding(start = 24.dp, end = 24.dp, top = 16.dp, bottom = 16.dp),
    verticalArrangement = Arrangement.spacedBy(12.dp),
  ) { content() }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun VerbRow(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
  FlowRow(modifier, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(0.dp)) { content() }
}

/** Tapping the whole row. Rows have a glyph and words, so it reads as a button. */
fun Modifier.clickableRow(onClick: () -> Unit): Modifier = clickable(role = Role.Button, onClick = onClick)

@Composable
fun Modifier.statusBarPadding(): Modifier = windowInsetsPadding(WindowInsets.statusBars)
