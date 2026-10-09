#!/usr/bin/env bash
# Builds the Rust JNI library (crates/veskra-ffi, cdylib `libveskra_ffi.so`) for Android and places
# it where the Android Gradle Plugin packages native libraries:
#
#   apps/android/app/src/main/jniLibs/<abi>/libveskra_ffi.so
#
# UNVERIFIED: written without access to an Android NDK (dl.google.com is blocked in the environment
# this was authored in). Treat the first run on a machine with the NDK as the real test.
#
# Requirements:
#   * Rust toolchain from the repo's rust-toolchain.toml (it lists the android targets), or:
#       rustup target add aarch64-linux-android x86_64-linux-android
#   * cargo-ndk (pin the version you audit), e.g.: cargo install cargo-ndk --locked --version <X.Y.Z>
#   * Android NDK r27 (27.0.12077973, matches app/build.gradle.kts ndkVersion), via ANDROID_NDK_HOME
#     or $ANDROID_HOME/ndk/27.0.12077973.
#
# Usage:
#   apps/android/scripts/build-rust.sh            # release, arm64-v8a + x86_64
#   PROFILE=debug ABIS="arm64-v8a" apps/android/scripts/build-rust.sh
#
# Host JVM build for the :kit JNI integration test (no NDK needed):
#   cargo build -p veskra-ffi
#   (cd apps/android && ./gradlew :kit:test -Pveskra.ffi.lib="$PWD/../../target/debug/libveskra_ffi.so")
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
OUT="$ROOT/apps/android/app/src/main/jniLibs"
PROFILE="${PROFILE:-release}"
ABIS="${ABIS:-arm64-v8a x86_64}"
MIN_API=29 # == minSdk (app/build.gradle.kts, gradle/libs.versions.toml)
NDK_VERSION="27.0.12077973"

if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "error: cargo-ndk not found (cargo install cargo-ndk --locked)" >&2
  exit 1
fi

if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
  SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
  if [[ -n "$SDK" && -d "$SDK/ndk/$NDK_VERSION" ]]; then
    export ANDROID_NDK_HOME="$SDK/ndk/$NDK_VERSION"
  else
    echo "error: set ANDROID_NDK_HOME or install NDK $NDK_VERSION under \$ANDROID_HOME/ndk" >&2
    exit 1
  fi
fi

TARGET_ARGS=()
for abi in $ABIS; do
  case "$abi" in
    arm64-v8a|x86_64) TARGET_ARGS+=(-t "$abi") ;;
    *) echo "error: unsupported ABI '$abi' (arm64-v8a, x86_64)" >&2; exit 1 ;;
  esac
done

PROFILE_ARGS=()
[[ "$PROFILE" == "release" ]] && PROFILE_ARGS+=(--release)

mkdir -p "$OUT"
cd "$ROOT"
# cargo-ndk writes <OUT>/<abi>/libveskra_ffi.so. 16 KB page alignment (required for targetSdk 35+
# devices with 16 KB pages) comes from NDK r27's default linker flags; verify with
#   llvm-readelf -l libveskra_ffi.so | grep LOAD   (alignment 0x4000)
cargo ndk "${TARGET_ARGS[@]}" --platform "$MIN_API" -o "$OUT" build -p veskra-ffi "${PROFILE_ARGS[@]}" --locked

echo "Built: $(find "$OUT" -name 'libveskra_ffi.so' | sort | tr '\n' ' ')"
