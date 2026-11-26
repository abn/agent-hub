package dev.agync.agenthub.viewmodel

import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.MainDispatcherRule
import dev.agync.agenthub.model.SearchKind
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

class SearchViewModelTest {
  @get:Rule val main = MainDispatcherRule()

  @Test fun `a query finds hits and an empty one finds nothing to show`() {
    val vm = SearchViewModel(Fixtures.rig().repo)
    vm.setQuery("longhorn")
    vm.submit()
    val found = vm.state.value as SearchState.Found
    assertEquals("longhorn", found.query)
    assertTrue(found.results.hits.any { it.kind == SearchKind.Feed })
    vm.setQuery("  ")
    vm.submit()
    assertEquals(SearchState.Idle, vm.state.value)
  }

  @Test fun `a hub that cannot be reached says the search failed, and the query stays`() {
    val rig = Fixtures.rig()
    val vm = SearchViewModel(rig.repo)
    rig.seeded.reachable = false
    vm.setQuery("backup")
    vm.submit()
    assertEquals(SearchState.Failed("backup"), vm.state.value)
    assertEquals("backup", vm.query.value)
  }

  @Test fun `a new search clears the kind chosen for the last one`() {
    val vm = SearchViewModel(Fixtures.rig().repo)
    vm.setQuery("backup")
    vm.submit()
    vm.setKind(SearchKind.Artifact)
    vm.submit()
    assertEquals(null, vm.kind.value)
  }
}
