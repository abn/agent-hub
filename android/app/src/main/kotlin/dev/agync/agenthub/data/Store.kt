package dev.agync.agenthub.data

import android.content.Context
import java.util.concurrent.ConcurrentHashMap

/** Small key to string storage for device-local state. Never holds a credential. */
interface LocalStore {
  fun get(key: String): String?

  fun put(key: String, value: String?)
}

class MemoryStore : LocalStore {
  private val map = ConcurrentHashMap<String, String>()

  override fun get(key: String): String? = map[key]

  override fun put(key: String, value: String?) {
    if (value == null) map.remove(key) else map[key] = value
  }
}

class PrefsStore(context: Context, name: String = "agent-hub-local") : LocalStore {
  private val prefs = context.getSharedPreferences(name, Context.MODE_PRIVATE)

  override fun get(key: String): String? = prefs.getString(key, null)

  override fun put(key: String, value: String?) {
    prefs.edit().apply { if (value == null) remove(key) else putString(key, value) }.apply()
  }
}
