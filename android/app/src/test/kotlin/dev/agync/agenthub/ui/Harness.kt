package dev.agync.agenthub.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import dev.agync.agenthub.ui.theme.BottomNav
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.Hub
import dev.agync.agenthub.ui.theme.NavItem
import dev.agync.agenthub.ui.theme.HubTheme
import dev.agync.agenthub.ui.theme.ThemeMode

/**
 * A phone frame as the board draws it: 412 by 915, a 32 status band on top and the gesture
 * inset below. Robolectric supplies no window insets, so the bands are drawn here.
 */
@Composable
fun PhoneFrame(dark: Boolean, time: String = "14:32", fontScale: Float = 1f, nav: Int? = null, badge: Int = 3, content: @Composable () -> Unit) {
  HubTheme(if (dark) ThemeMode.Dark else ThemeMode.Light) {
    val density = LocalDensity.current
    CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale)) {
      Column(Modifier.size(412.dp, 915.dp).background(Hub.colors.bg)) {
        Row(Modifier.fillMaxWidth().height(32.dp).padding(start = 24.dp, end = 20.dp), verticalAlignment = Alignment.CenterVertically) {
          Text(time, style = Hub.type.meta.copy(color = Hub.colors.ink))
        }
        Box(Modifier.weight(1f).fillMaxWidth()) { content() }
        if (nav != null) BottomNav(listOf(NavItem("Now", G.Now, badge), NavItem("Projects", G.Folder), NavItem("Artifacts", G.Doc), NavItem("Hub", G.Hub)), nav, {})
        Box(Modifier.fillMaxWidth().height(20.dp), contentAlignment = Alignment.TopCenter) {
          Box(Modifier.width(108.dp).height(4.dp).background(Hub.colors.ink.copy(alpha = 0.55f), RoundedCornerShape(2.dp)))
        }
      }
    }
  }
}

@Composable
fun TabletFrame(dark: Boolean, content: @Composable () -> Unit) {
  HubTheme(if (dark) ThemeMode.Dark else ThemeMode.Light) {
    Column(Modifier.size(1280.dp, 800.dp).background(Hub.colors.bg)) {
      Row(Modifier.fillMaxWidth().height(32.dp).padding(start = 24.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("14:32", style = Hub.type.meta.copy(color = Hub.colors.ink))
      }
      Box(Modifier.weight(1f).fillMaxSize()) { content() }
    }
  }
}
