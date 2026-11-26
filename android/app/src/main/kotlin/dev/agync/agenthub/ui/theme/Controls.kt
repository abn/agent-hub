package dev.agync.agenthub.ui.theme

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.input.TextFieldLineLimits
import androidx.compose.foundation.text.input.KeyboardActionHandler
import androidx.compose.foundation.text.input.TextObfuscationMode
import androidx.compose.foundation.text.input.rememberTextFieldState
import androidx.compose.foundation.text.input.setTextAndPlaceCursorAtEnd
import androidx.compose.foundation.text.BasicSecureTextField
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.layout.layout
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** Controls keep the web's 4 radius; sheets and dialogs take 12 to 16. */
val ControlShape = RoundedCornerShape(4.dp)

/**
 * A 2 dp copper ring drawn 2 dp outside the control. It shows only for keyboard and D-pad
 * focus, never on touch, because touch does not move focus onto a button.
 */
fun Modifier.hubFocusRing(focused: Boolean, color: Color, radius: Dp = 4.dp): Modifier =
  if (!focused) this
  else
    drawBehind {
      val w = 2.dp.toPx()
      val out = 2.dp.toPx() + w / 2
      drawRoundRect(
        color = color,
        topLeft = Offset(-out, -out),
        size = Size(size.width + out * 2, size.height + out * 2),
        cornerRadius = CornerRadius(radius.toPx() + out),
        style = Stroke(w),
      )
    }

enum class ButtonKind { Default, Primary, Ink, Text, Danger, DangerPrimary }

enum class ButtonSize(val height: Dp) { Small(36.dp), Normal(40.dp), Large(52.dp) }

/**
 * A button drawn at 40 (36 compact, 52 for thumb-bar verbs) with a hit area that reaches 48.
 * Its label wraps under large text instead of truncating.
 */
@Composable
fun HubButton(
  text: String,
  onClick: () -> Unit,
  modifier: Modifier = Modifier,
  kind: ButtonKind = ButtonKind.Default,
  size: ButtonSize = ButtonSize.Normal,
  glyph: String? = null,
  enabled: Boolean = true,
  contentDescription: String? = null,
  tint: Color? = null,
  outline: Color? = null,
) {
  val c = Hub.colors
  val source = remember { MutableInteractionSource() }
  val focused by source.collectIsFocusedAsState()
  val bg: Color
  var fg: Color
  var border: Color
  when (kind) {
    ButtonKind.Primary -> { bg = c.accent; fg = c.inv; border = c.accent }
    ButtonKind.Ink -> { bg = c.ink; fg = c.inv; border = c.ink }
    ButtonKind.Text -> { bg = Color.Transparent; fg = c.ink; border = Color.Transparent }
    ButtonKind.Danger -> { bg = Color.Transparent; fg = c.danger; border = c.danger }
    ButtonKind.DangerPrimary -> { bg = c.danger; fg = c.inv; border = c.danger }
    ButtonKind.Default -> { bg = Color.Transparent; fg = c.ink; border = c.lineStrong }
  }
  if (tint != null) fg = tint
  if (outline != null) border = outline
  val strong = kind == ButtonKind.Primary || kind == ButtonKind.Ink || kind == ButtonKind.DangerPrimary
  val style =
    (if (size == ButtonSize.Large) Hub.type.buttonLarge else Hub.type.button)
      .copy(fontWeight = if (strong) androidx.compose.ui.text.font.FontWeight.SemiBold else androidx.compose.ui.text.font.FontWeight.Medium, color = fg)
  Box(
    modifier
      .alpha(if (enabled) 1f else 0.45f)
      .minimumInteractiveComponentSize()
      .semantics { if (contentDescription != null) this.contentDescription = contentDescription }
      .clickable(interactionSource = source, indication = null, enabled = enabled, role = Role.Button, onClick = onClick),
    contentAlignment = Alignment.Center,
    // A button given a width (a weight, fillMaxWidth) draws to it; one in a wrapping row hugs its label.
    propagateMinConstraints = true,
  ) {
    Row(
      Modifier
        .layout { measurable, constraints ->
          val p = measurable.measure(constraints.copy(minHeight = 0))
          layout(p.width, p.height) { p.place(0, 0) }
        }
        .hubFocusRing(focused, c.accent)
        .defaultMinSize(minHeight = size.height)
        .background(bg, ControlShape)
        .border(BorderStroke(1.dp, border), ControlShape)
        .padding(horizontal = if (kind == ButtonKind.Text || size == ButtonSize.Large) 12.dp else 16.dp, vertical = 6.dp),
      horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      if (glyph != null) Glyph(glyph, size = 20.dp, tint = fg)
      Text(text, style = style, textAlign = TextAlign.Center, modifier = Modifier.weight(1f, fill = false))
    }
  }
}

