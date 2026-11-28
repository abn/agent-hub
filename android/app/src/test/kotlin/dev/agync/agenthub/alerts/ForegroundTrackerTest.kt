package dev.agync.agenthub.alerts

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ForegroundTrackerTest {
  @Test fun `the app is in the foreground while any activity is started`() {
    val f = ForegroundTracker()
    assertFalse(f.state.value)
    f.started()
    f.started()
    assertTrue(f.state.value)
    f.stopped()
    assertTrue("one activity is still started", f.state.value)
    f.stopped()
    assertFalse(f.state.value)
  }

  @Test fun `a stop without a start does not drive the count below zero`() {
    val f = ForegroundTracker()
    f.stopped()
    f.started()
    assertTrue(f.state.value)
  }
}
