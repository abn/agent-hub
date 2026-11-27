package dev.agync.agenthub.ui.screens

import android.app.Activity
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import dev.agync.agenthub.data.DevicePrefs
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.pairing.Handshake
import dev.agync.agenthub.pairing.PairOffer
import dev.agync.agenthub.ui.theme.Banner
import dev.agync.agenthub.ui.components.FactList
import dev.agync.agenthub.ui.components.FactValue
import dev.agync.agenthub.ui.components.GlyphList
import dev.agync.agenthub.ui.components.HeroNote
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubRadio
import dev.agync.agenthub.ui.theme.HubSwitch
import dev.agync.agenthub.ui.theme.Foot
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.hairlineBottom
import dev.agync.agenthub.ui.theme.switchRow
import dev.agync.agenthub.viewmodel.PairStep
import dev.agync.agenthub.viewmodel.StopReason
import java.time.Clock
import java.time.Instant
import kotlinx.coroutines.delay

private fun plain(s: String) = AnnotatedString(s)

private fun withBold(before: String, strong: String, after: String, c: Color): AnnotatedString =
  buildAnnotatedString {
    append(before)
    withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = c)) { append(strong) }
    append(after)
  }

/** Everything the pairing flow needs from outside: the camera, system settings, the permission ask. */
data class PairActions(
  val onStartScan: () -> Unit,
  val onStartManual: () -> Unit,
  val onBack: () -> Unit,
  val onEditManual: (String, String) -> Unit,
  val onSubmitManual: () -> Unit,
  val onName: (String) -> Unit,
  val onPair: () -> Unit,
  val onCancelPairing: () -> Unit,
  val onCheckMatch: () -> Unit,
  val onCheckDiffer: () -> Unit,
  val onStayConnected: (Boolean) -> Unit,
  val onAllowNotifications: () -> Unit,
  val onNotNow: () -> Unit,
  val onOpenSettings: () -> Unit,
  val onPrefs: ((DevicePrefs) -> DevicePrefs) -> Unit,
  val onContinue: () -> Unit,
  val onStartAgain: () -> Unit,
)

@Composable
fun PairScreen(
  step: PairStep,
  prefs: DevicePrefs,
  fmt: Fmt,
  actions: PairActions,
  notice: String? = null,
  clock: Clock = Clock.systemDefaultZone(),
  scanner: @Composable () -> Unit,
) {
  val c = Hub.colors
  Box(Modifier.fillMaxSize().background(if (step is PairStep.Scan && !step.cameraDenied) CameraBg else c.bg)) {
    when (step) {
      PairStep.Welcome -> Welcome(actions, notice)
      is PairStep.Scan -> if (step.cameraDenied) CameraDenied(actions) else Scan(actions, scanner)
      is PairStep.Manual -> Manual(step, actions)
      is PairStep.Confirm -> Confirm(step.offer, step.deviceName, fmt, actions)
      is PairStep.Connecting -> Working(actions)
      is PairStep.CheckString -> CheckString(step, fmt, clock, actions)
      is PairStep.Alerts -> if (step.notificationsDenied) NotifyDenied(step, actions) else Alerts(step, prefs, actions)
      is PairStep.CodeFailed -> CodeFailed(step, actions)
      is PairStep.Confirmed -> Confirmed(step, actions)
      is PairStep.Stopped -> Stopped(step, fmt, actions)
      PairStep.Done -> Unit
    }
  }
}

private val CameraBg = Color(0xFF0B0E0C)
private val CameraInk = Color(0xFFE4EAE5)
private val CameraMuted = Color(0xFFB5BFB8)

@Composable
private fun Welcome(a: PairActions, notice: String?) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Box(Modifier.padding(top = 50.dp))
      HeroNote(
        title = "Connect this phone to your hub",
        mark = true,
        topPadding = 24.dp,
        paragraphs = listOf(
          plain("Your hub runs on your own machine. This phone pairs with it once, by a code, and then finds it at home or away."),
          withBold("On the hub's web app, open ", "Settings, Phones, Add a phone", ". It shows a code.", c.ink),
          plain("No Add a phone there? The hub is older than 1.13. Update it first."),
        ),
        small = true,
      )
      if (notice != null) Text(notice, style = Hub.type.meta.copy(color = c.ink), modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 16.dp))
    }
    Foot {
      HubButton("Scan the code", a.onStartScan, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large, glyph = G.Qr)
      HubButton("Type the code", a.onStartManual, Modifier.fillMaxWidth(), size = ButtonSize.Large)
    }
  }
}

