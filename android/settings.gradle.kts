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
