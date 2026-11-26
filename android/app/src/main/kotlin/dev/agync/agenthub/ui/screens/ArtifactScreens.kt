package dev.agync.agenthub.ui.screens

import android.annotation.SuppressLint
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalInspectionMode
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.ui.components.HeroNote
import dev.agync.agenthub.ui.components.HubRow
import dev.agync.agenthub.ui.components.HubSheet
import dev.agync.agenthub.ui.components.SettingsRow
import dev.agync.agenthub.ui.theme.ButtonKind
import dev.agync.agenthub.ui.theme.ButtonSize
import dev.agync.agenthub.ui.theme.Dot
import dev.agync.agenthub.ui.theme.Foot
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Glyph
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.HubButton
import dev.agync.agenthub.ui.theme.HubChip
import dev.agync.agenthub.ui.theme.HubField
import dev.agync.agenthub.ui.theme.HubIconButton
import dev.agync.agenthub.ui.theme.HubSwitch
import dev.agync.agenthub.ui.theme.SmallTopBar
import dev.agync.agenthub.ui.theme.StateWord
import dev.agync.agenthub.ui.theme.TextAction
import dev.agync.agenthub.ui.theme.ToolsRow
import dev.agync.agenthub.ui.theme.hairlineBottom
import dev.agync.agenthub.ui.theme.hairlineTop
import dev.agync.agenthub.ui.theme.switchRow
import dev.agync.agenthub.viewmodel.ArtifactFilter
import dev.agync.agenthub.viewmodel.ArtifactsViewModel
import dev.agync.agenthub.viewmodel.Load
import dev.agync.agenthub.viewmodel.ViewerViewModel
import java.time.Instant

@Composable
fun ArtifactRow(a: ArtifactSummary, fmt: Fmt, now: Instant, onClick: () -> Unit) {
  val c = Hub.colors
  HubRow(
    glyph = { Glyph(if (a.protected) G.Lock else G.Doc, tint = c.ink2) },
    trailing = fmt.stamp(a.at, now), onClick = onClick, clickLabel = "Open ${a.name}",
    head = { Text(a.name, style = Hub.type.bodyStrong.copy(color = c.ink)) },
  ) {
    Row(Modifier.padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
      val base = buildList {
        add(a.project); add(a.tool)
        if (a.protected) add("protected") else if (!a.live) add("v${a.version}")
      }.joinToString(" · ")
      Text("$base ·", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2))
      if (a.live) { StateWord("live", Dot.Live); Text("·", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2)) }
      Text(Fmt.bytes(a.bytes) + if (a.comments > 0) " · ${a.comments} comments" else "", style = Hub.type.mono.copy(color = c.ink2))
    }
  }
}

@Composable
fun ArtifactsScreen(
  vm: ArtifactsViewModel, fmt: Fmt, now: Instant, hubName: String, onSwitchHub: () -> Unit, onOpen: (ArtifactSummary) -> Unit,
  onSearch: () -> Unit, onFilter: () -> Unit, onRetry: () -> Unit, modifier: Modifier = Modifier,
) {
  val c = Hub.colors
  val load by vm.artifacts.state.collectAsState()
  val filter by vm.filter.collectAsState()
  val all = (load as? Load.Ready)?.value.orEmpty()
  val today = all.count { java.time.Duration.between(it.at, now).toHours() < 24 }
  Column(modifier.fillMaxSize().background(c.bg)) {
    TopLevelHeader("Artifacts", if (load is Load.Ready) "${all.size} artifacts · $today changed today" else "", hubName, onSwitchHub) {
      HubIconButton(G.Search, "Search", onSearch)
      HubIconButton(G.Filter, "Filter", onFilter)
    }
    ToolsRow {
      HubChip("All", filter == ArtifactFilter.All, { vm.filter.value = ArtifactFilter.All }, count = all.size)
      HubChip("Live", filter == ArtifactFilter.Live, { vm.filter.value = ArtifactFilter.Live }, count = all.count { it.live })
      HubChip("Protected", filter == ArtifactFilter.Protected, { vm.filter.value = ArtifactFilter.Protected }, count = all.count { it.protected })
    }
    when (load) {
      Load.Loading -> Unit
      Load.Unreachable -> RetryNote("Artifacts could not be loaded.", onRetry)
      is Load.Ready -> LazyColumn(Modifier.weight(1f)) { items(vm.filtered(all, filter), key = { it.id }) { a -> ArtifactRow(a, fmt, now) { onOpen(a) } } }
    }
  }
}

