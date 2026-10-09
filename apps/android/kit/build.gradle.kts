// :kit — pure Kotlin/JVM library shared by :app. NO android.* imports allowed here so the
// JNI contract, input packing, tile planning, export sheet and save logic are testable on a JVM.
// Runtime dependencies: kotlin-stdlib only (small audit surface). Tests: JUnit 5.

import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.kotlin.jvm)
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
        allWarningsAsErrors.set(true)
        freeCompilerArgs.add("-Xjdk-release=17")
    }
}

dependencies {
    testImplementation(platform(libs.junit.bom))
    testImplementation(libs.junit.jupiter)
    testRuntimeOnly(libs.junit.platform.launcher)
}

tasks.withType<JavaCompile>().configureEach {
    options.release.set(17)
}

tasks.test {
    useJUnitPlatform()
    // Optional JNI integration test: pass -Pveskra.ffi.lib=/abs/path/libveskra_ffi.so
    // (or -Dveskra.ffi.lib=...). When absent the test is reported SKIPPED, never PASSED.
    val ffiLib = providers.gradleProperty("veskra.ffi.lib")
        .orElse(providers.systemProperty("veskra.ffi.lib"))
        .orNull
    if (ffiLib != null) systemProperty("veskra.ffi.lib", ffiLib)
    // Lets the copy-audit test scan the Android string resources for forbidden wording.
    systemProperty(
        "veskra.app.strings",
        rootProject.file("app/src/main/res/values/strings.xml").absolutePath,
    )
    testLogging {
        events("passed", "skipped", "failed")
        showStandardStreams = false
    }
}