@Composable
private fun StepBar(title: String, step: String, a: PairActions, onBack: (() -> Unit)?, backDescription: String = "Back", close: Boolean = false, onDark: Boolean = false) {
  if (onDark) {
    Row(
      Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(64.dp).padding(horizontal = 4.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      HubIconButton(G.Back, backDescription, onBack ?: a.onBack, tint = CameraInk)
      Text(title, style = Hub.type.section.copy(fontSize = 20.sp, color = CameraInk), modifier = Modifier.weight(1f).padding(start = 4.dp))
      Text(step, style = Hub.type.mono.copy(color = CameraMuted), modifier = Modifier.padding(end = 16.dp))
    }
  } else {
    SmallTopBar(title, trailingText = step, onBack = onBack, backDescription = backDescription, leadGlyph = if (close) G.Close else G.Back)
  }
}

@Composable
private fun Scan(a: PairActions, scanner: @Composable () -> Unit) {
  LightStatusIcons()
  Column(Modifier.fillMaxSize()) {
    StepBar("Scan the code", "1 of 2", a, a.onBack, onDark = true)
    Box(Modifier.padding(top = 40.dp).align(Alignment.CenterHorizontally).size(260.dp)) {
      Box(Modifier.padding(40.dp).fillMaxSize()) { scanner() }
      Viewfinder()
    }
    Text(
      buildAnnotatedString {
        append("Point the camera at the code on the hub's ")
        withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = CameraInk)) { append("Add a phone") }
        append(" screen. It holds the hub's key and where to find it. It works once, for 15 minutes.")
      },
      style = Hub.type.body.copy(lineHeight = 24.sp, color = CameraMuted),
      modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 32.dp),
    )
    Box(Modifier.weight(1f))
    Foot { HubButton("Type the code instead", a.onStartManual, Modifier.fillMaxWidth(), size = ButtonSize.Large, tint = CameraInk, outline = Color(0xFF7A867E)) }
  }
}

/** The camera is dark in both themes, so the system bars take light icons while it shows. */
@Composable
private fun LightStatusIcons() {
  val view = LocalView.current
  if (view.isInEditMode) return
  DisposableEffect(view) {
    val window = (view.context as? Activity)?.window
    val controls = window?.let { WindowCompat.getInsetsController(it, view) }
    val before = controls?.isAppearanceLightStatusBars
    controls?.isAppearanceLightStatusBars = false
    onDispose { if (before != null) controls.isAppearanceLightStatusBars = before }
  }
}

/** The four corner brackets of the viewfinder. They are ink on the dark camera, not copper. */
@Composable
private fun Viewfinder() {
  Box(
    Modifier.fillMaxSize().clearAndSetSemantics {}.drawBehind {
      val len = 44.dp.toPx()
      val w = 3.dp.toPx()
      val r = 12.dp.toPx()
      val s = Stroke(w, cap = androidx.compose.ui.graphics.StrokeCap.Round)
      fun corner(x: Float, y: Float, dx: Float, dy: Float) {
        val p = androidx.compose.ui.graphics.Path().apply {
          moveTo(x + dx * len, y)
          lineTo(x + dx * r, y)
          quadraticTo(x, y, x, y + dy * r)
          lineTo(x, y + dy * len)
        }
        drawPath(p, CameraInk, style = s)
      }
      val inset = w / 2
      corner(inset, inset, 1f, 1f)
      corner(size.width - inset, inset, -1f, 1f)
      corner(inset, size.height - inset, 1f, -1f)
      corner(size.width - inset, size.height - inset, -1f, -1f)
    },
  )
}