/** A 48 by 48 icon button. [description] is its `contentDescription` and is required. */
@Composable
fun HubIconButton(
  glyph: String,
  description: String,
  onClick: () -> Unit,
  modifier: Modifier = Modifier,
  tint: Color = Hub.colors.ink,
  glyphSize: Dp = 24.dp,
  badge: Int? = null,
) {
  val source = remember { MutableInteractionSource() }
  val focused by source.collectIsFocusedAsState()
  Box(
    modifier
      .size(48.dp)
      .clickable(interactionSource = source, indication = null, role = Role.Button, onClickLabel = null, onClick = onClick)
      .semantics { contentDescription = description },
    contentAlignment = Alignment.Center,
  ) {
    Box(Modifier.size(40.dp).hubFocusRing(focused, Hub.colors.accent, 20.dp), contentAlignment = Alignment.Center) {
      Glyph(glyph, size = glyphSize, tint = tint)
    }
    if (badge != null) CountBadge(badge, Modifier.align(Alignment.TopEnd).padding(top = 6.dp, end = 4.dp))
  }
}

/** The copper count badge: 20 high, mono, the one place a number takes the accent. */
@Composable
fun CountBadge(count: Int, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Box(
    modifier.defaultMinSize(minWidth = 20.dp, minHeight = 20.dp).background(c.accent, CircleShape).padding(horizontal = 5.dp),
    contentAlignment = Alignment.Center,
  ) {
    Text(count.toString(), style = Hub.type.monoSmall.copy(color = c.inv, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold))
  }
}

