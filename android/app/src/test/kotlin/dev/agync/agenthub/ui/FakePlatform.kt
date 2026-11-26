package dev.agync.agenthub.ui

import androidx.compose.runtime.Composable
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.ui.screens.ProtectedGate

/** A platform that records what the app asks of it. */
class FakePlatform(var fingerprintOk: Boolean = true) : Platform {
  val copied = mutableListOf<Pair<String, String>>()
  var notificationAnswer = true
  var settingsOpened = 0
  var stay: Boolean? = null
  val fingerprintAsked = mutableListOf<String>()

  override fun copy(label: String, text: String) { copied += label to text }

  override fun openAppSettings() { settingsOpened++ }

  override fun openNotificationSettings() { settingsOpened++ }

  val openedWeb = mutableListOf<String>()

  /** Whether the last [ProtectedGate.open] ran on the main thread; null until it ran. */
  @Volatile var openedOnMain: Boolean? = null

  override fun openWebApp(url: String) { openedWeb += url }

  override fun requestNotifications(onResult: (Boolean) -> Unit) = onResult(notificationAnswer)

  override suspend fun confirmWithFingerprint(reason: String): Boolean { fingerprintAsked += reason; return fingerprintOk }

  override fun setStayConnected(stay: Boolean) { this.stay = stay }

  override val protectedGate: ProtectedGate =
    object : ProtectedGate {
      override suspend fun remembered(hubId: String, project: String): String? = null

      override suspend fun remember(hubId: String, project: String, password: String) = Unit

      override suspend fun open(view: ArtifactView, password: String): String? {
        openedOnMain = android.os.Looper.myLooper() === android.os.Looper.getMainLooper()
        return null
      }
    }

  @Composable
  override fun Scanner(onScanned: (String) -> Unit, onDenied: () -> Unit) = Unit
}
