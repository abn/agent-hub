package dev.agync.agenthub.alerts

import android.app.NotificationManager
import android.content.Context
import androidx.test.core.app.ApplicationProvider
import dev.agync.agenthub.Fixtures
import dev.agync.agenthub.data.AppContainer
import dev.agync.agenthub.data.HubClient
import dev.agync.agenthub.data.HubClientFactory
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.PairedHub
import dev.agync.agenthub.data.SeededHub
import dev.agync.agenthub.pairing.SeededPairing
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNotSame
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class AlertCenterTest {
  private val context: Context = ApplicationProvider.getApplicationContext()
  private val hub = PairedHub("seeded-hub", "home hub", "pixel-phone", stateDir = "dir-1")
  private var clients = 0
  private val clock = Fixtures.clock()
  private val seeded = SeededHub(clock)

  @org.junit.Before fun allowNotifications() {
    shadowOf(context as android.app.Application).grantPermissions(android.Manifest.permission.POST_NOTIFICATIONS)
  }

  private fun container() =
    AppContainer(clock, MemoryStore(), SeededPairing(clock, 0, 0), HubClientFactory { clients++; seeded as HubClient }, Fixtures.fmt(), "pixel-phone")

  private fun center(c: AppContainer, holds: MutableList<Boolean> = mutableListOf()) =
    AlertCenter(context, c, clock, scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined), holdService = { holds += it })

  private fun shown(id: Int) = shadowOf(context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).getNotification(null, id)

  @Test fun `a hub paired again gets a new repository and a new engine, never the closed one`() {
    val c = container()
    c.registry.add(hub)
    c.prefs.update { it.copy(stayConnected = true) }
    val first = c.repository(hub)
    assertSame(first, c.repository(hub))
    assertEquals(1, clients)
    val alerts = center(c)
    alerts.sync()
    val oldEngine = alerts.engine(hub.id)
    assertNotNull(oldEngine)
    c.registry.add(hub.copy(stateDir = "dir-2"))
    val second = c.repository(c.registry.state.value.current!!)
    assertNotSame("the repository of the replaced pairing was kept", first, second)
    assertEquals(2, clients)
    alerts.sync()
    assertNotSame("the alert engine kept the old hub's repository", oldEngine, alerts.engine(hub.id))
  }

  @Test fun `stopping leaves no collector running and no Connected notification, even when the hub changes after`() {
    val c = container()
    c.registry.add(hub)
    c.prefs.update { it.copy(stayConnected = true) }
    val holds = mutableListOf<Boolean>()
    val alerts = center(c, holds)
    alerts.sync()
    alerts.sync()
    val running = alerts.runningJobs
    assertTrue("jobs while held: $running", running > 0)
    alerts.sync()
    assertEquals("syncing again must not add collectors", running, alerts.runningJobs)
    assertNotNull(shown(SystemAlertPoster.CONNECTION_ID))
    c.prefs.update { it.copy(stayConnected = false) }
    alerts.sync()
    assertEquals(0, alerts.runningJobs)
    assertNull("the Connected notification stayed", shown(SystemAlertPoster.CONNECTION_ID))
    assertEquals(false, holds.last())
    runBlocking { c.repository(hub).refresh() }
    assertNull("a stray collector posted after stop", shown(SystemAlertPoster.CONNECTION_ID))
  }

  @Test fun `the widget job follows the hubs that are paired now`() {
    val c = container()
    c.registry.add(hub)
    c.prefs.update { it.copy(stayConnected = true) }
    val alerts = center(c)
    alerts.sync()
    val one = alerts.runningJobs
    c.registry.add(PairedHub("second-hub", "second", "pixel-phone", stateDir = "dir-9"))
    alerts.sync()
    assertTrue("a second hub should add its jobs but not leave the first widget job running: $one -> ${alerts.runningJobs}", alerts.runningJobs == one + 2)
  }

  @Test fun `a refused foreground service does not stop the sessions from being held`() {
    val c = container()
    c.registry.add(hub)
    c.prefs.update { it.copy(stayConnected = true) }
    val alerts = AlertCenter(context, c, clock, scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined), holdService = { if (it) throw IllegalStateException("not allowed now") })
    alerts.sync()
    assertTrue(alerts.runningJobs > 0)
  }

  @Test fun `the phone's restart is received, so the sessions come back without opening the app`() {
    val receivers = context.packageManager.queryBroadcastReceivers(android.content.Intent(android.content.Intent.ACTION_BOOT_COMPLETED), 0)
    assertTrue(receivers.map { it.activityInfo.name }.toString(), receivers.any { it.activityInfo.name.endsWith("alerts.BootReceiver") })
  }
}
