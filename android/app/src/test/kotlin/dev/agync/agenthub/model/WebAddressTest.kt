package dev.agync.agenthub.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class WebAddressTest {
  @Test fun `http and https addresses with a host are kept, without a trailing slash`() {
    assertEquals("https://hub.example:7318", WebAddress.parse("https://hub.example:7318/"))
    assertEquals("http://192.168.1.10:7318", WebAddress.parse(" http://192.168.1.10:7318 "))
  }

  @Test fun `anything else is dropped`() {
    for (bad in listOf(null, "", "hub.example", "javascript:alert(1)", "file:///etc/passwd", "intent://x#Intent;end", "https://user@hub.example", "https:///nohost", "ht tp://x"))
      assertNull(bad, WebAddress.parse(bad))
  }
}
