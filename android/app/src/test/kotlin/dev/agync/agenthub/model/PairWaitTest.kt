package dev.agync.agenthub.model

import java.time.Duration
import java.time.Instant
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PairWaitTest {
  private val since = Instant.parse("2026-11-24T14:32:00Z")
  private val deadline = since.plus(Duration.ofMinutes(15))

  private fun wait(checkedAgo: Duration? = null, at: Duration) = PairWait(since, deadline, checkedAgo?.let { since.plus(at).minus(it) })

  @Test fun `the first seconds read as waiting with minutes and seconds`() {
    val v = wait(at = Duration.ofSeconds(0)).view(since)
    assertEquals("waiting 0:00", v.meta)
    assertFalse(v.stillWaiting)
    assertEquals("Waiting for you to confirm on the hub", v.status)
  }

  @Test fun `seconds are padded and minutes run past nine`() {
    assertEquals("waiting 1:05", wait(at = Duration.ofSeconds(65)).view(since.plusSeconds(65)).meta.substringBefore(" ·"))
    assertEquals("waiting 12:30", wait(at = Duration.ofSeconds(750)).view(since.plusSeconds(750)).meta.substringBefore(" ·"))
  }

  @Test fun `before two minutes the last check is not mentioned`() {
    val v = wait(checkedAgo = Duration.ofSeconds(1), at = Duration.ofSeconds(119)).view(since.plusSeconds(119))
    assertEquals("waiting 1:59", v.meta)
    assertFalse(v.stillWaiting)
  }

  @Test fun `at two minutes the wait is still waiting, and a fresh check reads just now`() {
    val v = wait(checkedAgo = Duration.ofSeconds(3), at = Duration.ofSeconds(120)).view(since.plusSeconds(120))
    assertEquals("waiting 2:00 · checked just now", v.meta)
    assertTrue(v.stillWaiting)
    assertEquals("Still waiting for you to confirm on the hub", v.status)
  }

  @Test fun `an older check is counted in seconds`() {
    val v = wait(checkedAgo = Duration.ofSeconds(5), at = Duration.ofSeconds(200)).view(since.plusSeconds(200))
    assertEquals("waiting 3:20 · checked 5s ago", v.meta)
    assertEquals("waiting 3:20 · checked just now", wait(checkedAgo = Duration.ofSeconds(4), at = Duration.ofSeconds(200)).view(since.plusSeconds(200)).meta)
  }

  @Test fun `with no check recorded yet the meta line holds only the elapsed time`() {
    assertEquals("waiting 2:10", wait(at = Duration.ofSeconds(130)).view(since.plusSeconds(130)).meta)
  }

  @Test fun `the status changes only when the state does, not every second`() {
    val a = wait(at = Duration.ofSeconds(10)).view(since.plusSeconds(10)).status
    val b = wait(at = Duration.ofSeconds(100)).view(since.plusSeconds(100)).status
    val c = wait(at = Duration.ofSeconds(130)).view(since.plusSeconds(130)).status
    val d = wait(at = Duration.ofSeconds(500)).view(since.plusSeconds(500)).status
    assertEquals(a, b)
    assertEquals(c, d)
    assertTrue(a != c)
  }
}
