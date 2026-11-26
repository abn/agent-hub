package dev.agync.agenthub.ui

import androidx.activity.ComponentActivity
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.ExperimentalTestApi
import androidx.compose.ui.test.getBoundsInRoot
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.down
import androidx.compose.ui.test.moveBy
import androidx.compose.ui.test.center
import androidx.compose.ui.unit.dp
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.ui.screens.NowActions
import dev.agync.agenthub.ui.screens.NowScreen
import dev.agync.agenthub.ui.theme.BottomNav
import dev.agync.agenthub.ui.theme.G
import dev.agync.agenthub.ui.theme.NavItem
import dev.agync.agenthub.viewmodel.NowViewModel
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** At 200% text nothing in the bottom bar or the Now list lands on something else. */
@OptIn(ExperimentalTestApi::class)
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w412dp-h915dp-xhdpi")
class LargeTextTest {
  private val compose = createAndroidComposeRule<ComponentActivity>()

  @get:Rule val chain: RuleChain = RuleChain.outerRule(MainDispatcherRule()).around(compose)

  private val items = listOf(NavItem("Now", G.Now, 5), NavItem("Projects", G.Folder), NavItem("Artifacts", G.Doc), NavItem("Hub", G.Hub))

  @Test fun `at 200 percent the count sits clear of its icon and no two labels touch`() {
    compose.setContent { PhoneFrame(dark = true, fontScale = 2f, nav = 0, badge = 5) { } }
    val glyph = compose.onNodeWithTag("nav-glyph-0", useUnmergedTree = true).getBoundsInRoot()
    val badge = compose.onNodeWithTag("nav-badge-0", useUnmergedTree = true).getBoundsInRoot()
    assertTrue("the count ($badge) overlaps its icon ($glyph)", badge.left >= glyph.right || badge.bottom <= glyph.top)
    val labels = items.map { compose.onNodeWithText(it.label, useUnmergedTree = true).getBoundsInRoot() }
    for ((a, b) in labels.zipWithNext()) assertTrue("labels $a and $b touch", a.right < b.left)
  }

  @Test fun `bar labels follow the font to 130 percent and stop, above the 12sp floor`() {
    compose.setContent { PhoneFrame(dark = true, fontScale = 2f, nav = 0, badge = 5) { } }
    val label = compose.onNodeWithText("Projects", useUnmergedTree = true).getBoundsInRoot()
    assertTrue("a label line is ${(label.bottom - label.top)} high: not above the 12sp floor", (label.bottom - label.top) >= 14.dp)
    assertTrue("a label line is ${(label.bottom - label.top)} high: it kept growing past 130%", (label.bottom - label.top) <= 24.dp)
  }

  @Test fun `the bar grows to hold its labels instead of clipping them`() {
    compose.setContent { PhoneFrame(dark = false, fontScale = 2f, nav = 0, badge = 5) { } }
    val bar = compose.onNodeWithText("Hub", useUnmergedTree = true).getBoundsInRoot()
    assertTrue("the label ends at ${bar.bottom}, below the visible frame", bar.bottom <= 915.dp - 20.dp)
  }

  @Test fun `pulling Now down makes room for the indicator instead of covering the chips`() {
    val rig = Fixtures.rig()
    runBlocking { rig.repo.refresh() }
    val vm = NowViewModel(rig.repo, Fixtures.fmt(), rig.clock)
    compose.setContent {
      PhoneFrame(dark = true) {
        NowScreen(vm.ui.value, Fixtures.fmt(), NowActions({}, { _, _ -> }, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}))
      }
    }
    compose.waitForIdle()
    val before = compose.onNodeWithText("Needs you", substring = true).getBoundsInRoot().top
    compose.onNodeWithText("Needs you", substring = true).performTouchInput {
      down(center)
      moveBy(Offset(0f, 600f))
    }
    compose.waitForIdle()
    val after = compose.onNodeWithText("Needs you", substring = true).getBoundsInRoot().top
    assertTrue("the chips stayed at $before while pulled, so the indicator covers them", after - before >= 40.dp)
  }
}
