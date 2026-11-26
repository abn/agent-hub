package dev.agync.agenthub

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.tendril.TendrilEndpoint
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Runs on a device. Tendril's native library only loads there (an UnsatisfiedLinkError once hid in
 * a missing SONAME and a 16 KB page alignment), so every unit test passing proves nothing about it.
 */
@RunWith(AndroidJUnit4::class)
class TendrilNativeSmokeTest {
  @Test fun theNativeLibraryLoadsAndAnEphemeralEndpointExists() {
    TendrilEndpoint.create().use { endpoint -> assertTrue(endpoint.infoJson().isNotEmpty()) }
  }

  @Test fun aSealedEndpointCreatesUnderAKeystoreWrappedStateDirectoryAndReopens() {
    val dir = File(InstrumentationRegistry.getInstrumentation().targetContext.cacheDir, "tendril-smoke-" + System.nanoTime())
    try {
      val first = TendrilEndpoint.create(dir.absolutePath).use { it.infoJson() }
      val again = TendrilEndpoint.create(dir.absolutePath).use { it.infoJson() }
      assertTrue(first.isNotEmpty())
      assertEquals("the same identity comes back from the sealed directory", first, again)
    } finally {
      dir.deleteRecursively()
    }
  }

  @Test fun theAppRunsAsItsDebugPackageAndMayUseTheNetwork() {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    assertTrue(context.packageName.endsWith(".debug"))
    assertEquals(android.content.pm.PackageManager.PERMISSION_GRANTED, context.checkSelfPermission(android.Manifest.permission.INTERNET))
  }

  /**
   * The hub's REST rides Tendril as HTTP/2 prior knowledge, which OkHttp issues as an http request to a
   * placeholder host. Android refuses cleartext by default, which stalled every request on a real phone;
   * only that placeholder is allowed, and nothing else.
   */
  @Test fun theHubPlaceholderHostMayUseCleartextAndNothingElseMay() {
    val policy = android.security.NetworkSecurityPolicy.getInstance()
    assertTrue(policy.isCleartextTrafficPermitted("hub"))
    assertEquals(false, policy.isCleartextTrafficPermitted("example.com"))
    assertEquals(false, policy.isCleartextTrafficPermitted("192.168.1.10"))
  }
}
