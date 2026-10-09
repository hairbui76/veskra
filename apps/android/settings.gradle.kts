// Veskra Android build.
//
// `:kit` (pure Kotlin/JVM) is ALWAYS included so it can be built and tested on any JDK host.
// `:app` (Android) is included ONLY when an Android SDK is available, detected via
// ANDROID_HOME / ANDROID_SDK_ROOT or `sdk.dir` in local.properties. Without an SDK the
// Android Gradle Plugin (Google Maven) is never resolved, so `./gradlew :kit:test` works
// on hosts that cannot reach dl.google.com.

import java.util.Properties

pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
    }
}

rootProject.name = "veskra-android"

fun androidSdkDir(): File? {
    val fromEnv = listOf("ANDROID_HOME", "ANDROID_SDK_ROOT")
        .mapNotNull { System.getenv(it)?.takeIf(String::isNotBlank) }
        .map(::File)
        .firstOrNull { it.isDirectory }
    if (fromEnv != null) return fromEnv
    val localProps = File(settingsDir, "local.properties")
    if (!localProps.isFile) return null
    val props = Properties().apply { localProps.inputStream().use(::load) }
    return props.getProperty("sdk.dir")?.takeIf(String::isNotBlank)?.let(::File)?.takeIf { it.isDirectory }
}

val sdkDir = androidSdkDir()
// Shared with the root build script so the AGP classpath is only added when :app exists.
gradle.extra["veskra.androidSdkAvailable"] = sdkDir != null

include(":kit")
if (sdkDir != null) {
    include(":app")
} else {
    logger.lifecycle(
        "Veskra: no Android SDK found (ANDROID_HOME/ANDROID_SDK_ROOT/local.properties sdk.dir); " +
            "building :kit only. :app is NOT configured or compiled.",
    )
}
