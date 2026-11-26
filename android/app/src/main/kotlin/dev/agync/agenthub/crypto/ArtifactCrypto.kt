package dev.agync.agenthub.crypto

import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.PBEKeySpec
import javax.crypto.spec.SecretKeySpec
import kotlinx.serialization.Serializable

/** The envelope the web app seals a protected artifact with. Same fields, same names. */
@Serializable
data class Envelope(val alg: String, val kdf: String, val iterations: Int, val salt: String, val iv: String)

class WrongPassword : Exception("wrong password or altered data")

class UnsupportedEnvelope : Exception("unsupported encryption envelope")

/**
 * Opens a protected artifact the way the web app's `crypto.mjs` does: PBKDF2-HMAC-SHA256 into an
 * AES-256-GCM key, with the envelope fields bound in as additional data. Standard JCA only.
 */
object ArtifactCrypto {
  private const val MIN_ITERATIONS = 100_000
  private const val MAX_ITERATIONS = 10_000_000

  private fun aad(e: Envelope) = "alg=${e.alg};kdf=${e.kdf};iterations=${e.iterations};salt=${e.salt};iv=${e.iv}".toByteArray(Charsets.UTF_8)

  fun decrypt(password: String, envelope: Envelope, ciphertextBase64: String): String {
    if (envelope.alg != "AES-256-GCM" || envelope.kdf != "PBKDF2-HMAC-SHA256") throw UnsupportedEnvelope()
    if (envelope.iterations !in MIN_ITERATIONS..MAX_ITERATIONS) throw UnsupportedEnvelope()
    require(password.isNotEmpty()) { "a password is required" }
    val salt = Base64.getDecoder().decode(envelope.salt)
    val iv = Base64.getDecoder().decode(envelope.iv)
    val spec = PBEKeySpec(password.toCharArray(), salt, envelope.iterations, 256)
    val key = SecretKeySpec(SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded, "AES")
    spec.clearPassword()
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
    cipher.updateAAD(aad(envelope))
    return try {
      String(cipher.doFinal(Base64.getDecoder().decode(ciphertextBase64)), Charsets.UTF_8)
    } catch (e: javax.crypto.AEADBadTagException) {
      throw WrongPassword()
    }
  }
}
