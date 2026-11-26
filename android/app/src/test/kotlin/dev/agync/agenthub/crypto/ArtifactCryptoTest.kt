package dev.agync.agenthub.crypto

import dev.agync.agenthub.data.SeededHub
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ArtifactCryptoTest {
  /** The envelope and ciphertext were sealed by the web app's own crypto.mjs, so this is a cross-implementation check. */
  @Test fun `opens what the web app sealed`() {
    val html = ArtifactCrypto.decrypt(SeededHub.PROTECTED_PASSWORD, SeededHub.PROTECTED_ENVELOPE, SeededHub.PROTECTED_CIPHERTEXT)
    assertEquals("<!doctype html><h1>Q3 roadmap</h1><p>Ship the phone app.</p>", html)
  }

  @Test fun `a wrong password is refused`() {
    assertThrows(WrongPassword::class.java) { ArtifactCrypto.decrypt("not it", SeededHub.PROTECTED_ENVELOPE, SeededHub.PROTECTED_CIPHERTEXT) }
  }

  @Test fun `an altered envelope fails authentication`() {
    val tampered = SeededHub.PROTECTED_ENVELOPE.copy(salt = "AAAAAAAAAAAAAAAAAAAAAA==")
    assertThrows(WrongPassword::class.java) { ArtifactCrypto.decrypt(SeededHub.PROTECTED_PASSWORD, tampered, SeededHub.PROTECTED_CIPHERTEXT) }
  }

  @Test fun `an iteration count below the floor is not accepted`() {
    val weak = SeededHub.PROTECTED_ENVELOPE.copy(iterations = 1000)
    assertThrows(UnsupportedEnvelope::class.java) { ArtifactCrypto.decrypt(SeededHub.PROTECTED_PASSWORD, weak, SeededHub.PROTECTED_CIPHERTEXT) }
  }

  @Test fun `another algorithm is not accepted`() {
    val other = SeededHub.PROTECTED_ENVELOPE.copy(alg = "AES-128-CBC")
    assertThrows(UnsupportedEnvelope::class.java) { ArtifactCrypto.decrypt(SeededHub.PROTECTED_PASSWORD, other, SeededHub.PROTECTED_CIPHERTEXT) }
  }
}
