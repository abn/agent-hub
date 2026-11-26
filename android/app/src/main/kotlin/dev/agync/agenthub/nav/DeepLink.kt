package dev.agync.agenthub.nav

import dev.agync.agenthub.data.HubRegistry

/** Where a notification sends the person: to its hub, and to its ask when it names one. */
object DeepLink {
  fun open(askId: String?, hubId: String?, registry: HubRegistry, navigator: Navigator) {
    // The hub first: an ask is read on the hub that asked, not on whichever one was showing.
    if (hubId != null && registry.state.value.hubs.any { it.id == hubId }) registry.select(hubId)
    // A finished-work notification names no ask: it opens Now, not an item with no id.
    val ask = askId?.takeIf { it.isNotBlank() } ?: return
    navigator.openFromOutside(Route.Item(ask))
  }
}
