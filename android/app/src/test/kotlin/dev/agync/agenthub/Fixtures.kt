package dev.agync.agenthub

import dev.agync.agenthub.data.HubRepository
import dev.agync.agenthub.data.MemoryStore
import dev.agync.agenthub.data.PairedHub
import dev.agync.agenthub.data.SeededHub
import dev.agync.agenthub.model.Fmt
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import java.util.Locale
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.setMain
import org.junit.rules.TestWatcher
import org.junit.runner.Description

/** Tuesday 24 November, 14:32 UTC: the time every board frame is drawn at. */
object Fixtures {
  val now: Instant = Instant.parse("2026-11-24T14:32:00Z")
  val hub = PairedHub("seeded-hub", "home hub", "pixel-phone")

  class MutableClock(var at: Instant) : Clock() {
    override fun getZone() = ZoneOffset.UTC

    override fun withZone(zone: java.time.ZoneId?) = this

    override fun instant(): Instant = at
  }

  fun clock() = MutableClock(now)

  fun fmt() = Fmt(ZoneOffset.UTC, Locale.UK)

  class Rig(val clock: MutableClock = clock(), val seeded: SeededHub = SeededHub(clock), val store: MemoryStore = MemoryStore()) {
    val repo = HubRepository(hub, seeded, store, clock)
  }

  fun rig() = Rig()
}

@OptIn(ExperimentalCoroutinesApi::class)
class MainDispatcherRule : TestWatcher() {
  override fun starting(description: Description) = Dispatchers.setMain(UnconfinedTestDispatcher())

  override fun finished(description: Description) = Dispatchers.resetMain()
}
