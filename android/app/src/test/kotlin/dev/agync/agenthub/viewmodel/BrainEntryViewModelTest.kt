package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Rule
import org.junit.Test

class BrainEntryViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  @Test fun `an entry that is there loads with its text`() {
    val vm = BrainEntryViewModel(Fixtures.rig().repo, "nightly", "/kv/upgrade-plan")
    val read = (vm.entry.state.value as Load.Ready).value
    assertEquals("/kv/upgrade-plan", read.entry!!.path)
  }

  @Test fun `an entry the hub does not have is a read with nothing in it`() {
    val vm = BrainEntryViewModel(Fixtures.rig().repo, "nightly", "/kv/gone")
    assertNull((vm.entry.state.value as Load.Ready).value.entry)
  }
}
