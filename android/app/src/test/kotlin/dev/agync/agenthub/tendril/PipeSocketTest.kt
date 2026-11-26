package dev.agync.agenthub.tendril

import java.io.IOException
import java.net.InetSocketAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PipeSocketTest {
  private class FakePipe : TendrilPipe {
    @Volatile var closed = false
    override fun send(bytes: ByteArray) = Unit
    override fun poll(timeoutMillis: Long): String { Thread.sleep(20); return """{"status":"would_block"}""" }
    override fun close() { closed = true }
  }

  private val watch = object : SessionWatch { override fun ended(reason: EndReason, sawData: Boolean) = Unit }

  @Test fun `a socket closed while its session is still opening closes the session that arrives`() {
    val opening = CountDownLatch(1)
    val release = CountDownLatch(1)
    val pipe = FakePipe()
    val socket = PipeSocket({ opening.countDown(); release.await(5, TimeUnit.SECONDS); pipe }, watch)
    var failure: Throwable? = null
    val connecting = Thread { try { socket.connect(InetSocketAddress.createUnresolved("hub", 80)) } catch (e: Throwable) { failure = e } }
    connecting.start()
    assertTrue(opening.await(5, TimeUnit.SECONDS))
    socket.close()
    release.countDown()
    connecting.join(5_000)
    assertTrue("connect ended with $failure", failure is IOException)
    assertTrue("the session that opened after the cancel was left open", pipe.closed)
  }

  @Test fun `a socket that is not closed keeps its session`() {
    val pipe = FakePipe()
    val socket = PipeSocket({ pipe }, watch)
    socket.connect(InetSocketAddress.createUnresolved("hub", 80))
    assertEquals(false, pipe.closed)
    socket.close()
    assertTrue(pipe.closed)
  }
}
