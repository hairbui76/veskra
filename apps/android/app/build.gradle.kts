// :app — Android shell (Kotlin + Jetpack Compose, Material 3).
// UNVERIFIED IN THIS ENVIRONMENT: the Android SDK host is blocked, so this module has never been
// configured or compiled here. settings.gradle.kts only includes it when an SDK is present.

import org.jetbrains.kotlin.gradle.dsl.JvmTarget

// Release signing comes from outside the repository: environment variables (CI secrets) or the
// user-level ~/.gradle/gradle.properties. Without them the release APK is built unsigned.
fun signingValue(name: String): String? =
    System.getenv(name)?.takeIf(String::isNotBlank)
        ?: (project.findProperty(name) as String?)?.takeIf(String::isNotBlank)

val releaseStoreFile = signingValue("VESKRA_RELEASE_STORE_FILE")

plugins {
    id("com.android.application") // version from the root buildscript classpath (libs.versions.agp)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

android {
    // PROVISIONAL namespace/applicationId pending trademark + namespace approval
    // (README.md "About the name", ADR-002). Do not publish under this id.
    namespace = "org.veskra.dev"
    compileSdk = libs.versions.compileSdk.get().toInt()
    // AGP 8.13 default NDK (r27); pinned so cargo-ndk and AGP agree (scripts/build-rust.sh).
    ndkVersion = "27.0.12077973"

    defaultConfig {
        applicationId = "org.veskra.dev"
        minSdk = libs.versions.minSdk.get().toInt()
        targetSdk = libs.versions.targetSdk.get().toInt()
        versionCode = 2
        versionName = "0.1.0-alpha.2"
        ndk {
            // arm64-v8a: launch ABI (MASTER_PLAN §1.3). x86_64: emulator/dev only.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        if (releaseStoreFile != null) {
            create("release") {
                storeFile = file(releaseStoreFile)
                storePassword = signingValue("VESKRA_RELEASE_STORE_PASSWORD")
                keyAlias = signingValue("VESKRA_RELEASE_KEY_ALIAS")
                keyPassword = signingValue("VESKRA_RELEASE_KEY_PASSWORD")
            }
        }
    }

    // One APK per ABI (each carries only its own libveskra_ffi.so) plus a universal one.
    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = true
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.findByName("release")
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        jniLibs {
            // Keep .so files uncompressed + page aligned in the APK (no extraction at install).
            useLegacyPackaging = false
        }
    }

    lint {
        abortOnError = true
        warningsAsErrors = false
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(project(":kit"))

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.input.motionprediction)
    implementation(libs.kotlinx.coroutines.android)

    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.ui.tooling.preview)
    debugImplementation(libs.androidx.compose.ui.tooling)
}
