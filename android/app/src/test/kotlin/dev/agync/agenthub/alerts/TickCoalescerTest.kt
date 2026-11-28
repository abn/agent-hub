package dev.agync.agenthub.alerts

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.currentTime
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class TickCoalescerTest {
  private class Rig(scope: TestScope, foreground: Boolean) {
    val ticks = MutableSharedFlow<Unit>(extraBufferCapacity = 256)
    val foreground = MutableStateFlow(foreground)
    val refreshedAt = mutableListOf<Long>()

    init {
      scope.backgroundScope.launch { ticks.coalesced(this@Rig.foreground).collect { refreshedAt += scope.currentTime } }
      scope.runCurrent()
    }
  }

  @Test fun `a tick after a quiet hour refreshes at once, in the foreground and in the background`() = runTest {
    for (fg in listOf(true, false)) {
      val r = Rig(this, foreground = fg)
      val t0 = currentTime
      advanceTimeBy(3_600_000)
      r.ticks.emit(Unit)
      runCurrent()
      assertEquals("foreground=$fg", listOf(t0 + 3_600_000), r.refreshedAt)
    }
  }

  @Test fun `fifty ticks in one second in the foreground make one refresh at once and one at the end of the window`() = runTest {
    val r = Rig(this, foreground = true)
    repeat(50) { r.ticks.emit(Unit); advanceTimeBy(20) }
    assertEquals(1_000L, currentTime)
    assertEquals(listOf(0L), r.refreshedAt)
    advanceTimeBy(1_100)
    assertEquals(listOf(0L, 2_000L), r.refreshedAt)
    advanceTimeBy(60_000)
    assertEquals("a refresh with no tick behind it", 2, r.refreshedAt.size)
  }

  @Test fun `a steady stream of ticks refreshes once per window, never more`() = runTest {
    val r = Rig(this, foreground = true)
    repeat(200) { r.ticks.emit(Unit); advanceTimeBy(100) }
    advanceTimeBy(2_100)
    val gaps = r.refreshedAt.zipWithNext { a, b -> b - a }
    assertEquals(true, gaps.isNotEmpty() && gaps.all { it >= 2_000 })
    assertEquals(true, r.refreshedAt.size in 10..12)
  }

  @Test fun `in the background a burst makes one refresh at once and one per fifteen seconds after`() = runTest {
    val r = Rig(this, foreground = false)
    repeat(60) { r.ticks.emit(Unit); advanceTimeBy(500) }
    advanceTimeBy(15_000)
    assertEquals(0L, r.refreshedAt.first())
    assertEquals(true, r.refreshedAt.zipWithNext { a, b -> b - a }.all { it >= 15_000L })
    assertEquals(3, r.refreshedAt.size)
  }

  @Test fun `coming to the foreground cuts a waiting background window short`() = runTest {
    val r = Rig(this, foreground = false)
    r.ticks.emit(Unit)
    runCurrent()
    r.ticks.emit(Unit)
    advanceTimeBy(5_000)
    assertEquals(listOf(0L), r.refreshedAt)
    r.foreground.value = true
    advanceTimeBy(1_900)
    assertEquals(listOf(0L), r.refreshedAt)
    advanceTimeBy(200)
    assertEquals(listOf(0L, 7_000L), r.refreshedAt)
  }

  @Test fun `the newest tick is never lost when a refresh is still running`() = runTest {
    val ticks = MutableSharedFlow<Unit>(extraBufferCapacity = 256)
    val foreground = MutableStateFlow(true)
    val finished = mutableListOf<Long>()
    backgroundScope.launch {
      ticks.coalesced(foreground).collect {
        kotlinx.coroutines.delay(5_000)
        finished += currentTime
      }
    }
    runCurrent()
    ticks.emit(Unit)
    advanceTimeBy(3_000)
    ticks.emit(Unit)
    advanceTimeBy(20_000)
    assertEquals(2, finished.size)
  }
}
