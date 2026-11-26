package dev.agync.agenthub.alerts

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AlertIntentsTest {
  @Test fun `every button of every ask has its own URI`() {
    val uris =
      listOf(
        AlertIntents.uri("h", "a", AlertIntents.ACTION_APPROVE), AlertIntents.uri("h", "a", AlertIntents.ACTION_DECLINE),
        AlertIntents.uri("h", "a", AlertIntents.ACTION_SNOOZE), AlertIntents.uri("h", "a", AlertIntents.ACTION_ANSWER, 0),
        AlertIntents.uri("h", "a", AlertIntents.ACTION_ANSWER, 1), AlertIntents.uri("h", "a", AlertIntents.ACTION_ANSWER),
        AlertIntents.uri("h", "b", AlertIntents.ACTION_APPROVE), AlertIntents.uri("g", "a", AlertIntents.ACTION_APPROVE),
      ).map { it.toString() }
    assertEquals(uris.size, uris.toSet().size)
  }

  @Test fun `a button reads back as what it was made for`() {
    val t = AlertIntents.parse(AlertIntents.uri("hub 1", "ask/2", AlertIntents.ACTION_ANSWER, 1))!!
    assertEquals(AlertIntents.Target("hub 1", "ask/2", AlertIntents.ACTION_ANSWER, 1), t)
  }

  @Test fun `approve and decline cannot be mistaken for each other`() {
    assertNotEquals(AlertIntents.uri("h", "a", AlertIntents.ACTION_APPROVE), AlertIntents.uri("h", "a", AlertIntents.ACTION_DECLINE))
  }

  @Test fun `a foreign URI is not ours`() {
    assertNull(AlertIntents.parse(android.net.Uri.parse("https://example.org/ask/h/a/approve/-1")))
    assertNull(AlertIntents.parse(android.net.Uri.parse("agenthub://ask/h/a/approve")))
  }
}
