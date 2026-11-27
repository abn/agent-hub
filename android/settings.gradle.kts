pluginManagement {
  repositories {
    google()
    mavenCentral()
    gradlePluginPortal()
  }
}

dependencyResolutionManagement {
  repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
  repositories {
    google()
    // Google's mirror of Maven Central, ahead of Central: Central answers CI runners with
    // HTTP 429, most of all on large test jars such as Robolectric's native runtime.
    maven("https://maven-central.storage-download.googleapis.com/maven2/")
    mavenCentral()
    maven {
      name = "TendrilGitHubPackages"
      url = uri("https://maven.pkg.github.com/agync/tendril")
      content { includeGroup("dev.agync") }
      credentials {
        username = System.getenv("GITHUB_ACTOR") ?: ""
        password = System.getenv("GITHUB_TOKEN") ?: ""
      }
    }
  }
}

rootProject.name = "agent-hub-android"
include(":app")