@Composable
private fun Manual(s: PairStep.Manual, a: PairActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    StepBar("Type the code", "1 of 2", a, a.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp)) {
        Text("Code", style = Hub.type.meta.copy(fontWeight = FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 6.dp))
        HubField(
          s.code, { a.onEditManual(it.uppercase(), s.address) }, Modifier.fillMaxWidth(), textStyle = Hub.type.codeInput,
          hint = "TND-XXXX-XXXX-XXXX", description = "Code",
          keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false),
        )
        Text(
          "From Add a phone on the hub. Letters and digits; codes never use 0, 1, I, L or O.",
          style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(top = 6.dp),
        )
      }
      Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp)) {
        Text("Hub address", style = Hub.type.meta.copy(fontWeight = FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 6.dp))
        HubField(
          s.address, { a.onEditManual(s.code, it) }, Modifier.fillMaxWidth(), textStyle = Hub.type.mono.copy(fontSize = 15.sp), hint = "192.168.1.10:48732", description = "Hub address",
          keyboardOptions = KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, autoCorrectEnabled = false),
        )
        Text("The address and port the hub shows beside the code.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(top = 6.dp))
      }
      if (s.invalid) Text("That is not shaped like a code. Check it against the hub and try again.", style = Hub.type.meta.copy(color = c.ink), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp))
      Banner(G.Shield, Modifier.padding(top = 24.dp)) {
        Text(
          "Without the picture code, this phone cannot check the hub's key ahead of time. You compare the check string on both screens instead.",
          style = Hub.type.meta.copy(fontSize = 15.sp, color = c.ink),
        )
      }
    }
    Foot { HubButton("Continue", a.onSubmitManual, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large, enabled = s.code.isNotBlank() && s.address.isNotBlank()) }
  }
}

@Composable
private fun Confirm(offer: PairOffer, name: String, fmt: Fmt, a: PairActions) {
  val c = Hub.colors
  val hub = offer.hubName ?: "this hub"
  Column(Modifier.fillMaxSize()) {
    StepBar("Pair with $hub?", "1 of 2", a, a.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      val rows = buildList<Pair<String, @Composable () -> Unit>> {
        if (offer.hubName != null) add("Hub" to { Text(offer.hubName, style = Hub.type.bodyStrong.copy(fontSize = 15.sp, color = c.ink)) })
        if (offer.codeLabel != null) add("Code" to { FactValue(offer.codeLabel) })
        if (offer.address != null) add("Address" to { FactValue(offer.address) })
        if (offer.worksUntil != null) add("Works until" to { FactValue(fmt.time(offer.worksUntil), suffix = "· once") })
      }
      if (rows.isNotEmpty()) FactList(rows, Modifier.padding(top = 8.dp), labelWidth = 104.dp)
      Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp)) {
        Text("Name this phone", style = Hub.type.meta.copy(fontWeight = FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 6.dp))
        HubField(name, a.onName, Modifier.fillMaxWidth(), description = "Name this phone")
        Text("The hub lists it under Settings, Phones. You can unpair it from there.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(top = 6.dp))
      }
      GlyphList(
        listOf(
          G.Check to plain("Reads and answers everything you can reach from the web app."),
          G.Check to plain("Has its own identity. Unpairing it leaves the admin token and every agent alone."),
          G.Check to plain("Nothing works until you confirm a check string on the hub."),
        ),
        Modifier.padding(top = 24.dp), horizontalPadding = 16.dp,
      )
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Cancel", a.onBack, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Pair", a.onPair, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large)
      }
    }
  }
}

@Composable
private fun Working(a: PairActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    StepBar("Connecting", "1 of 2", a, null, close = true)
    Column(Modifier.weight(1f).padding(horizontal = 16.dp)) {
      StepLine(G.Check, plain("Looking for the hub"), done = false, working = true)
    }
    Foot { HubButton("Cancel pairing", a.onCancelPairing, Modifier.fillMaxWidth(), size = ButtonSize.Large) }
  }
}

/**
 * One step of the handshake: a check when done, a ring when working. The ring pulses slowly while
 * [pulse] is set and animations are on. [announce] makes the text a polite live region that reads
 * that string: it changes with the state, not with a ticking [detail].
 */
