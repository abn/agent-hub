import java.util.Properties

plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.compose)
  alias(libs.plugins.kotlin.serialization)
  alias(libs.plugins.roborazzi)
}

// Release signing. The key never lives in the repository: the path to a
// keystore.properties file comes from -Pagenthub.keystoreProperties=<path> or
// the AGENT_HUB_KEYSTORE_PROPERTIES environment variable, and is read at build
// time only. Without one the release build stays unsigned.
val keystorePropertiesPath: String? =
  (findProperty("agenthub.keystoreProperties") as String?)
    ?: System.getenv("AGENT_HUB_KEYSTORE_PROPERTIES")
val keystoreProperties: Properties? =
  keystorePropertiesPath?.takeIf { it.isNotBlank() }?.let { path ->
    val file = File(path)
    require(file.isFile) { "agenthub.keystoreProperties does not point at a file" }
    Properties().apply { file.inputStream().use(::load) }.also { props ->
      props.setProperty("agenthub.base", file.parentFile.absolutePath)
    }
  }

// Tendril comes from its Kotlin binding. A locally built AAR (make android/tendril)
// wins; otherwise the published coordinate is used.
val tendrilAar: File =
  (findProperty("tendrilAar") as String?)?.let(::File)
    ?: rootProject.layout.projectDirectory.file("build/tendril/tendril-android.aar").asFile
val tendrilVersion: String = (findProperty("tendrilVersion") as String?) ?: libs.versions.tendrilFallback.get()

// Where the app gets its hub from. "seeded" runs the whole app against built-in sample data (no
// network); "tendril" is the real transport. Debug builds and the screenshots default to seeded, the
// release build to tendril. -PhubTransport=... overrides both.
val hubTransportOverride: String? = findProperty("hubTransport") as String?

android {
  namespace = "dev.agync.agenthub"
  compileSdk = 37

  defaultConfig {
    applicationId = "dev.agync.agenthub"
    minSdk = 26
    targetSdk = 37
    versionCode = (findProperty("versionCode") as String?)?.toInt() ?: 1
    versionName = (findProperty("versionName") as String?) ?: "0.1.0"
    ndk { abiFilters += listOf("arm64-v8a") }
    testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
  }

  signingConfigs {
    if (keystoreProperties != null) {
      create("release") {
        val store = File(keystoreProperties.getProperty("storeFile"))
        storeFile = if (store.isAbsolute) store else File(keystoreProperties.getProperty("agenthub.base"), store.path)
        storePassword = keystoreProperties.getProperty("storePassword")
        keyAlias = keystoreProperties.getProperty("keyAlias")
        keyPassword = keystoreProperties.getProperty("keyPassword")
        storeType = "pkcs12"
      }
    }
  }

  buildTypes {
    debug {
      // A debug install never collides with a release one.
      applicationIdSuffix = ".debug"
      versionNameSuffix = "-debug"
      buildConfigField("String", "HUB_TRANSPORT", "\"${hubTransportOverride ?: "seeded"}\"")
    }
    release {
      buildConfigField("String", "HUB_TRANSPORT", "\"${hubTransportOverride ?: "tendril"}\"")
      isMinifyEnabled = true
      isShrinkResources = true
      proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
      signingConfig = signingConfigs.findByName("release")
    }
  }

  buildFeatures {
    compose = true
    buildConfig = true
  }

  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
  }

  lint {
    abortOnError = true
    warningsAsErrors = false
    checkReleaseBuilds = true
    // The test fixtures build view models outside composition on purpose.
    ignoreTestSources = true
  }

  testOptions {
    unitTests {
      isIncludeAndroidResources = true
      isReturnDefaultValues = true
      // The real-hub test needs the host build of Tendril's JNI library (make android/test-hub).
      all { test -> System.getenv("TENDRIL_HOST_LIB")?.let { test.jvmArgs("-Djava.library.path=$it") } }
    }
  }

  packaging {
    resources.excludes += "/META-INF/{AL2.0,LGPL2.1}"
  }
}

kotlin {
  compilerOptions {
    jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    allWarningsAsErrors.set(true)
  }
}

roborazzi {
  outputDir.set(layout.buildDirectory.dir("screenshots"))
}

dependencies {
  implementation(platform(libs.compose.bom))
  implementation(libs.compose.ui)
  implementation(libs.compose.ui.graphics)
  implementation(libs.compose.ui.text)
  implementation(libs.compose.foundation)
  implementation(libs.compose.material3)
  implementation(libs.activity.compose)
  implementation(libs.lifecycle.viewmodel.compose)
  implementation(libs.lifecycle.runtime.compose)
  implementation(libs.core.ktx)
  implementation(libs.coroutines.android)
  implementation(libs.serialization.json)
  implementation(libs.camera.camera2)
  implementation(libs.camera.lifecycle)
  implementation(libs.camera.view)
  implementation(libs.mlkit.barcode)
  implementation(libs.biometric)
  implementation(libs.okhttp)
  implementation(libs.commonmark)
  implementation(libs.commonmark.tables)
  implementation(libs.glance.appwidget)

  if (tendrilAar.isFile) {
    implementation(files(tendrilAar))
  } else {
    implementation("dev.agync:tendril-android:$tendrilVersion")
  }

  debugImplementation(libs.compose.ui.test.manifest)

  androidTestImplementation(libs.androidx.test.ext.junit)
  androidTestImplementation(libs.androidx.test.runner)

  testImplementation(libs.junit)
  testImplementation("org.json:json:20250517")
  testImplementation(libs.coroutines.test)
  testImplementation(libs.turbine)
  testImplementation(libs.androidx.test.core)
  testImplementation(libs.mockwebserver)
  testImplementation(libs.robolectric)
  testImplementation(platform(libs.compose.bom))
  testImplementation(libs.compose.ui.test.junit4)
  testImplementation(libs.androidx.test.ext.junit)
  testImplementation(libs.roborazzi)
  testImplementation(libs.roborazzi.compose)
  testImplementation(libs.roborazzi.junit)
}
