package dev.agync.agenthub.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.SearchHit
import dev.agync.agenthub.model.SearchKind
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubChip
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.SectionLabel
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.ToolsRow
import dev.agync.agenthub.viewmodel.SearchState
import dev.agync.agenthub.viewmodel.SearchViewModel
import java.time.Instant

/**
 * Search over the hub: a field, a chip per kind, and the hits as flat rows under a label per kind.
 * It searches on the keyboard's search key. The matched words are bold, so colour carries nothing.
 */
@Composable
fun SearchScreen(vm: SearchViewModel, fmt: Fmt, now: Instant, onBack: () -> Unit, onOpen: (SearchHit) -> Unit, modifier: Modifier = Modifier) {
  val c = Hub.colors
  val query by vm.query.collectAsState()
  val kind by vm.kind.collectAsState()
  val state by vm.state.collectAsState()
  Column(modifier.fillMaxSize().background(c.bg)) {
    SmallTopBar("Search", onBack = onBack)
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
      HubField(
        query, vm::setQuery, Modifier.weight(1f), hint = "Search the hub", description = "Search",
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), onAction = vm::submit,
      )
      HubIconButton(G.Search, "Run the search", vm::submit)
    }
    val found = state as? SearchState.Found
    if (found != null && found.results.hits.isNotEmpty()) {
      ToolsRow {
        HubChip("All", kind == null, { vm.setKind(null) }, count = found.results.count(null))
        SearchKind.entries.filter { found.results.count(it) > 0 }.forEach { k -> HubChip(k.label, kind == k, { vm.setKind(k) }, count = found.results.count(k)) }
      }
    }
    when (val s = state) {
      SearchState.Idle -> Note("Search every feed, artifact, brain entry and knowledge base this hub keeps.")
      SearchState.Searching -> Note("Searching.")
      is SearchState.Failed -> Note("Cannot search now. The hub did not answer.")
      is SearchState.Found ->
        if (s.results.hits.isEmpty()) Note("No results for “${s.query}”.")
        else Results(s, kind, fmt, now, onOpen)
    }
  }
}

@Composable
private fun Note(text: String) {
  Text(text, style = Hub.type.body.copy(color = Hub.colors.ink2), modifier = Modifier.padding(horizontal = 16.dp, vertical = 20.dp))
}

@Composable
private fun Results(s: SearchState.Found, kind: SearchKind?, fmt: Fmt, now: Instant, onOpen: (SearchHit) -> Unit) {
  val words = s.query.split(Regex("\\s+")).filter { it.isNotEmpty() }
  val shown = if (kind == null) s.results.hits else s.results.hits.filter { it.kind == kind }
  LazyColumn(Modifier.fillMaxSize()) {
    SearchKind.entries.filter { k -> shown.any { it.kind == k } }.forEach { k ->
      val inKind = shown.filter { it.kind == k }
      item(key = "label-" + k.name) { SectionLabel(k.label, count = inKind.size) }
      items(inKind, key = { k.name + "-" + it.ref + "-" + it.title.hashCode() }) { hit -> HitRow(hit, words, fmt, now, onOpen) }
    }
    if (s.results.truncated) item(key = "truncated") { Note("These are the first results. Narrow the search to see the rest.") }
  }
}

@Composable
private fun HitRow(hit: SearchHit, words: List<String>, fmt: Fmt, now: Instant, onOpen: (SearchHit) -> Unit) {
  val c = Hub.colors
  val context = listOfNotNull(hit.project, hit.sessionName?.takeIf { hit.kind == SearchKind.Brain }, hit.by).joinToString(" · ")
  HubRow(
    glyph = { Glyph(glyphFor(hit.kind), tint = c.ink2) },
    trailing = hit.at?.let { fmt.stamp(it, now) },
    onClick = { onOpen(hit) },
    clickLabel = "Open ${hit.title}",
    head = { Text(marked(hit.title, words), style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink, fontWeight = FontWeight.Medium)) },
  ) {
    if (hit.snippet.isNotBlank() && hit.snippet != hit.title) {
      Text(marked(hit.snippet, words), maxLines = 3, style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.padding(top = 2.dp))
    }
    if (context.isNotEmpty()) Text(context, style = Hub.type.mono.copy(color = c.ink3), modifier = Modifier.padding(top = 4.dp))
  }
}

private fun glyphFor(kind: SearchKind): String =
  when (kind) {
    SearchKind.Feed -> G.Now
    SearchKind.Artifact -> G.Doc
    SearchKind.Brain -> G.Branch
    SearchKind.Knowledge -> G.Kb
  }

/** The text with each matched word in bold. Nothing is parsed: a snippet that carries markup stays text. */
internal fun marked(text: String, words: List<String>): AnnotatedString =
  buildAnnotatedString {
    if (words.isEmpty()) {
      append(text)
      return@buildAnnotatedString
    }
    val pattern = Regex(words.joinToString("|") { Regex.escape(it) }, RegexOption.IGNORE_CASE)
    var at = 0
    for (m in pattern.findAll(text)) {
      append(text.substring(at, m.range.first))
      withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(m.value) }
      at = m.range.last + 1
    }
    append(text.substring(at))
  }