@Composable
private fun StepLine(
  glyph: String,
  text: AnnotatedString,
  done: Boolean,
  working: Boolean = false,
  pulse: Boolean = false,
  announce: String? = null,
  detail: String? = null,
) {
  val c = Hub.colors
  val alpha = if (working && pulse) rememberPulse() else 1f
  Row(Modifier.fillMaxWidth().hairlineBottom(c.line).padding(vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
    if (working) Box(Modifier.alpha(alpha).padding(2.dp).size(20.dp).border(2.dp, c.line, CircleShape).drawBehind {
      drawArc(c.ink, -90f, 90f, false, style = Stroke(2.dp.toPx()), topLeft = Offset(-1.dp.toPx(), -1.dp.toPx()), size = Size(size.width + 2.dp.toPx(), size.height + 2.dp.toPx()))
    }) else Glyph(glyph, tint = c.ink2)
    Column(Modifier.weight(1f)) {
      Text(
        text, style = Hub.type.body.copy(fontSize = 15.sp, fontWeight = if (working) FontWeight.SemiBold else FontWeight.Normal, color = c.ink),
        modifier = if (announce == null) Modifier else Modifier.semantics { contentDescription = announce; liveRegion = LiveRegionMode.Polite },
      )
      if (detail != null) Text(detail, style = Hub.type.mono.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(top = 4.dp))
    }
  }
}

/** A slow fade of the working ring, about 1.8 seconds a round. Still when the system has animations off. */
@Composable
private fun rememberPulse(): Float {
  val context = LocalContext.current
  if (!pulseAllowed(animatorDurationScale(context))) return 1f
  return rememberInfiniteTransition(label = "pairing pulse").animateFloat(
    1f, 0.35f, infiniteRepeatable(tween(900, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "pairing pulse alpha",
  ).value
}

/** The time as the wait sees it: read again each second, and again whenever the hub was asked. */
@Composable
private fun rememberNow(clock: Clock, running: Boolean, checkedAt: Instant?): State<Instant> =
  produceState(clock.instant(), clock, running, checkedAt) {
    value = clock.instant()
    while (running) {
      delay(1_000)
      value = clock.instant()
    }
  }

@Composable
private fun CheckString(s: PairStep.CheckString, fmt: Fmt, clock: Clock, a: PairActions) {
  val c = Hub.colors
  val h = s.handshake
  val needsYou = s.needsYou
  val wait = s.wait
  val now by rememberNow(clock, running = wait != null, checkedAt = wait?.checkedAt)
  val view = wait?.view(now)
  Column(Modifier.fillMaxSize()) {
    StepBar("Confirm on the hub", "1 of 2", a, a.onCancelPairing, backDescription = "Cancel pairing", close = true)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Column(Modifier.padding(horizontal = 16.dp)) {
        StepLine(G.Check, buildAnnotatedString {
          append("Reached ${h.hubName}")
          if (h.via != null) withStyle(SpanStyle(color = c.ink3)) { append(" · via ${h.via}") }
        }, done = true)
        if (!needsYou) StepLine(G.Check, plain(if (h.needsLocalConfirm) "You said the check strings match" else "The hub's key matches the code"), done = true)
        StepLine(
          G.Check,
          buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(if (needsYou) "Compare the check string" else "Waiting for you to confirm on the hub") } },
          done = false, working = true, pulse = !needsYou,
          announce = if (needsYou) null else view?.status ?: "Waiting for you to confirm on the hub",
          detail = view?.meta,
        )
      }
      Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 20.dp).fillMaxWidth().background(c.sunk, dev.agync.agenthub.ui.theme.ControlShape).padding(16.dp)) {
        Text("Check string", style = Hub.type.meta.copy(fontWeight = FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 8.dp))
        Text(h.checkString, style = Hub.type.sas.copy(color = c.ink), modifier = Modifier.semantics { contentDescription = "Check string: " + h.checkString.split(' ').joinToString(", ") { it.toCharArray().joinToString(" ") } })
      }
      if (needsYou) Text(
        "You typed the code, so nothing has checked the hub's key yet. Add a phone on the hub shows a check string too. Say they match only if the two are the same.",
        style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp),
      ) else Text(
        "On the hub, Settings, Phones shows the same check string. Confirm there if they match. This usually takes under a minute, and this screen moves on by itself.",
        style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp),
      )
      if (view != null && view.stillWaiting) Column(
        Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp).fillMaxWidth()
          .drawBehind { drawRect(c.accent, size = Size(3.dp.toPx(), size.height)) }.padding(start = 16.dp),
      ) {
        Text(
          buildAnnotatedString {
            withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("Still waiting.") }
            append(" If Add a phone on the hub shows no confirm button, reload that page.")
          },
          style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink),
        )
        Text("The pairing stays open until ${fmt.time(wait.deadline)}.", style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(top = 4.dp))
      }
      Text(
        "If they differ, refuse on the hub and cancel here. Something stands between this phone and the hub.",
        style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp),
      )
    }
    Foot {
      if (needsYou) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
          HubButton("They differ", a.onCheckDiffer, Modifier.weight(1f), size = ButtonSize.Large)
          HubButton("They match", a.onCheckMatch, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large)
        }
      } else HubButton("Cancel pairing", a.onCancelPairing, Modifier.fillMaxWidth(), size = ButtonSize.Large)
    }
  }
}

