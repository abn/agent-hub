package dev.agync.agenthub.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.agync.agenthub.data.HubException
import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.data.PruneUndo
import dev.agync.agenthub.model.ArtifactSummary
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.model.Member
import dev.agync.agenthub.model.SessionState
import dev.agync.agenthub.model.SessionSummary
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

class ProjectsViewModel(val repo: HubRepository) : ViewModel() {
  val projects = Loader(viewModelScope, repo) { projects() }

  /** Needs-you and can-wait counts by project, from Now's asks: no endpoint of their own. */
  fun waitingByProject(state: dev.agync.agenthub.data.HubState, now: java.time.Instant): Map<String, Pair<Int, Int>> {
    val asks = state.snapshot?.asks.orEmpty().filter { it.waiting && it.project != null }
    val queued = state.outbox.map { it.askId }.toSet()
    return asks.groupBy { it.project!! }.mapValues { (_, list) ->
      val open = list.filter { it.id !in queued }
      val wait = open.count { state.snoozes[it.id]?.isAfter(now) == true }
      (open.size - wait) to wait
    }
  }
}

class ProjectViewModel(val repo: HubRepository, val project: String) : ViewModel() {
  /** Opening a feed moves the hub's cursor to its newest event. A hub that will not say so does not stop the feed. */
  val feed =
    Loader(viewModelScope, repo) {
      feed(project).also { events ->
        events.maxByOrNull { it.at }?.let { newest -> try { markFeedSeen(project, newest.id) } catch (_: HubException.NotAllowed) { } catch (_: HubException.Unreachable) { } }
      }
    }
  val sessions = Loader(viewModelScope, repo) { sessions(project) }
  val artifacts = Loader(viewModelScope, repo) { artifacts() }
  val summary = Loader(viewModelScope, repo) { projects().firstOrNull { it.name == project } }

  val sessionFilter = MutableStateFlow(SessionFilter.All)

  fun filtered(all: List<SessionSummary>, f: SessionFilter): List<SessionSummary> =
    when (f) {
      SessionFilter.All -> all
      SessionFilter.Live -> all.filter { it.state == SessionState.Live }
      SessionFilter.Ended -> all.filter { it.state == SessionState.Ended }
    }

  /** Sessions in tree order: each parent followed by its children. */
  fun tree(list: List<SessionSummary>): List<SessionSummary> {
    val roots = list.filter { it.parent == null }.sortedByDescending { it.lastActivity }
    return roots.flatMap { r -> listOf(r) + list.filter { it.parent == r.name }.sortedByDescending { it.lastActivity } } +
      list.filter { s -> s.parent != null && list.none { it.name == s.parent } }
  }
}

enum class SessionFilter { Live, All, Ended }

class SessionViewModel(val repo: HubRepository, val sessionId: String) : ViewModel() {
  val session = Loader(viewModelScope, repo) { session(sessionId) }

  /** What pruning this session can be undone with, while the hub still holds it. */
  val undo = MutableStateFlow<PruneUndo?>(null)

  fun prune() {
    viewModelScope.launch { undo.value = repo.guarded { pruneSession(sessionId) }?.takeIf { it.tokens.isNotEmpty() } }
  }

  fun undoPrune() {
    val u = undo.value ?: return
    undo.value = null
    viewModelScope.launch {
      repo.guarded { undoPrune(u) }
      session.reload()
    }
  }

  fun end() {
    viewModelScope.launch {
      repo.guarded { endSession(sessionId) }
      session.reload()
    }
  }
}

class ArtifactsViewModel(val repo: HubRepository) : ViewModel() {
  val artifacts = Loader(viewModelScope, repo) { artifacts() }
  val filter = MutableStateFlow(ArtifactFilter.All)

  fun filtered(all: List<ArtifactSummary>, f: ArtifactFilter): List<ArtifactSummary> =
    when (f) {
      ArtifactFilter.All -> all
      ArtifactFilter.Live -> all.filter { it.live }
      ArtifactFilter.Protected -> all.filter { it.protected }
    }
}

enum class ArtifactFilter { All, Live, Protected }

class ViewerViewModel(val repo: HubRepository, val artifactId: String) : ViewModel() {
  private val shown = MutableStateFlow<Int?>(null)
  val view = Loader(viewModelScope, repo) { artifact(artifactId, shown.value) }

  /** The bar says a newer version was published; Show it. */
  fun showLatest() {
    val latest = (view.state.value as? Load.Ready)?.value?.latestVersion ?: return
    shown.value = latest
    view.reload()
  }
}

class HubViewModel(val repo: HubRepository) : ViewModel() {
  val members = Loader(viewModelScope, repo) { members() }
  val storage = Loader(viewModelScope, repo) { storage() }

  /** What the last prune can be undone with, while the hub still holds it. */
  val undo = MutableStateFlow<PruneUndo?>(null)

  fun prune() {
    viewModelScope.launch {
      undo.value = repo.guarded { pruneEnded() }?.takeIf { it.tokens.isNotEmpty() }
      storage.reload()
    }
  }

  fun undoPrune() {
    val u = undo.value ?: return
    undo.value = null
    viewModelScope.launch {
      repo.guarded { undoPrune(u) }
      storage.reload()
    }
  }
}

class MemberViewModel(repo: HubRepository, val name: String) : ViewModel() {
  val members = Loader(viewModelScope, repo) { members() }

  fun find(all: List<Member>): Member? = all.firstOrNull { it.name == name }
}

/** The hub's answer to one brain read: the entry, or nothing there. */
class BrainRead(val entry: dev.agync.agenthub.model.BrainEntry?)

class BrainEntryViewModel(val repo: HubRepository, val session: String, val path: String) : ViewModel() {
  val entry = Loader(viewModelScope, repo) { BrainRead(brainEntry(session, path)) }
}

sealed interface SearchState {
  /** Nothing asked yet. */
  data object Idle : SearchState

  data object Searching : SearchState

  data class Found(val query: String, val results: dev.agync.agenthub.model.SearchResults) : SearchState

  /** The hub did not answer. */
  data class Failed(val query: String) : SearchState
}

class SearchViewModel(val repo: HubRepository) : ViewModel() {
  val query = MutableStateFlow("")
  val kind = MutableStateFlow<dev.agync.agenthub.model.SearchKind?>(null)
  private val _state = MutableStateFlow<SearchState>(SearchState.Idle)
  val state: StateFlow<SearchState> = _state

  fun setQuery(q: String) {
    query.value = q
  }

  fun setKind(k: dev.agync.agenthub.model.SearchKind?) {
    kind.value = k
  }

  /** Runs the search on the keyboard's search key. An empty query clears the results. */
  fun submit() {
    val q = query.value.trim()
    if (q.isEmpty()) {
      _state.value = SearchState.Idle
      return
    }
    _state.value = SearchState.Searching
    kind.value = null
    viewModelScope.launch {
      val r = repo.guarded { search(q) }
      _state.value = if (r == null) SearchState.Failed(q) else SearchState.Found(q, r)
    }
  }
}
