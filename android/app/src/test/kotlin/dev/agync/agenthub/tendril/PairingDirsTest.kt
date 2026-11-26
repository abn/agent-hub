package dev.agync.agenthub.tendril

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/**
 * A state directory is a hub's sealed identity and cannot be recovered, so pairing a second hub
 * must never touch the first one's. These drive the real [AttemptDirs] on real directories.
 */
class PairingDirsTest {
  @get:Rule val tmp = TemporaryFolder(File("build").also { it.mkdirs() })

  private var n = 0

  private fun dirs(root: File, paired: Set<String>) =
    AttemptDirs({ "d${n++}".also { File(root, it).mkdirs() } }, { File(root, it).deleteRecursively() }, { it in paired })

  private fun names(root: File) = root.list().orEmpty().toSet()

  @Test fun `pairing hub B after hub A leaves A's directory alone`() {
    val root = tmp.newFolder()
    val paired = mutableSetOf<String>()
    val d = dirs(root, paired)
    val a = d.begin(); paired += a; d.adopt()
    val b = d.begin()
    assertTrue(File(root, a).exists())
    paired += b; d.adopt()
    assertEquals(setOf(a, b), names(root))
    assertNull(d.current)
  }

  @Test fun `a failed or aborted attempt leaves no orphan and keeps A`() {
    val root = tmp.newFolder()
    val paired = mutableSetOf<String>()
    val d = dirs(root, paired)
    val a = d.begin(); paired += a; d.adopt()
    val failed = d.begin()
    d.discard()
    assertFalse(File(root, failed).exists())
    d.begin()
    d.begin()
    d.discard()
    assertEquals(setOf(a), names(root))
  }

  @Test fun `a directory a paired hub owns is refused even if the attempt still points at it`() {
    val root = tmp.newFolder()
    val d = dirs(root, setOf("d0"))
    val a = d.begin()
    d.discard()
    assertTrue(File(root, a).exists())
  }

  @Test fun `an adopted directory is not discarded by a later discard`() {
    val root = tmp.newFolder()
    val d = dirs(root, emptySet())
    val a = d.begin()
    d.adopt()
    d.discard()
    assertTrue(File(root, a).exists())
  }
}