/** The hub confirmed: all three steps done, and a button for the impatient. The flow moves on by itself after a beat. */
@Composable
private fun Confirmed(s: PairStep.Confirmed, a: PairActions) {
  val c = Hub.colors
  val h = s.handshake
  Column(Modifier.fillMaxSize()) {
    StepBar("Confirm on the hub", "1 of 2", a, null, close = true)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      Column(Modifier.padding(horizontal = 16.dp)) {
        StepLine(G.Check, buildAnnotatedString {
          append("Reached ${h.hubName}")
          if (h.via != null) withStyle(SpanStyle(color = c.ink3)) { append(" · via ${h.via}") }
        }, done = true)
        StepLine(G.Check, plain(if (h.needsLocalConfirm) "You said the check strings match" else "The hub's key matches the code"), done = true)
        StepLine(
          G.Check, buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("Confirmed on the hub") } }, done = true,
          announce = "Confirmed on the hub. Paired.",
        )
      }
      Text(
        "Paired. Next, choose how alerts reach you.",
        style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp),
      )
    }
    Foot { HubButton("Continue", a.onContinue, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large) }
  }
}

/** The wait ended without a pairing. Nothing is left on the phone; the person starts over from a new code. */
@Composable
private fun Stopped(s: PairStep.Stopped, fmt: Fmt, a: PairActions) {
  val c = Hub.colors
  val body = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, color = c.ink2)
  Column(Modifier.fillMaxSize()) {
    StepBar("Pairing stopped", "1 of 2", a, a.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 16.dp)) {
      when (val r = s.reason) {
        StopReason.Refused -> {
          Text("The hub refused this phone.", style = Hub.type.body.copy(fontSize = 15.sp, lineHeight = 23.sp, fontWeight = FontWeight.SemiBold, color = c.ink))
          Text(
            "If you did not refuse it, the check strings differed: something may stand between this phone and the hub. Start again from a new code.",
            style = body, modifier = Modifier.padding(top = 8.dp),
          )
        }
        is StopReason.RanOut -> Text("The pairing ran out at ${fmt.time(r.at)} before it was confirmed on the hub.", style = body)
      }
    }
    Foot { HubButton("Start again", a.onStartAgain, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large) }
  }
}

@Composable
private fun Alerts(s: PairStep.Alerts, prefs: DevicePrefs, a: PairActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    SmallTopBar("Paired with ${s.hubName}", titleMuted = true, trailingText = "2 of 2")
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote("How should alerts reach you?", topPadding = 0.dp, paragraphs = listOf(plain("Your hub has no cloud to push through. Pick how this phone hears from it.")))
      Column(Modifier.padding(top = 16.dp).fillMaxWidth().drawBehind { drawLine(c.line, Offset(0f, 0.5f), Offset(size.width, 0.5f), 1.dp.toPx()) }) {
        Option("Stay connected to the hub", "Alerts arrive within seconds. Android shows a quiet Connected notification while it runs. Uses some battery.", s.stayConnected) { a.onStayConnected(true) }
        Option("Only when I open the app", "No alerts. Asks wait in Now.", !s.stayConnected) { a.onStayConnected(false) }
      }
      SectionLabel("Alert me for", Modifier.padding(top = 0.dp), topPadding = 28.dp)
      AlertSwitch("Questions and approvals", null, prefs.alertApprovals) { v -> a.onPrefs { it.copy(alertApprovals = v) } }
      AlertSwitch("A machine asking to join", null, prefs.alertJoining) { v -> a.onPrefs { it.copy(alertJoining = v) } }
      AlertSwitch("Finished work", "Silent, in the shade only", prefs.alertFinished) { v -> a.onPrefs { it.copy(alertFinished = v) } }
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Not now", a.onNotNow, Modifier.weight(1f), kind = ButtonKind.Text, size = ButtonSize.Large)
        HubButton("Allow notifications", a.onAllowNotifications, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large)
      }
    }
  }
}

