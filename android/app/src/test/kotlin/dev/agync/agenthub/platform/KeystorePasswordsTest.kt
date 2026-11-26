package dev.agync.agenthub.platform

import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
class KeystorePasswordsTest {
  @Config(sdk = [29]) @Test fun `on API 29 the check asks for a weak biometric or the screen lock, never the pair the platform refuses`() {
    assertEquals(BIOMETRIC_WEAK or DEVICE_CREDENTIAL, KeystorePasswords.confirmAuthenticators())
  }

  @Config(sdk = [35]) @Test fun `from API 30 it asks for a strong biometric or the screen lock`() {
    assertEquals(BIOMETRIC_STRONG or DEVICE_CREDENTIAL, KeystorePasswords.confirmAuthenticators())
  }
}
