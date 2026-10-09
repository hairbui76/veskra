// Root build script.
//
// The Android Gradle Plugin lives on Google Maven. It is placed on the ROOT buildscript
// classpath (same classloader as the Kotlin Gradle plugin, which KGP's Android integration
// requires) but only when settings.gradle.kts found an Android SDK. Without an SDK nothing
// from Google Maven is resolved.
buildscript {
    val androidSdkAvailable = gradle.extra.has("veskra.androidSdkAvailable") &&
        gradle.extra["veskra.androidSdkAvailable"] == true
    if (androidSdkAvailable) {
        dependencies {
            classpath(libs.android.gradle.plugin)
        }
    }
}

plugins {
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}