@Composable
private fun AlertSwitch(title: String, detail: String?, checked: Boolean, onChange: (Boolean) -> Unit) {
  val c = Hub.colors
  Row(
    Modifier.fillMaxWidth().hairlineBottom(c.line).switchRow(checked, onChange).heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(16.dp),
  ) {
    Column(Modifier.weight(1f)) {
      Text(title, style = Hub.type.body.copy(fontWeight = FontWeight.Medium, color = c.ink))
      if (detail != null) Text(detail, style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
    }
    HubSwitch(checked, null)
  }
}

@Composable
private fun Option(title: String, body: String, selected: Boolean, onSelect: () -> Unit) {
  val c = Hub.colors
  Row(
    Modifier.fillMaxWidth().background(if (selected) c.sunk else Color.Transparent).hairlineBottom(c.line)
      .selectable(selected, role = Role.RadioButton, onClick = onSelect).padding(horizontal = 24.dp, vertical = 16.dp),
    horizontalArrangement = Arrangement.spacedBy(16.dp),
  ) {
    HubRadio(selected, Modifier.padding(top = 3.dp))
    Column(Modifier.weight(1f)) {
      Text(title, style = Hub.type.body.copy(fontWeight = FontWeight.Medium, color = c.ink))
      Text(body, style = Hub.type.meta.copy(lineHeight = 20.sp, color = c.ink2), modifier = Modifier.padding(top = 4.dp))
    }
  }
}

@Composable
private fun NotifyDenied(s: PairStep.Alerts, a: PairActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    SmallTopBar("Paired with ${s.hubName}", titleMuted = true, trailingText = "2 of 2")
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote(
        "Notifications are off", glyph = G.BellOff,
        paragraphs = listOf(
          plain("Android blocked notifications for Agent Hub. Asks still wait in Now, but nothing reaches the shade or the lock screen."),
          plain("You can turn them on later in Android's settings, or from Settings, Alerts in this app."),
        ),
      )
    }
    Foot {
      HubButton("Continue without alerts", a.onNotNow, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large)
      HubButton("Open Android settings", a.onOpenSettings, Modifier.fillMaxWidth(), size = ButtonSize.Large)
    }
  }
}

@Composable
private fun CameraDenied(a: PairActions) {
  Column(Modifier.fillMaxSize()) {
    StepBar("Scan the code", "1 of 2", a, a.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote(
        "The camera is off for Agent Hub", glyph = G.Camera,
        paragraphs = listOf(
          plain("Android has not let this app use the camera. Turn it on in Android's settings, under Permissions, or type the code instead."),
          plain("The camera is used only on this screen, to read the code."),
        ),
      )
    }
    Foot {
      HubButton("Open Android settings", a.onOpenSettings, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large)
      HubButton("Type the code", a.onStartManual, Modifier.fillMaxWidth(), size = ButtonSize.Large)
    }
  }
}

@Composable
private fun CodeFailed(s: PairStep.CodeFailed, a: PairActions) {
  val c = Hub.colors
  Column(Modifier.fillMaxSize()) {
    SmallTopBar("", onBack = a.onBack)
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      if (s.unreachable) {
        HeroNote("The hub did not answer", glyph = G.Warn, paragraphs = listOf(plain("Nothing answered at the address in the code. Check this phone is online and the hub is running, then try again.")))
      } else {
        HeroNote("That code did not work", glyph = G.Warn, paragraphs = listOf(plain("The hub does not say why, so a guessed code learns nothing. One of these is true:")))
        GlyphList(
          listOf(
            G.Check to withBold("", "It was used.", " Each code pairs one phone.", c.ink),
            G.Clock to withBold("", "It ran out.", " A code works for 15 minutes.", c.ink),
            G.Kb to withBold("", "It was mistyped.", " After 5 wrong tries the hub's open codes stop working.", c.ink),
          ),
          Modifier.padding(top = 12.dp),
        )
        Text("On the hub, open Add a phone again for a new code.", style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink2), modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 16.dp))
      }
    }
    Foot {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        HubButton("Type it again", a.onStartManual, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Scan a new code", a.onStartScan, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, glyph = G.Qr)
      }
    }
  }
}

/** The system's animation speed: 0 when the person has turned animations off. */
internal fun animatorDurationScale(context: android.content.Context): Float =
  android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f)

internal fun pulseAllowed(durationScale: Float): Boolean = durationScale > 0f
