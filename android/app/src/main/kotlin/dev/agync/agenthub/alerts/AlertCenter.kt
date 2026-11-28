package dev.agync.agenthub.alerts

import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import dev.agync.agenthub.data.AppContainer
import dev.agync.agenthub.data.HubRepository
import java.time.Clock
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * Keeps the held sessions and the shade in step with the paired hubs and the person's settings.
 * One engine per hub; each holds that hub's Tendril session and refetches on every tick. Everything it
 * starts is a job it keeps, so stopping cancels all of it and nothing posts afterwards.
 */
class AlertCenter(
  private val context: Context,
  private val container: AppContainer,
  private val clock: Clock = Clock.systemDefaultZone(),
  val poster: SystemAlertPoster = SystemAlertPoster(context, { container.prefs.state.value }, { container.registry.state.value.hubs.size }),
  private val scope: CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default),
  /** Starts or stops the foreground service that keeps the process alive. Replaced in tests. */
  private val holdService: (Boolean) -> Unit = { start ->
    if (start) runCatching { ContextCompat.startForegroundService(context, Intent(context, HoldService::class.java)) }
    else context.stopService(Intent(context, HoldService::class.java))
  },
) {
  /** One hub held open: the repository it was built on, its engine, and every job that serves it. */
  private class Held(val repo: HubRepository, val engine: AlertEngine, val jobs: List<Job>)

  private val held = HashMap<String, Held>()
  private var prefsJob: Job? = null
  private var widgetJob: Job? = null
  private var widgetHubs: List<String> = emptyList()
  private var registryJob: Job? = null

  init {
    Channels.create(context)
  }

  /** How many jobs this center keeps running: none once it is stopped. */
  @get:Synchronized
  val runningJobs: Int
    get() = held.values.sumOf { h -> h.jobs.count { it.isActive } } + listOf(prefsJob, widgetJob, registryJob).count { it?.isActive == true }

  /**
   * Whether HoldService is running. Its own start syncs, so asking for the service again from that sync
   * would restart it in a loop: each lap posts the foreground notification and keeps the phone awake.
   */
  private var serviceRunning = false

  /** Called by HoldService when it starts. */
  @Synchronized
  fun serviceStarted() {
    serviceRunning = true
  }

  /** Called by HoldService when it stops. */
  @Synchronized
  fun serviceStopped() {
    serviceRunning = false
  }

  @Synchronized
  fun engine(hubId: String): AlertEngine? = held[hubId]?.engine

  /** Starts or stops the held sessions to match the settings and the paired hubs. Idempotent. */
  @Synchronized
  fun sync() {
    if (registryJob == null) {
      // Pairing a hub, pairing it again, or forgetting one changes what is held, whoever did it.
      registryJob = scope.launch { container.registry.state.map { s -> s.hubs.map { it.id to it.stateDir } }.distinctUntilChanged().drop(1).collect { sync() } }
    }
    val hubs = container.registry.state.value.hubs
    val stay = container.prefs.state.value.stayConnected && hubs.isNotEmpty()
    if (!stay) {
      stopHeld()
      holdService(false)
      serviceRunning = false
      return
    }
    for ((id, h) in held.toList()) {
      val hub = hubs.firstOrNull { it.id == id }
      // A hub that left, or that was paired again and so has a new repository, must not keep its old session.
      if (hub == null || container.repository(hub) !== h.repo) stop(id)
    }
    for (hub in hubs) {
      if (held.containsKey(hub.id)) continue
      val repo = container.repository(hub)
      val engine = AlertEngine(repo, poster, { container.prefs.state.value }, clock)
      held[hub.id] = Held(repo, engine, listOf(scope.launch { engine.run() }, scope.launch { repo.state.collect { engine.apply() } }))
    }
    val ids = hubs.map { it.id }
    if (widgetJob == null || widgetHubs != ids) {
      // The launcher widgets read the cache, so they redraw when it changes; the job follows the hubs there are now.
      widgetJob?.cancel()
      widgetHubs = ids
      widgetJob = scope.launch {
        val repos = hubs.map(container::repository)
        widgetTriggers(repos).collect {
          runCatching { dev.agync.agenthub.widgets.WidgetRefresh.updateAll(context) }
          kotlinx.coroutines.delay(2_000)
        }
      }
    }
    if (prefsJob == null) {
      // The buttons depend on the fingerprint opt-in and the lock-screen setting, so a change reposts what is showing.
      prefsJob = scope.launch {
        container.prefs.state.map { it.fingerprintBeforeApproving to it.hideOnLockScreen }.distinctUntilChanged().drop(1).collect {
          synchronized(this@AlertCenter) { held.values.map { it.engine } }.forEach(AlertEngine::repostAll)
        }
      }
    }
    // The system may refuse to start the service (ForegroundServiceStartNotAllowedException from API 31): the held
    // sessions still run while the process lives, and the next sync starts the service when it is allowed.
    if (!serviceRunning) {
      try {
        holdService(true)
      } catch (_: RuntimeException) {
      }
    }
  }

  private fun stop(hubId: String) {
    held.remove(hubId)?.jobs?.forEach(Job::cancel)
  }

  private fun stopHeld() {
    held.values.forEach { h -> h.jobs.forEach(Job::cancel) }
    held.clear()
    prefsJob?.cancel(); prefsJob = null
    widgetJob?.cancel(); widgetJob = null; widgetHubs = emptyList()
    registryJob?.cancel(); registryJob = null
    poster.clearConnection()
  }

  private val actions =
    NotificationActions({ id -> container.registry.state.value.hubs.firstOrNull { it.id == id }?.let(container::repository) }, { container.prefs.state.value })

  /** A shade action, checked against the live ask. See [NotificationActions]. */
  suspend fun act(hubId: String, askId: String, action: String, option: Int, typed: String?): ActionOutcome {
    val outcome = actions.act(hubId, askId, action, option, typed)
    engine(hubId)?.apply()
    return outcome
  }

  /** The service is going: cancel every job. A later [sync] starts them again. */
  @Synchronized
  fun stopAll() {
    stopHeld()
  }
}

/** What redraws the launcher widgets: a change in what a hub holds, or the minute its last answer is shown as. */
internal fun widgetTriggers(repos: List<HubRepository>): kotlinx.coroutines.flow.Flow<Unit> =
  kotlinx.coroutines.flow.merge(*repos.flatMap { r -> listOf(r.state.map { }, r.syncedAt.map { }) }.toTypedArray())
