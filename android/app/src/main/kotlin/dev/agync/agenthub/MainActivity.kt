package dev.agync.agenthub

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import dev.agync.agenthub.model.WebAddress
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import dev.agync.agenthub.alerts.AlertIntents
import dev.agync.agenthub.model.ArtifactView
import dev.agync.agenthub.nav.DeepLink
import dev.agync.agenthub.nav.Navigator
import dev.agync.agenthub.nav.Route
import dev.agync.agenthub.platform.CameraScanner
import dev.agync.agenthub.platform.KeystorePasswords
import dev.agync.agenthub.ui.AppRoot
import dev.agync.agenthub.ui.Platform
import dev.agync.agenthub.ui.screens.ProtectedGate

class MainActivity : FragmentActivity(), Platform {
  private val navigator = Navigator()
  private lateinit var passwords: KeystorePasswords
  private var notificationResult: ((Boolean) -> Unit)? = null

  private val notificationLauncher =
    registerForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> notificationResult?.invoke(ok) }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    passwords = KeystorePasswords(this)
    route(intent)
    val app = application as AgentHubApp
    setContent { AppRoot(app.container, this, navigator) }
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    route(intent)
  }

  private fun route(intent: Intent?) {
    intent ?: return
    val app = application as AgentHubApp
    DeepLink.open(intent.getStringExtra(AlertIntents.EXTRA_ASK_ID), intent.getStringExtra(AlertIntents.EXTRA_HUB_ID), app.container.registry, navigator)
  }

  override fun onStart() {
    super.onStart()
    (application as AgentHubApp).alerts.sync()
  }

  override fun copy(label: String, text: String) {
    (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText(label, text))
  }

  override fun openAppSettings() {
    startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null)))
  }

  override fun openNotificationSettings() {
    startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
  }

  override fun openWebApp(url: String) {
    // Only an address the hub reported and the app vetted (http or https) gets this far; the browser handles it.
    if (WebAddress.parse(url) == null) return
    try {
      startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE))
    } catch (_: android.content.ActivityNotFoundException) {
    }
  }

  override fun requestNotifications(onResult: (Boolean) -> Unit) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) {
      onResult(true)
    } else {
      notificationResult = onResult
      notificationLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
  }

  override suspend fun confirmWithFingerprint(reason: String): Boolean = passwords.confirm(reason)

  override fun setStayConnected(stay: Boolean) {
    (application as AgentHubApp).alerts.sync()
  }

  override val protectedGate: ProtectedGate =
    object : ProtectedGate {
      override suspend fun remembered(hubId: String, project: String): String? = passwords.recall(hubId, project)

      override suspend fun remember(hubId: String, project: String, password: String) {
        passwords.remember(hubId, project, password)
      }

      override suspend fun open(view: ArtifactView, password: String): String? {
        val sealed = view.sealed ?: return view.html
        return try {
          dev.agync.agenthub.crypto.ArtifactCrypto.decrypt(password, sealed.envelope, sealed.ciphertext)
        } catch (_: dev.agync.agenthub.crypto.WrongPassword) {
          null
        }
      }
    }

  @Composable
  override fun Scanner(onScanned: (String) -> Unit, onDenied: () -> Unit) = CameraScanner(onScanned, onDenied)

}
