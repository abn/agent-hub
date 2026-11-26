package dev.agync.agenthub.model

import org.junit.Assert.assertEquals
import org.junit.Test

class BrainPathTest {
  @Test fun `a bare key is a kv entry`() = assertEquals("/kv/upgrade-plan", BrainPath.of("upgrade-plan"))

  @Test fun `a namespaced key gains its slash and a full path is kept`() {
    assertEquals("/kv/upgrade-plan", BrainPath.of("kv/upgrade-plan"))
    assertEquals("/fs/notes/plan.md", BrainPath.of("fs/notes/plan.md"))
    assertEquals("/fs/notes/plan.md", BrainPath.of("/fs/notes/plan.md"))
  }
}
