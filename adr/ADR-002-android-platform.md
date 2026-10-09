# ADR-002: Android platform floor/target, ABIs and namespace
Status: Accepted (provisional)
Date: 2026-10-09
Owner: ORCH + AND
Related tickets: SPK-01, SPK-12, REL-04

## Context
MASTER_PLAN §1.3 proposes API 29 minimum, API 36 target, primary ABI arm64-v8a. In this execution
environment the Android SDK/NDK host (`dl.google.com`) is denied by network policy, there is no
emulator (no KVM) and no physical tablet, so no device survey or APK build is possible.

## Decision
* `minSdk 29`, `targetSdk 36`, `compileSdk 36`; ABIs `arm64-v8a` (release) and `x86_64` (emulator).
* Kotlin JVM target 17. Versions pinned in `apps/android/gradle/libs.versions.toml`.
* Application id / namespace **`org.veskra.dev`** is a *provisional placeholder* pending trademark,
  domain and store availability checks (README "About the name"). It must be changed before any
  public release; it is not a publisher identity claim.
* Rust is cross-compiled for `aarch64-linux-android` and `x86_64-linux-android`. In this environment
  we can only **type-check** (`cargo check --target aarch64-linux-android`); linking the `cdylib`
  requires the NDK.

## Alternatives considered
1. minSdk 31+ (simpler predictive-back/stylus APIs) — narrower device reach; revisit after survey.
2. minSdk 26 — larger reach but older stylus/Vulkan behaviour; not justified without data.

## Acceptance experiment
SPK-01: APK built with pinned AGP/NDK loads `libveskra_ffi.so` on an arm64 tablet; JNI typed call
succeeds; device log attached. **Status: BLOCKED** (no SDK/NDK/device in this environment).

## Consequences
Host-JVM JNI tests exercise the same Rust FFI code on x86_64 Linux as interim evidence; this is
not Android evidence.

## Rollback / fallback
Raise minSdk if the device survey shows required APIs are missing on 29–30.

## Review
ORCH 2026-10-09 (provisional). QAP must attach SPK-01/SPK-12 device evidence.
