package dev.agync.agenthub.platform

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import dev.agync.agenthub.data.PrefsPasswordVault
import dev.agync.agenthub.data.vaultKey
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlin.coroutines.resume
import kotlinx.coroutines.suspendCancellableCoroutine

/**
 * Project passwords the person asked the phone to remember. Each is sealed under an Android
 * Keystore key that needs a fingerprint for every use, so the password never leaves the phone
 * and never opens without the person present.
 */
class KeystorePasswords(private val activity: FragmentActivity) {
  private val prefs = activity.getSharedPreferences(PrefsPasswordVault.NAME, Context.MODE_PRIVATE)

  val available: Boolean
    get() = BiometricManager.from(activity).canAuthenticate(BiometricManager.Authenticators.BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS


  private fun key(): SecretKey {
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
    val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    gen.init(
      KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .setKeySize(256)
        .setUserAuthenticationRequired(true)
        .apply {
          if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
          } else {
            // Before API 30, a validity of -1 asks for a fingerprint on every use.
            @Suppress("DEPRECATION") setUserAuthenticationValidityDurationSeconds(-1)
          }
        }
        .setInvalidatedByBiometricEnrollment(true)
        .build(),
    )
    return gen.generateKey()
  }

  suspend fun remember(hubId: String, project: String, password: String): Boolean {
    val id = vaultKey(hubId, project)
    if (!available) return false
    val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key()) }
    val unlocked = authenticate(cipher, "Remember the $project password", "Agent Hub keeps it in this phone's keystore, behind your fingerprint.") ?: return false
    val sealed = unlocked.doFinal(password.toByteArray(Charsets.UTF_8))
    prefs.edit()
      .putString("ct.$id", Base64.getEncoder().encodeToString(sealed))
      .putString("iv.$id", Base64.getEncoder().encodeToString(unlocked.iv))
      .apply()
    return true
  }

  suspend fun recall(hubId: String, project: String): String? {
    val id = vaultKey(hubId, project)
    val ct = prefs.getString("ct.$id", null) ?: return null
    val iv = prefs.getString("iv.$id", null) ?: return null
    val cipher =
      try {
        Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, Base64.getDecoder().decode(iv))) }
      } catch (_: Exception) {
        prefs.edit().remove("ct.$id").remove("iv.$id").apply()
        return null
      }
    val unlocked = authenticate(cipher, "Unlock $project", "Agent Hub uses your fingerprint to release the $project password kept on this phone.") ?: return null
    return String(unlocked.doFinal(Base64.getDecoder().decode(ct)), Charsets.UTF_8)
  }

  /** The fingerprint prompt alone, with no key: for the opt-in check before approving. */
  suspend fun confirm(title: String): Boolean =
    suspendCancellableCoroutine { cont ->
      val prompt =
        BiometricPrompt(
          activity, ContextCompat.getMainExecutor(activity),
          object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { if (cont.isActive) cont.resume(true) }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) { if (cont.isActive) cont.resume(false) }
          },
        )
      prompt.authenticate(
        BiometricPrompt.PromptInfo.Builder().setTitle(title).setAllowedAuthenticators(confirmAuthenticators()).build(),
      )
      cont.invokeOnCancellation { prompt.cancelAuthentication() }
    }

  private suspend fun authenticate(cipher: Cipher, title: String, subtitle: String): Cipher? =
    suspendCancellableCoroutine { cont ->
      val prompt =
        BiometricPrompt(
          activity, ContextCompat.getMainExecutor(activity),
          object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) { if (cont.isActive) cont.resume(result.cryptoObject?.cipher) }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) { if (cont.isActive) cont.resume(null) }
          },
        )
      prompt.authenticate(
        BiometricPrompt.PromptInfo.Builder().setTitle(title).setSubtitle(subtitle).setNegativeButtonText("Use password").setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG).build(),
        BiometricPrompt.CryptoObject(cipher),
      )
      cont.invokeOnCancellation { prompt.cancelAuthentication() }
    }

  companion object {
    private const val ALIAS = PrefsPasswordVault.KEY_ALIAS

    /**
     * What may answer the opt-in check before approving. Strong biometrics or the screen lock from API 30; before that
     * the platform refuses that pair, so a weak biometric or the screen lock is asked for instead.
     */
    fun confirmAuthenticators(sdk: Int = Build.VERSION.SDK_INT): Int =
      if (sdk >= Build.VERSION_CODES.R) BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL
      else BiometricManager.Authenticators.BIOMETRIC_WEAK or BiometricManager.Authenticators.DEVICE_CREDENTIAL
  }
}
