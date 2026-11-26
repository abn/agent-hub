package dev.agync.agenthub.ui.components

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton

/**
 * The scrim and the surface above it, in the tree so it is one surface to look at and to test.
 * Touches never reach what is under it, back dismisses, and the surface is announced as a pane.
 */
@Composable
private fun Scrim(onDismiss: () -> Unit, content: @Composable BoxScope.() -> Unit) {
  BackHandler(onBack = onDismiss)
  Box(Modifier.fillMaxSize()) {
    Box(
      Modifier.fillMaxSize().background(Hub.colors.scrim).clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClickLabel = "Dismiss", onClick = onDismiss),
    )
    content()
  }
}

/** A bottom sheet: 16 top radius, grab handle, the thumb inset below. */
@Composable
fun HubSheet(title: String?, onDismiss: () -> Unit, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
  val c = Hub.colors
  Scrim(onDismiss) {
    Column(
      modifier.align(Alignment.BottomCenter).fillMaxWidth().widthIn(max = 640.dp)
        .background(c.raised, RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp))
        .semantics { if (title != null) paneTitle = title }
        .pointerInput(Unit) {}
        .windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 16.dp),
    ) {
      Box(Modifier.align(Alignment.CenterHorizontally).padding(top = 12.dp, bottom = 8.dp).size(width = 32.dp, height = 4.dp).background(c.lineStrong, RoundedCornerShape(2.dp)))
      if (title != null) Text(title, style = Hub.type.section.copy(color = c.ink), modifier = Modifier.padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 4.dp))
      content()
    }
  }
}

/** A dialog: 12 radius, 24 padding, actions in a row. */
@Composable
fun HubDialog(
  title: String,
  body: String,
  onDismiss: () -> Unit,
  confirmLabel: String,
  onConfirm: () -> Unit,
  modifier: Modifier = Modifier,
  dismissLabel: String = "Cancel",
  destructive: Boolean = false,
) {
  val c = Hub.colors
  Scrim(onDismiss) {
    Column(
      modifier.align(Alignment.Center).padding(horizontal = 24.dp).fillMaxWidth().widthIn(max = 560.dp)
        .background(c.raised, RoundedCornerShape(12.dp)).semantics { paneTitle = title }.pointerInput(Unit) {}.padding(24.dp),
    ) {
      Text(title, style = Hub.type.title.copy(color = c.ink))
      Text(body, style = Hub.type.meta.copy(fontSize = 15.sp, color = c.ink2), modifier = Modifier.padding(top = 12.dp))
      Row(Modifier.fillMaxWidth().padding(top = 24.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        HubButton(dismissLabel, onDismiss, Modifier.weight(1f), size = dev.agync.agenthub.ui.theme.ButtonSize.Large)
        HubButton(confirmLabel, onConfirm, Modifier.weight(1f), kind = if (destructive) ButtonKind.DangerPrimary else ButtonKind.Primary, size = dev.agync.agenthub.ui.theme.ButtonSize.Large)
      }
    }
  }
}

/** A snackbar: ink on inverse, 52 high, one action. A live region so it is spoken. */
@Composable
fun HubSnackbar(message: String, modifier: Modifier = Modifier, actionLabel: String? = null, onAction: (() -> Unit)? = null) {
  val c = Hub.colors
  Row(
    modifier.padding(horizontal = 12.dp).fillMaxWidth().widthIn(max = 640.dp).heightIn(min = 52.dp)
      .background(c.ink, RoundedCornerShape(4.dp)).semantics { liveRegion = LiveRegionMode.Polite }
      .padding(start = 16.dp, top = 4.dp, bottom = 4.dp, end = 4.dp),
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Text(message, style = Hub.type.meta.copy(fontSize = 15.sp, color = c.inv), modifier = Modifier.weight(1f))
    if (actionLabel != null && onAction != null) {
      dev.agync.agenthub.ui.theme.TextAction(actionLabel, onAction, color = c.inv)
    }
  }
}

/** Shows [message] for a few seconds, then calls [onTimeout]. Reduced motion changes nothing: it does not animate. */
@Composable
fun TimedSnackbar(message: String, key: Any, onTimeout: () -> Unit, modifier: Modifier = Modifier, actionLabel: String? = null, onAction: (() -> Unit)? = null) {
  LaunchedEffect(key) {
    kotlinx.coroutines.delay(5_000)
    onTimeout()
  }
  HubSnackbar(message, modifier, actionLabel, onAction)
}
