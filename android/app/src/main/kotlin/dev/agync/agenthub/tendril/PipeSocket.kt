package dev.agync.agenthub.tendril

import dev.tendril.TendrilSession
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetAddress
import java.net.Socket
import java.net.SocketAddress
import java.util.Base64
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.net.SocketFactory
import org.json.JSONObject

/** The two things the transport needs of a Tendril session, so it can be tested without the native library. */
interface TendrilPipe {
  /** At most 16384 bytes. */
  fun send(bytes: ByteArray)

  /** One event as Tendril's JSON, or the would-block or eof marker. */
  fun poll(timeoutMillis: Long): String

  fun close()
}

class SessionPipe(private val session: TendrilSession) : TendrilPipe {
  override fun send(bytes: ByteArray) = session.sendData(bytes)

  override fun poll(timeoutMillis: Long): String = session.pollEvent(timeoutMillis)

  override fun close() = session.close()
}

/** Why a session ended, as far as Tendril says. */
enum class EndReason { Closed, Revoked, Eof }

/** What the app learns about the sessions it opens: how they end, and whether anything ever came back. */
interface SessionWatch {
  fun ended(reason: EndReason, sawData: Boolean)
}

/**
 * A byte pipe over a Tendril session that looks like a socket to OkHttp. The hub speaks HTTP/2 with
 * prior knowledge over the session's DATA payloads, so OkHttp runs on top of this unchanged. It
 * dials nothing: the session is opened by [connectPipe] when OkHttp "connects".
 */
class PipeSocket(private val connectPipe: () -> TendrilPipe, private val watch: SessionWatch) : Socket() {
  @Volatile private var pipe: TendrilPipe? = null
  private val inbox = LinkedBlockingQueue<ByteArray>()
  private val eof = ByteArray(0)

  @Volatile private var closed = false
  @Volatile private var inputDone = false
  @Volatile private var sawData = false
  @Volatile private var timeoutMillis = 0
  @Volatile private var reader: Thread? = null

  override fun connect(endpoint: SocketAddress?) = connect(endpoint, 0)

  override fun connect(endpoint: SocketAddress?, timeout: Int) {
    // Tendril reports a session that cannot open or has died as an IllegalStateException; OkHttp copes
    // with IOException only, and an unchecked one ends its thread and the app with it.
    val opened = try { connectPipe() } catch (e: RuntimeException) { throw IOException(e.message, e) }
    pipe = opened
    // OkHttp may cancel the call while the session is still opening. The session that arrives after that is nobody's: close it.
    if (closed) {
      runCatching { opened.close() }
      throw IOException("closed")
    }
    reader = Thread({ pump() }, "tendril-pipe-reader").also { it.isDaemon = true; it.start() }
  }

  private fun pump() {
    val p = pipe ?: return
    var reason = EndReason.Eof
    try {
      while (!closed) {
        val text = p.poll(POLL_MILLIS)
        val json = runCatching { JSONObject(text) }.getOrNull() ?: continue
        when {
          json.optString("event") == "data" -> {
            sawData = true
            inbox.put(Base64.getUrlDecoder().decode(json.getString("data_b64")))
          }
          json.optString("event") == "closed" -> {
            reason = if (json.optInt("code") == REVOKED) EndReason.Revoked else EndReason.Closed
            break
          }
          json.optString("status") == "eof" -> break
        }
      }
    } catch (_: Exception) {
      // A failed poll ends the session like an EOF does.
    } finally {
      if (!closed) watch.ended(reason, sawData)
      inputDone = true
      inbox.put(eof)
    }
  }

  private val input =
    object : InputStream() {
      private var current: ByteArray = ByteArray(0)
      private var at = 0

      override fun read(): Int {
        val b = ByteArray(1)
        return if (read(b, 0, 1) < 0) -1 else b[0].toInt() and 0xff
      }

      override fun read(buffer: ByteArray, off: Int, len: Int): Int {
        if (len == 0) return 0
        while (at >= current.size) {
          if (inputDone && inbox.isEmpty()) return -1
          val next =
            if (timeoutMillis > 0) inbox.poll(timeoutMillis.toLong(), TimeUnit.MILLISECONDS) ?: throw java.net.SocketTimeoutException("read timed out")
            else inbox.take()
          if (next === eof) { inputDone = true; inbox.put(eof); return -1 }
          current = next
          at = 0
        }
        val n = minOf(len, current.size - at)
        System.arraycopy(current, at, buffer, off, n)
        at += n
        return n
      }

      override fun available(): Int = current.size - at
    }

  private val output =
    object : OutputStream() {
      override fun write(b: Int) = write(byteArrayOf(b.toByte()), 0, 1)

      override fun write(buffer: ByteArray, off: Int, len: Int) {
        val p = pipe ?: throw IOException("not connected")
        if (closed) throw IOException("closed")
        var at = off
        while (at < off + len) {
          val n = minOf(MAX_CHUNK, off + len - at)
          try { p.send(buffer.copyOfRange(at, at + n)) } catch (e: RuntimeException) { throw IOException(e.message, e) }
          at += n
        }
      }
    }

  override fun getInputStream(): InputStream = input

  override fun getOutputStream(): OutputStream = output

  override fun isConnected(): Boolean = pipe != null

  override fun isClosed(): Boolean = closed

  override fun isInputShutdown(): Boolean = inputDone

  override fun isOutputShutdown(): Boolean = closed

  override fun shutdownInput() = Unit

  override fun shutdownOutput() = Unit

  override fun setSoTimeout(timeout: Int) { timeoutMillis = timeout }

  override fun getSoTimeout(): Int = timeoutMillis

  override fun setTcpNoDelay(on: Boolean) = Unit

  override fun setKeepAlive(on: Boolean) = Unit

  override fun getInetAddress(): InetAddress = InetAddress.getLoopbackAddress()

  override fun close() {
    if (closed) return
    closed = true
    runCatching { pipe?.close() }
    inputDone = true
    inbox.put(eof)
  }

  companion object {
    const val MAX_CHUNK = 16384
    const val REVOKED = 4001
    private const val POLL_MILLIS = 1000L
  }
}

/** Hands OkHttp one [PipeSocket] per connection it wants; each opens its own Tendril session. */
class PipeSocketFactory(private val connectPipe: () -> TendrilPipe, private val watch: SessionWatch) : SocketFactory() {
  override fun createSocket(): Socket = PipeSocket(connectPipe, watch)

  override fun createSocket(host: String?, port: Int): Socket = createSocket()

  override fun createSocket(host: String?, port: Int, localHost: InetAddress?, localPort: Int): Socket = createSocket()

  override fun createSocket(host: InetAddress?, port: Int): Socket = createSocket()

  override fun createSocket(address: InetAddress?, port: Int, localAddress: InetAddress?, localPort: Int): Socket = createSocket()
}
