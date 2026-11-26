package dev.agync.agenthub.data

import android.content.Context
import java.security.KeyStore

/**
 * What the phone remembers of a hub's project passwords, by hub. A password belongs to one hub:
 * two hubs with a project of the same name never share one, and unpairing a hub deletes all of
 * its passwords.
 */
interface PasswordVault {
  fun projects(hubId: String): List<String>

  fun forget(hubId: String, project: String)

  fun forgetHub(hubId: String)
}

/** The key for one hub's project, so the pair can never collide with another hub's. */
internal fun vaultKey(hubId: String, project: String) = hubId + "\u001F" + project

class MemoryPasswordVault : PasswordVault {
  private val entries = mutableSetOf<String>()

  fun add(hubId: String, project: String) { entries += vaultKey(hubId, project) }

  override fun projects(hubId: String) = entries.filter { it.startsWith(hubId + "\u001F") }.map { it.substringAfter("\u001F") }

  override fun forget(hubId: String, project: String) { entries -= vaultKey(hubId, project) }

  override fun forgetHub(hubId: String) { entries.removeAll { it.startsWith(hubId + "\u001F") } }
}

/** Sealed passwords in app-private storage, under one Keystore key. The key goes when the last password does. */
class PrefsPasswordVault(context: Context) : PasswordVault {
  private val prefs = context.getSharedPreferences(NAME, Context.MODE_PRIVATE)

  override fun projects(hubId: String): List<String> =
    prefs.all.keys.filter { it.startsWith("ct." + hubId + "\u001F") }.map { it.substringAfter("\u001F") }

  override fun forget(hubId: String, project: String) {
    prefs.edit().remove("ct." + vaultKey(hubId, project)).remove("iv." + vaultKey(hubId, project)).apply()
    dropKeyIfEmpty()
  }

  override fun forgetHub(hubId: String) {
    val edit = prefs.edit()
    prefs.all.keys.filter { it.substringAfter('.').startsWith(hubId + "\u001F") }.forEach(edit::remove)
    edit.apply()
    dropKeyIfEmpty()
  }

  private fun dropKeyIfEmpty() {
    if (prefs.all.keys.any { it.startsWith("ct.") }) return
    runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(KEY_ALIAS) }
  }

  companion object {
    const val NAME = "agent-hub-passwords"
    const val KEY_ALIAS = "agent-hub-project-passwords"
  }
}