/**
 * The viewer. The artifact renders in a sandboxed web view and keeps its own look; the app adds
 * the version bar and the comment sheet around it. Protected artifacts ask for the project
 * password first (or the fingerprint that releases the remembered one).
 */
@Composable
fun ViewerScreen(
  vm: ViewerViewModel,
  fmt: Fmt,
  now: Instant,
  gate: ProtectedGate,
  hubId: String,
  onBack: () -> Unit,
  onMore: () -> Unit,
  onUnreachable: () -> Unit = {},
  modifier: Modifier = Modifier,
) {
  val c = Hub.colors
  val load by vm.view.state.collectAsState()
  var comments by rememberSaveable { mutableStateOf(false) }
  Column(modifier.fillMaxSize().background(c.bg)) {
    val v = (load as? Load.Ready)?.value
    val name = v?.summary?.name.orEmpty()
    val sub = v?.let { "${it.summary.project} · " + if (it.sealed != null) "protected" else "v${it.shownVersion} of ${it.latestVersion} · ${it.summary.tool}" }
    SmallTopBar(name, subtitle = sub, onBack = onBack) {
      if (v != null && v.sealed == null) HubIconButton(G.Comments, "Comments, ${v.comments.size}", { comments = !comments })
    }
    when {
      load == Load.Unreachable -> RetryNote("This artifact could not be loaded.") { vm.view.reload(); onUnreachable() }
      v == null -> Unit
      v.sealed != null -> ProtectedBody(v, gate, hubId)
      else -> {
        if (v.latestVersion > v.shownVersion) Row(Modifier.fillMaxWidth().hairlineTop(c.line).hairlineBottom(c.line).background(c.sunk).heightIn(min = 48.dp).padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
          Glyph(G.Refresh, size = 20.dp, tint = c.ink2)
          Text("v${v.latestVersion} published at ${fmt.time(v.latestAt)}", style = Hub.type.meta.copy(color = c.ink2), modifier = Modifier.weight(1f).padding(start = 8.dp))
          TextAction("Show v${v.latestVersion}", vm::showLatest)
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(c.raised)) { SandboxedPage(v.html) }
        if (comments) CommentsPanel(v, fmt) { comments = false }
      }
    }
  }
}

@Composable
private fun CommentsPanel(v: ArtifactView, fmt: Fmt, onClose: () -> Unit) {
  val c = Hub.colors
  HubSheet(null, onClose) {
    v.comments.forEach { cm ->
      Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text("${cm.by} · ${fmt.time(cm.at)}", style = Hub.type.mono.copy(color = c.ink2))
        Text(cm.text, style = Hub.type.body.copy(fontSize = 15.sp, color = c.ink), modifier = Modifier.padding(top = 4.dp))
      }
    }
    Text("Reply and resolve in the web app for now.", style = Hub.type.meta.copy(fontSize = 13.sp, color = c.ink2), modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 8.dp))
  }
}

/** The page in a web view that can load nothing from the network and exposes nothing of the app. */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun SandboxedPage(html: String, modifier: Modifier = Modifier) {
  if (LocalInspectionMode.current) {
    Box(modifier.fillMaxSize())
    return
  }
  AndroidView(
    modifier = modifier.fillMaxSize(),
    factory = { ctx ->
      WebView(ctx).apply {
        settings.javaScriptEnabled = true
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        settings.blockNetworkLoads = true
        settings.setSupportMultipleWindows(false)
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        settings.domStorageEnabled = false
        setBackgroundColor(android.graphics.Color.TRANSPARENT)
        webViewClient = object : WebViewClient() {
          override fun shouldOverrideUrlLoading(view: WebView?, request: WebResourceRequest?): Boolean = true
        }
      }
    },
    update = { loadIfChanged(it, html) },
  )
}