/** A filter chip: 32 drawn, 48 answered. The row of chips scrolls, so a chip never truncates. */
@Composable
fun HubChip(
  label: String,
  selected: Boolean,
  onClick: () -> Unit,
  modifier: Modifier = Modifier,
  count: Int? = null,
  countAccent: Boolean = false,
) {
  val c = Hub.colors
  val source = remember { MutableInteractionSource() }
  val focused by source.collectIsFocusedAsState()
  Box(
    modifier.minimumInteractiveComponentSize().selectable(selected, interactionSource = source, indication = null, role = Role.Tab, onClick = onClick),
    contentAlignment = Alignment.Center,
  ) {
    Row(
      Modifier
        .hubFocusRing(focused, c.accent)
        .defaultMinSize(minHeight = 32.dp)
        .background(if (selected) c.sunk else Color.Transparent, ControlShape)
        .border(1.dp, if (selected) c.ink else c.lineStrong, ControlShape)
        .padding(horizontal = 12.dp, vertical = 4.dp),
      horizontalArrangement = Arrangement.spacedBy(6.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      Text(label, style = Hub.type.button.copy(color = c.ink, fontWeight = if (selected) androidx.compose.ui.text.font.FontWeight.SemiBold else androidx.compose.ui.text.font.FontWeight.Medium))
      if (count != null) Text(count.toString(), style = Hub.type.monoSmall.copy(color = if (countAccent) c.accent else c.ink2, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium))
    }
  }
}

/** The status pill: a dot, a word, copper wash. State is never colour alone. */
@Composable
fun StatusPill(text: String, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Row(
    modifier.defaultMinSize(minHeight = 24.dp).background(c.accentBg, ControlShape).padding(horizontal = 8.dp, vertical = 4.dp),
    horizontalArrangement = Arrangement.spacedBy(6.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Box(Modifier.size(6.dp).background(c.accent, CircleShape))
    Text(text, style = Hub.type.label.copy(letterSpacing = androidx.compose.ui.unit.TextUnit.Unspecified, color = c.accent))
  }
}

enum class Dot { Ring, Filled, Live, Idle }

/** A state word with its dot. Only the state word and dot of a live thing are green. */
@Composable
fun StateWord(
  word: String,
  dot: Dot,
  modifier: Modifier = Modifier,
  rest: String? = null,
  textStyle: TextStyle = Hub.type.meta.copy(fontSize = androidx.compose.ui.unit.TextUnit(13f, androidx.compose.ui.unit.TextUnitType.Sp)),
) {
  val c = Hub.colors
  val color = if (dot == Dot.Live) c.ok else c.ink2
  Row(modifier, horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
    val ring = when (dot) { Dot.Live -> c.ok; Dot.Idle -> c.ink2; Dot.Filled -> c.ink3; Dot.Ring -> c.ink3 }
    Box(
      Modifier.size(8.dp).let {
        when (dot) {
          Dot.Live, Dot.Filled -> it.background(if (dot == Dot.Live) c.ok else c.ink3, CircleShape)
          else -> it.border(1.5.dp, ring, CircleShape)
        }
      },
    )
    Text(word, style = textStyle.copy(color = color, fontWeight = if (dot == Dot.Live) androidx.compose.ui.text.font.FontWeight.Medium else androidx.compose.ui.text.font.FontWeight.Normal))
    if (rest != null) Text(rest, style = textStyle.copy(color = c.ink2))
  }
}

/** The text field: 52 high, 4 radius, line-strong border. State-backed, with a value in and out. */
@Composable
fun HubField(
  value: String,
  onValueChange: (String) -> Unit,
  modifier: Modifier = Modifier,
  hint: String = "",
  textStyle: TextStyle = Hub.type.body,
  singleLine: Boolean = true,
  minHeight: Dp = 52.dp,
  keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
  secure: Boolean = false,
  revealed: Boolean = false,
  trailing: (@Composable RowScope.() -> Unit)? = null,
  description: String? = null,
  /** What the keyboard's action key does: send, search, done. */
  onAction: (() -> Unit)? = null,
) {
  val c = Hub.colors
  val source = remember { MutableInteractionSource() }
  val focused by source.collectIsFocusedAsState()
  val state = rememberTextFieldState(value)
  val act by rememberUpdatedState(onAction)
  val latest by rememberUpdatedState(onValueChange)
  val current by rememberUpdatedState(value)
  // What the field itself sent out lately. The value coming back is an echo of one of those, possibly a
  // stale one when characters arrive faster than the owner echoes them, and must not overwrite newer
  // typing. Only a value the field never sent (an uppercased code, a reset) is written in.
  val sent = remember { ArrayDeque<String>() }
  LaunchedEffect(value) { if (state.text.toString() != value && value !in sent) state.setTextAndPlaceCursorAtEnd(value) }
  LaunchedEffect(state) {
    snapshotFlow { state.text.toString() }.collect {
      if (it != current) {
        sent.addLast(it)
        if (sent.size > 16) sent.removeFirst()
        latest(it)
      }
    }
  }
  val focus = remember { FocusRequester() }
  val keyboard = LocalSoftwareKeyboardController.current
  // The field fills its box, and a tap anywhere in the box focuses it: it used to be as wide as its text.
  val field = Modifier.fillMaxWidth().focusRequester(focus).semantics { if (description != null) contentDescription = description }
  Row(
    modifier
      .heightIn(min = minHeight)
      .clickable(interactionSource = null, indication = null) { focus.requestFocus(); keyboard?.show() }
      .background(c.raised, ControlShape)
      .border(if (focused) 2.dp else 1.dp, if (focused) c.accent else c.lineStrong, ControlShape)
      .padding(start = 16.dp, end = 12.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Box(Modifier.weight(1f).padding(vertical = 8.dp)) {
      if (value.isEmpty() && hint.isNotEmpty()) Text(hint, style = textStyle.copy(color = c.ink3))
      if (secure) {
        BasicSecureTextField(
          state = state, textStyle = textStyle.copy(color = c.ink), interactionSource = source, cursorBrush = SolidColor(c.ink),
          keyboardOptions = keyboardOptions, textObfuscationMode = if (revealed) TextObfuscationMode.Visible else TextObfuscationMode.RevealLastTyped, modifier = field,
        )
      } else {
        BasicTextField(
          state = state, textStyle = textStyle.copy(color = c.ink), interactionSource = source, cursorBrush = SolidColor(c.ink),
          keyboardOptions = keyboardOptions, lineLimits = if (singleLine) TextFieldLineLimits.SingleLine else TextFieldLineLimits.Default, modifier = field,
          onKeyboardAction = if (onAction != null) KeyboardActionHandler { act?.invoke() } else null,
        )
      }
    }
    if (trailing != null) trailing()
  }
}

/** The switch: a 52 by 32 track, border in line-strong, ink when on. Toggles as a whole row. */
@Composable
fun HubSwitch(checked: Boolean, onCheckedChange: ((Boolean) -> Unit)?, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Box(modifier.size(width = 52.dp, height = 32.dp)) {
    Box(
      Modifier.matchParentSize()
        .background(if (checked) c.ink else Color.Transparent, CircleShape)
        .border(2.dp, if (checked) c.ink else c.lineStrong, CircleShape),
    )
    val thumb = if (checked) 24.dp else 16.dp
    Box(
      Modifier.align(if (checked) Alignment.CenterEnd else Alignment.CenterStart)
        .padding(horizontal = if (checked) 2.dp else 6.dp)
        .size(thumb)
        .background(if (checked) c.inv else c.lineStrong, CircleShape),
    )
  }
}

/** Makes a whole settings row a toggle target with switch semantics. */
fun Modifier.switchRow(checked: Boolean, onChange: (Boolean) -> Unit): Modifier =
  toggleable(value = checked, role = Role.Switch, onValueChange = onChange)

@Composable
fun HubRadio(selected: Boolean, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Box(modifier.size(20.dp).border(2.dp, if (selected) c.ink else c.lineStrong, CircleShape), contentAlignment = Alignment.Center) {
    if (selected) Box(Modifier.size(10.dp).background(c.ink, CircleShape))
  }
}

@Composable
fun HubCheckbox(checked: Boolean, modifier: Modifier = Modifier) {
  val c = Hub.colors
  Box(
    modifier.size(20.dp).background(if (checked) c.ink else Color.Transparent, RoundedCornerShape(2.dp)).border(2.dp, if (checked) c.ink else c.lineStrong, RoundedCornerShape(2.dp)),
    contentAlignment = Alignment.Center,
  ) {
    if (checked) Glyph(G.Check, size = 14.dp, tint = c.inv)
  }
}

/** A text action in a section label or tools row; 48 high hit box, no border. */
@Composable
fun TextAction(text: String, onClick: () -> Unit, modifier: Modifier = Modifier, color: Color = Hub.colors.ink) {
  val source = remember { MutableInteractionSource() }
  val focused by source.collectIsFocusedAsState()
  Box(
    modifier.heightIn(min = 48.dp).clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onClick).padding(horizontal = 12.dp),
    contentAlignment = Alignment.Center,
  ) {
    Text(text, style = Hub.type.button.copy(color = color), modifier = Modifier.hubFocusRing(focused, Hub.colors.accent))
  }
}
