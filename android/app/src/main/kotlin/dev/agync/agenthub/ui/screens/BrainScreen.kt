package dev.agync.agenthub.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.TextAction
import dev.agync.agenthub.viewmodel.BrainEntryViewModel
import dev.agync.agenthub.viewmodel.Load

/** One entry of a session's brain: its path, its size, and its text exactly as the hub holds it. */
@Composable
fun BrainEntryScreen(vm: BrainEntryViewModel, onBack: () -> Unit, onCopy: (String) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  val load by vm.entry.state.collectAsState()
  val e = ((load as? Load.Ready)?.value)?.entry
  Column(modifier.fillMaxSize().background(c.bg)) {
    SmallTopBar("Brain", subtitle = vm.session, onBack = onBack)
    when {
      load is Load.Loading -> Unit
      load is Load.Unreachable -> {
        Text("Cannot read this entry now. The hub did not answer.", style = Hub.type.body.copy(color = c.ink2), modifier = Modifier.padding(24.dp))
        Row(Modifier.padding(start = 12.dp)) { TextAction("Try again", vm.entry::reload) }
      }
      e == null -> Text("This entry is no longer on the hub. A session's brain is cleared when it is pruned.", style = Hub.type.body.copy(color = c.ink2), modifier = Modifier.padding(24.dp))
      else -> Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
        Row(Modifier.padding(start = 16.dp, end = 4.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
          Text("${e.path} \u00B7 ${Fmt.bytes(e.sizeBytes)}", style = Hub.type.mono.copy(color = c.ink2), modifier = Modifier.weight(1f))
          HubIconButton(G.Copy, "Copy the entry", { onCopy(e.content) }, glyphSize = 20.dp)
        }
        SectionLabel("Value", topPadding = 8.dp)
        Text(
          e.content,
          style = Hub.type.mono.copy(fontSize = 13.sp, lineHeight = 21.sp, color = c.ink),
          modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        )
      }
    }
  }
}
