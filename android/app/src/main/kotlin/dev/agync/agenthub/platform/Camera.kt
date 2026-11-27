package dev.agync.agenthub.platform

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.UseCase
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.fillMaxSize
import android.content.Context
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.LifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/** The camera preview that reads a QR code. It asks for the permission itself, on this screen only. */
@Composable
fun CameraScanner(onScanned: (String) -> Unit, onDenied: () -> Unit) {
  val context = LocalContext.current
  var granted by remember { mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
  val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
    granted = ok
    if (!ok) onDenied()
  }
  LaunchedEffect(Unit) { if (!granted) launcher.launch(Manifest.permission.CAMERA) }
  if (granted) ScannerPreview(onScanned)
}

/** The camera, as the preview needs it. A seam so the binding can be checked without a camera. */
internal interface CameraBinder {
  fun bind(owner: LifecycleOwner, vararg useCases: UseCase)

  fun unbind(vararg useCases: UseCase)
}

/** Hands over the camera on the main thread, once it is ready. */
internal fun interface CameraSource {
  fun get(context: Context, onReady: (CameraBinder) -> Unit)
}

/** Reads QR codes from camera frames. */
internal interface QrReader {
  fun read(image: InputImage, onCode: (String) -> Unit, onDone: () -> Unit)

  fun close()
}

private object CameraXSource : CameraSource {
  override fun get(context: Context, onReady: (CameraBinder) -> Unit) {
    val future = ProcessCameraProvider.getInstance(context)
    future.addListener({
      val provider = future.get()
      onReady(
        object : CameraBinder {
          override fun bind(owner: LifecycleOwner, vararg useCases: UseCase) {
            provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, *useCases)
          }

          override fun unbind(vararg useCases: UseCase) = provider.unbind(*useCases)
        },
      )
    }, ContextCompat.getMainExecutor(context))
  }
}

private class MlKitReader : QrReader {
  private val client = BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build())

  override fun read(image: InputImage, onCode: (String) -> Unit, onDone: () -> Unit) {
    client.process(image)
      .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull { it.rawValue }?.let(onCode) }
      .addOnCompleteListener { onDone() }
  }

  override fun close() = client.close()
}

/** What the preview holds while it is on screen, and lets go of when it leaves. */
private class Held(val preview: Preview, val analysis: ImageAnalysis, val executor: ExecutorService, val reader: QrReader) {
  var binder: CameraBinder? = null
  var disposed = false
}

@androidx.annotation.OptIn(androidx.camera.core.ExperimentalGetImage::class)
@Composable
internal fun ScannerPreview(onScanned: (String) -> Unit, source: CameraSource = CameraXSource, newReader: () -> QrReader = { MlKitReader() }) {
  val owner = LocalLifecycleOwner.current
  val held =
    remember {
      val reader = newReader()
      val executor = Executors.newSingleThreadExecutor()
      val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
      analysis.setAnalyzer(executor) { proxy ->
        val media = proxy.image
        if (media == null) proxy.close() else reader.read(InputImage.fromMediaImage(media, proxy.imageInfo.rotationDegrees), onScanned) { proxy.close() }
      }
      Held(Preview.Builder().build(), analysis, executor, reader)
    }
  // The camera is the scanner's, not the activity's: when this leaves composition the use cases come
  // off the camera, so it is not switched on again each time the app returns to the front.
  DisposableEffect(held) {
    onDispose {
      held.disposed = true
      held.binder?.unbind(held.preview, held.analysis)
      held.binder = null
      held.analysis.clearAnalyzer()
      held.reader.close()
      held.executor.shutdown()
    }
  }
  AndroidView(
    modifier = Modifier.fillMaxSize(),
    factory = { ctx ->
      val view = PreviewView(ctx)
      held.preview.surfaceProvider = view.surfaceProvider
      source.get(ctx) { binder ->
        // The camera can become ready after the scanner has gone: then there is nothing to bind.
        if (!held.disposed) {
          held.binder = binder
          binder.bind(owner, held.preview, held.analysis)
        }
      }
      view
    },
  )
}
