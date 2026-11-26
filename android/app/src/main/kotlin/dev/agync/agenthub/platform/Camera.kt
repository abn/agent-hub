package dev.agync.agenthub.platform

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
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
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
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
  if (granted) Preview(onScanned)
}

@androidx.annotation.OptIn(androidx.camera.core.ExperimentalGetImage::class)
@Composable
private fun Preview(onScanned: (String) -> Unit) {
  val owner = LocalLifecycleOwner.current
  val executor = remember { Executors.newSingleThreadExecutor() }
  val scanner = remember { BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()) }
  AndroidView(
    modifier = Modifier.fillMaxSize(),
    factory = { ctx ->
      val view = PreviewView(ctx)
      val future = ProcessCameraProvider.getInstance(ctx)
      future.addListener({
        val provider = future.get()
        val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
        val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
        analysis.setAnalyzer(executor) { proxy ->
          val media = proxy.image
          if (media == null) {
            proxy.close()
          } else {
            scanner.process(InputImage.fromMediaImage(media, proxy.imageInfo.rotationDegrees))
              .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull { it.rawValue }?.let(onScanned) }
              .addOnCompleteListener { proxy.close() }
          }
        }
        provider.unbindAll()
        provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
      }, ContextCompat.getMainExecutor(ctx))
      view
    },
  )
}