/** Loads [html] only when it differs from what the view shows: a recomposition must not reload the page and lose its scroll. */
internal fun loadIfChanged(view: WebView, html: String): Boolean {
  if (view.tag == html) return false
  view.tag = html
  view.loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
  return true
}

/** What the locked screen needs from outside: the stored password and the fingerprint prompt. */
interface ProtectedGate {
  /** The remembered password for [project], released by the fingerprint, or null. */
  suspend fun remembered(hubId: String, project: String): String?

  suspend fun remember(hubId: String, project: String, password: String)

  /** Opens the protected artifact with [password]. Returns the page, or null when the password is wrong. */
  suspend fun open(view: ArtifactView, password: String): String?
}

@Composable
private fun ProtectedBody(v: ArtifactView, gate: ProtectedGate, hubId: String) {
  val c = Hub.colors
  var password by rememberSaveable { mutableStateOf("") }
  var show by rememberSaveable { mutableStateOf(false) }
  var remember by rememberSaveable { mutableStateOf(true) }
  var html by androidx.compose.runtime.remember { mutableStateOf<String?>(null) }
  var wrong by rememberSaveable { mutableStateOf(false) }
  var working by rememberSaveable { mutableStateOf(false) }
  val scope = androidx.compose.runtime.rememberCoroutineScope()
  val project = v.summary.project
  androidx.compose.runtime.LaunchedEffect(v.summary.id) {
    gate.remembered(hubId, project)?.let { pw -> html = withContext(Dispatchers.Default) { gate.open(v, pw) } }
  }
  val page = html
  if (page != null) {
    Box(Modifier.fillMaxSize().background(c.raised)) { SandboxedPage(page) }
    return
  }
  Column(Modifier.fillMaxSize().hairlineTop(c.line)) {
    Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
      HeroNote("This artifact is encrypted", glyph = G.Lock, topPadding = 32.dp, paragraphs = listOf(androidx.compose.ui.text.AnnotatedString("The hub holds only ciphertext. Enter the $project password to decrypt it on this phone.")))
      Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 24.dp)) {
        Text("Password", style = Hub.type.meta.copy(fontWeight = androidx.compose.ui.text.font.FontWeight.Medium, color = c.ink2), modifier = Modifier.padding(bottom = 6.dp))
        HubField(
          password, { password = it; wrong = false }, Modifier.fillMaxWidth(), description = "Password",
          secure = true, revealed = show,
          keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(keyboardType = androidx.compose.ui.text.input.KeyboardType.Password),
          trailing = { HubIconButton(if (show) G.EyeOff else G.Eye, if (show) "Hide the password" else "Show the password", { show = !show }) },
        )
        if (wrong) Text("That password did not open it.", style = Hub.type.meta.copy(color = c.ink), modifier = Modifier.padding(top = 6.dp))
      }
      SettingsRow(
        G.Finger, "Remember for $project on this phone", Modifier.padding(top = 16.dp).hairlineTop(c.line).switchRow(remember) { remember = it },
        detail = androidx.compose.ui.text.AnnotatedString("Kept in this phone's keystore behind your fingerprint, never sent to the hub. Opens every protected artifact in $project, as the web app does."),
        end = { HubSwitch(remember, null) },
      )
    }
    Foot {
      HubButton("Decrypt", {
        working = true
        scope.launch {
          // Key stretching takes long enough to freeze the screen on the main thread.
          val opened = withContext(Dispatchers.Default) { gate.open(v, password) }
          working = false
          if (opened == null) wrong = true else { if (remember) gate.remember(hubId, project, password); html = opened }
        }
      }, Modifier.fillMaxWidth(), kind = ButtonKind.Primary, size = ButtonSize.Large, enabled = password.isNotEmpty() && !working)
    }
  }
}
