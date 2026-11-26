package dev.agync.agenthub.share

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.AgentHubApp
import dev.agync.agenthub.ui.components.HubSheet
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubChip
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubTheme
import kotlinx.coroutines.launch

/** Share into a project: captures an idea, and only an idea. Notes and artifacts as "you" are not in v1. */
class ShareActivity : ComponentActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    val text = intent.getStringExtra(Intent.EXTRA_TEXT).orEmpty()
    val title = intent.getStringExtra(Intent.EXTRA_SUBJECT) ?: intent.getStringExtra(Intent.EXTRA_TITLE)
    val app = application as AgentHubApp
    val hub = app.container.registry.state.value.current
    setContent {
      HubTheme {
        if (hub == null) {
          HubSheet("Pair a hub first", { finish() }) {
            Text("Open Agent Hub and pair with a hub, then share again.", style = Hub.type.body.copy(color = Hub.colors.ink2), modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp))
          }
        } else {
          val repo = app.container.repository(hub)
          ShareSheet(
            hub.name, text, title, load = { repo.guarded { projects().map { it.name } }.orEmpty() },
            // guarded gives null when the hub could not be reached: the sheet stays open and says so.
            onCapture = { project, line -> repo.guarded { captureIdea(project, line, text.takeIf { it.startsWith("http") }) } != null },
            onDone = { finish() }, onCancel = { finish() },
          )
        }
      }
    }
  }
}

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
internal fun ShareSheet(
  hubName: String, text: String, title: String?, load: suspend () -> List<String>,
  onCapture: suspend (String, String) -> Boolean, onDone: () -> Unit, onCancel: () -> Unit,
) {
  val c = Hub.colors
  var chosen by remember { mutableStateOf<String?>(null) }
  var line by remember { mutableStateOf("") }
  var projects by remember { mutableStateOf<List<String>>(emptyList()) }
  var failed by remember { mutableStateOf(false) }
  val scope = rememberCoroutineScope()
  LaunchedEffect(Unit) { projects = load(); chosen = projects.firstOrNull() }
  HubSheet("Capture an idea in $hubName", onCancel) {
    Column(Modifier.padding(horizontal = 24.dp)) {
      Row(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Glyph(G.Link, tint = c.ink2)
        Column {
          Text(title ?: text, style = Hub.type.body.copy(fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.ink))
          if (title != null) Text(text, style = Hub.type.mono.copy(color = c.ink2))
        }
      }
      Row(Modifier.padding(bottom = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
        Glyph(G.Idea, size = 18.dp, tint = c.ink2)
        Text("Saved as an idea. It waits in Needs you until you resolve or clear it; agents can pick it up.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
      }
      Text("Project", style = Hub.type.meta.copy(fontWeight = FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 6.dp))
      // Every project, wrapping: a person with five projects is not offered three.
      androidx.compose.foundation.layout.FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        projects.forEach { p -> HubChip(p, chosen == p, { chosen = p }) }
      }
      HubField(line, { line = it }, Modifier.fillMaxWidth().padding(top = 12.dp), hint = "Add a line for the agents", description = "A line for the agents")
      if (failed) Text("Could not save the idea. The hub did not answer; try again.", style = Hub.type.meta.copy(color = c.ink), modifier = Modifier.padding(top = 12.dp))
      Row(Modifier.fillMaxWidth().padding(top = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        HubButton("Cancel", onCancel, Modifier.weight(1f), size = ButtonSize.Large)
        HubButton("Capture", { chosen?.let { p -> scope.launch { if (onCapture(p, line.ifBlank { title ?: text })) onDone() else failed = true } } }, Modifier.weight(1f), kind = ButtonKind.Primary, size = ButtonSize.Large, enabled = chosen != null)
      }
    }
  }
}
