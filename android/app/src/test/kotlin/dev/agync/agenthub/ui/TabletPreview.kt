package dev.agync.agenthub.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.model.Fmt
import dev.agync.agenthub.model.NowFilter
import dev.agync.agenthub.ui.screens.ItemActions
import dev.agync.agenthub.ui.screens.ItemScreen
import dev.agync.agenthub.ui.screens.NowActions
import dev.agync.agenthub.ui.screens.NowScreen
import dev.agync.agenthub.ui.screens.TabletListHeader
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.NavItem
import dev.agync.agenthub.ui.theme.NavRail
import dev.agync.agenthub.ui.theme.hairlineRight
import dev.agync.agenthub.viewmodel.ItemViewModel
import dev.agync.agenthub.viewmodel.NowViewModel

/** The tablet's list-detail as the shell composes it: a rail, the list, the selected ask. */
@Composable
fun TabletPreview(rig: Fixtures.Rig, fmt: Fmt) {
  val vm = NowViewModel(rig.repo, fmt, rig.clock).also { it.setFilter(NowFilter.NeedsYou) }
  val ui = vm.ui.value
  val first = ui.model.needsYou.first().id
  Row(Modifier.fillMaxSize()) {
    Box(Modifier.width(96.dp).fillMaxHeight()) {
      NavRail(listOf(NavItem("Now", G.Now, ui.model.badge), NavItem("Projects", G.Folder), NavItem("Artifacts", G.Doc), NavItem("Hub", G.Hub)), 0, {})
    }
    androidx.compose.foundation.layout.Column(Modifier.width(420.dp).fillMaxHeight().hairlineRight(Hub.colors.line)) {
      TabletListHeader(ui.hubName, {}, {})
      NowScreen(ui, fmt, NowActions({}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}), selectedAskId = first, bar = false)
    }
    androidx.compose.foundation.layout.Box(Modifier.weight(1f).fillMaxHeight()) {
      ItemScreen(ItemViewModel(rig.repo, first, fmt, rig.clock).ui.value, fmt, Fixtures.now, ItemActions({}, {}, {}, {}, {}, { _, _, _ -> }, {}, {}, {}, {}, {}, {}, {}), wide = true)
    }
  }
}
