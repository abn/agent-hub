package dev.agync.agenthub.platform

import android.content.Context
import androidx.activity.ComponentActivity
import androidx.camera.core.UseCase
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.lifecycle.LifecycleOwner
import com.google.mlkit.vision.common.InputImage
import dev.agync.agenthub.MainDispatcherRule
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.RuleChain
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

private class FakeBinder : CameraBinder {
  val bound = mutableListOf<UseCase>()
  val unbound = mutableListOf<UseCase>()

  override fun bind(owner: LifecycleOwner, vararg useCases: UseCase) { bound += useCases }

  override fun unbind(vararg useCases: UseCase) { unbound += useCases }
}

private class FakeReader : QrReader {
  var closed = false

  override fun read(image: InputImage, onCode: (String) -> Unit, onDone: () -> Unit) = onDone()

  override fun close() { closed = true }
}

/** The camera belongs to the scanner on screen: once it leaves, the use cases are unbound and nothing keeps the camera on. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35])
class CameraScannerTest {
  private val compose = createAndroidComposeRule<ComponentActivity>()

  @get:Rule val chain: RuleChain = RuleChain.outerRule(MainDispatcherRule()).around(compose)

  @Test fun `the camera is bound while the scanner shows and unbound exactly as it leaves`() {
    val binder = FakeBinder()
    val reader = FakeReader()
    var showing by mutableStateOf(true)
    compose.setContent { if (showing) ScannerPreview({}, source = { _, ready -> ready(binder) }, newReader = { reader }) }
    compose.waitForIdle()
    assertEquals(2, binder.bound.size)
    assertTrue(binder.unbound.isEmpty())
    showing = false
    compose.waitForIdle()
    assertEquals(binder.bound.toSet(), binder.unbound.toSet())
    assertEquals(2, binder.unbound.size)
    assertTrue(reader.closed)
  }

  @Test fun `a camera that becomes ready after the scanner left is never bound`() {
    val binder = FakeBinder()
    var ready: ((CameraBinder) -> Unit)? = null
    var showing by mutableStateOf(true)
    compose.setContent { if (showing) ScannerPreview({}, source = { _, r -> ready = r }, newReader = { FakeReader() }) }
    compose.waitForIdle()
    showing = false
    compose.waitForIdle()
    ready!!.invoke(binder)
    assertTrue(binder.bound.isEmpty())
  }
}
