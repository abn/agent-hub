package dev.agync.agenthub.nav

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

sealed interface Route {
  /** A top-level destination of the bottom bar or rail. */
  sealed interface Tab : Route

  data object Now : Tab

  data object Projects : Tab

  data object Artifacts : Tab

  data object Hub : Tab

  data class Item(val askId: String) : Route

  data class Project(val name: String, val tab: Int = 0) : Route

  data class Session(val id: String) : Route

  data class Viewer(val artifactId: String) : Route

  /** One entry of a session's brain, read from an ask's receipts. */
  /** Search over the hub. */
  data object Search : Route

  data class Brain(val session: String, val path: String) : Route

  data class Member(val name: String) : Route

  data object Settings : Route

  data object Storage : Route

  data object Phones : Route

  data object PairAnother : Route

  /** Whether the bottom bar shows on this route. */
  val showsNav: Boolean
    get() = when (this) {
      is Tab, is Project, is Session, is Member -> true
      else -> false
    }
}

/** The back stack. A tab switch starts a fresh stack at that tab; back pops until the root. */
class Navigator(start: Route.Tab = Route.Now) {
  private val _stack = MutableStateFlow<List<Route>>(listOf(start))
  val stack: StateFlow<List<Route>> = _stack

  val current: Route get() = _stack.value.last()

  val tab: Route.Tab get() = _stack.value.first() as Route.Tab

  fun push(route: Route) {
    if (current != route) _stack.value = _stack.value + route
  }

  /** Replaces the top of the stack, as when an ask opens the next one. */
  fun replace(route: Route) {
    _stack.value = _stack.value.dropLast(1) + route
  }

  /** Returns false when already at the root, so the system can take the back press. */
  fun pop(): Boolean {
    if (_stack.value.size <= 1) return false
    _stack.value = _stack.value.dropLast(1)
    return true
  }

  fun selectTab(tab: Route.Tab) {
    _stack.value = listOf(tab)
  }

  /** Opens a deep link, as a notification does, on top of Now. */
  fun openFromOutside(route: Route) {
    _stack.value = listOf(Route.Now, route)
  }
}
