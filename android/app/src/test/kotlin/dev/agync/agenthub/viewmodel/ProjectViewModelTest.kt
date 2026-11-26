package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class ProjectViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  @Test fun `opening a project's feed marks its newest event seen on the hub`() {
    val rig = Fixtures.rig()
    val vm = ProjectViewModel(rig.repo, "home-lab")
    val feed = (vm.feed.state.value as Load.Ready).value
    assertEquals(feed.maxByOrNull { it.at }!!.id, rig.seeded.feedSeen["home-lab"])
  }

  @Test fun `a hub that cannot be reached for it does not stop the feed from showing`() {
    val rig = Fixtures.rig()
    rig.seeded.refuseSeen = true
    val vm = ProjectViewModel(rig.repo, "home-lab")
    assertTrue((vm.feed.state.value as Load.Ready).value.isNotEmpty())
    assertTrue(rig.seeded.feedSeen.isEmpty())
  }
}
