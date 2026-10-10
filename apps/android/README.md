# Veskra Android shell (`apps/android`)

Kotlin + Jetpack Compose (Material 3) shell around the Rust engine (`crates/veskra-ffi`). The engine
owns the document, painting, history and rendering; Android owns input capture, display of engine
tiles, lifecycle, Storage Access Framework (SAF) and UX.

> **Status (2026-10-10): `:kit` builds and its tests run on a plain JVM. `:app` builds
> (`assembleDebug`, `lintDebug`: 0 errors) and runs on an x86_64 tablet emulator (API 36).**
> Nothing has been run on physical hardware yet: stylus pressure/tilt, latency, palm rejection and
> multi-touch gestures are still unverified. See "Verification" below.

## Layout

```text
apps/android/
  settings.gradle.kts        :kit always; :app only if an Android SDK is found
  build.gradle.kts           AGP on the root classpath only when an SDK exists
  gradle/libs.versions.toml  pinned versions (see below)
  gradlew, gradle/wrapper/   Gradle 8.14.3 wrapper
  scripts/build-rust.sh      cargo-ndk build of libveskra_ffi.so into app/src/main/jniLibs (UNVERIFIED)
  kit/                       pure Kotlin/JVM library (package org.veskra.kit), no android.* imports
    COMMANDS.md              PROPOSED nativeExecute command vocabulary + open contract questions
    src/main/kotlin/org/veskra/kit/
      NativeBridge.kt        CONTRACTS §8 `external` JNI statics + load(); EngineBridge abstraction
      PointerPacker.kt       packed IntArray/FloatArray/LongArray layout, pooled buffers, PointerSample
      EngineSession.kt       Closeable typed wrapper, EngineResult/EngineError, InputBatchResult
      Commands.kt            JSON command builders + DocumentInfo/LayerInfo parsing
      Json.kt                minimal hand-written JSON encoder/decoder (bounded, strict)
      Export.kt              ExportFormat (PNG/PSD/SVG/PDF — no .ai), ExportOptions, preflight/report
      ExportSheetState.kt    Export sheet state machine, per-destination preferences, copy, file names
      TileUploadPlanner.kt   dirty rects → 512×512 display tiles to re-render/upload
      SaveStateMachine.kt    staged save plan (SafCopyPlan), dirty-version tracking (SAV-002), StreamCopier
      EditorModels.kt        HexColor, RecentColors, BrushPreset ids, CanvasSpec limits + memory estimate
      ColorMath.kt           HSB / Lab (D50) / naive CMYK conversions for the picker, built-in palettes
      Geometry.kt            RectI, PointF, Affine (edit.transform convention), ViewState (doc↔view mapping)
    src/test/kotlin/...      JUnit 5 tests incl. NativeBridgeIntegrationTest (skipped without a .so)
  app/                       Android application (namespace/applicationId org.veskra.dev — PROVISIONAL)
    src/main/AndroidManifest.xml   no permissions at all (no INTERNET, no storage; SAF only)
    src/main/kotlin/org/veskra/dev/
      VeskraApplication.kt, MainActivity.kt
      engine/EngineHost.kt       single HandlerThread "veskra-engine" owning the EngineSession
      engine/CanvasRenderer.kt   engine-thread tile planner + bounded direct staging buffers
      canvas/CanvasView.kt       android.view.View: input → PointerPacker → nativeInputBatch; draws tiles
      canvas/MotionEventAdapter.kt  MotionEvent (+historical, +predicted, FLAG_CANCELED) → packed samples
      canvas/CanvasDisplay.kt    UI-thread tile Bitmaps (ARGB_8888, copyPixelsFromBuffer) + view Matrix
      canvas/TextRasterizer.kt   Text tool: platform text → transparent bitmap → imported as a raster layer
      storage/ProjectStore.kt    app-private dirs, recovery checkpoint meta, SAF copy in/out
      storage/AppPreferences.kt  rail side, remembered export prefs, recent colors
      ui/VeskraViewModel.kt      state, commands, staged save, export, autosave, lifecycle
      ui/gallery/*               Gallery, New canvas dialog
      ui/editor/EditorScreen.kt  full-bleed canvas + floating chrome: Home/save state, Tools/Layers/More,
                                 Undo/Redo, bottom dock (brush, eraser, two colors), tool option bar
      ui/editor/ToolOverlay.kt   one-finger gesture → one engine command for every non-paint tool, with
                                 previews (selection, shapes, transform/distort handles, crop frame);
                                 two fingers navigate via view.set; CanvasAnnotations draws the selection
                                 outline, symmetry axes and rulers
      ui/editor/ToolsPanel.kt    Create / Edit tool grid;  BrushPanel.kt, LayerPanel.kt, ExportSheet.kt
      ui/editor/ColorPanel.kt    4 tabs: hue wheel + SV square, HSB/RGB/LAB/CMYK sliders + hex, palettes
                                 (Recent, Basics, Skin Tones, My colors), tone sphere
      ui/editor/AdjustPanels.kt  color adjust sliders, tone-curve editor, canvas resize dialog
      ui/icons                   original simple vector icons (no icon-pack dependency)
    src/main/res/values/strings.xml  every user-facing string (localization keys)
```

## Key behaviors (where they are implemented)

* **JNI contract (CONTRACTS §8)** — `NativeBridge` declares exactly the 8 `@JvmStatic external`
  functions; native symbols are `Java_org_veskra_kit_NativeBridge_<name>`. `load(libPath)` uses
  `System.load(path)` or `System.loadLibrary("veskra_ffi")`.
* **Input** — `CanvasView.onTouchEvent/onHoverEvent` → `MotionEventAdapter` packs every historical
  sample (flag bit0), the current sample, `MotionEventPredictor` output (flag bit1, presentation-only)
  and `FLAG_CANCELED` (bit2); axes the device does not report are NaN, never Android's synthetic
  defaults. Stylus `ACTION_DOWN` calls `requestUnbufferedDispatch` (API 30+). Packing uses a pooled
  `PointerPacker` (no per-event allocation); the batch runs on the engine thread, never the UI thread.
  Gesture arbitration, palm rejection and 2/3-finger undo/redo are the engine's job (veskra-input).
* **Tools** — painting goes straight to the engine (`nativeInputBatch`). Every other tool is a
  `Tool` in `VeskraViewModel`; `ToolOverlay` sits above the canvas view, converts the gesture to
  document coordinates with the inverse view matrix and sends one JSON command. Tools with a live
  result (transform, distort, color adjust, curves) keep exactly one *preview* on top of the undo
  stack: each change undoes the previous preview and applies the new parameters to the original
  pixels, so content is resampled once; Done keeps it, Cancel/Back undoes it.
* **Color picker** — sRGB only (the v1 working space). Dragging a control repaints the chrome; the
  engine gets `color.set` when the drag ends. A color joins "Recent" when it is actually used on the
  canvas. Lab values are D50; CMYK is a naive device conversion and is labelled approximate.
* **Display** — the engine renders only dirty 512×512 tiles (`TileUploadPlanner`, visible-first) into
  ≤6 reusable direct buffers; the UI thread `copyPixelsFromBuffer`s them into per-tile Bitmaps and
  draws them with `Canvas.concat(matrix from nativeGetView)`. Compose never paints per point.
* **Configuration changes** — session, tiles and state live in `VeskraViewModel`; rotation/resize are
  also declared in `configChanges` so the view is not even recreated.
* **Save (ARCHITECTURE §G)** — `SaveStateMachine`: `document.save` to `cacheDir/save-staging` →
  `document.validate` → SAF copy with `openOutputStream(uri, "wt")` on `Dispatchers.IO` (or atomic
  rename for app-private) → `savedVersion = N` only after the stream closed. Edits during a save keep
  the document dirty (SAV-002). The validated staging file is kept if the SAF copy fails.
* **Export (MASTER_PLAN §2.3)** — `ExportSheetState`: three independent checkboxes (default on);
  vector/raster only for SVG/PDF; both off → blocked with an actionable message; counts from
  `export.preflight` before export; success only for `structurally_validated` reports after the SAF
  copy closed. Label "Export for Illustrator (SVG/PDF)". No `.ai` anywhere (test-enforced, including a
  scan of `strings.xml`).
* **Lifecycle** — `ON_PAUSE` → engine thread: `input.reset`, `document.autosave` into
  `filesDir/recovery`; committed strokes also autosave after 15 s. Gallery shows a recovery card.
* **Privacy** — no `INTERNET` permission, no storage permissions, `allowBackup=false` and backup
  rules excluding all data (no artwork upload).

## Pinned versions (`gradle/libs.versions.toml`)

Constraint: Gradle **8.14.3** (wrapper). Evidence was gathered on 2026-10-09. `maven.google.com`
only 301-redirects to the blocked `dl.google.com`, so Google Maven artifacts could not be resolved or
checksummed here; their existence was verified from the official AndroidX/AGP release-note pages on
developer.android.com (reachable). Kotlin/JUnit/coroutines were verified in Maven Central /
Gradle Plugin Portal `maven-metadata.xml`.

| Item | Version | Why / evidence |
|---|---|---|
| Gradle wrapper | 8.14.3 | Required by task; `gradle wrapper --gradle-version 8.14.3`. `distributionSha256Sum` NOT set: `downloads.gradle.org` is blocked here — add it when the checksum can be fetched. |
| AGP | 8.13.2 | Last AGP line that runs on Gradle 8.x (AGP 9.x requires Gradle 9). AGP 8.13 release notes: min Gradle 8.13, max API 36.1, JDK 17, default NDK 27.0.12077973; 8.13.2 adds Kotlin 2.3 support (R8 8.13.19). |
| Kotlin (jvm/android/compose plugins) | 2.3.21 | Plugin Portal + Central metadata list 2.3.21. kotlinlang.org KGP table: 2.3.20–2.3.21 supports Gradle 7.6.3–9.3.0 and AGP 8.2.2–9.0.0. |
| compileSdk / targetSdk / minSdk | 36 / 36 / 29 | Task + MASTER_PLAN §1.3. compileSdk 37 is impossible with AGP 8.13 (max 36.1). |
| Compose BOM | 2025.12.00 | Listed on the official BOM mapping page; the per-BOM mapping table could not be fetched (frame host blocked). Chosen because Compose UI 1.10.0 (2025-12-03) is the last line before the **compileSdk 37 / AGP ≥ 9.2** requirement introduced in Compose 1.12.0-alpha01. Re-verify the mapping when the SDK host is reachable. |
| material3 | 1.4.0 | Stable 2025-09-24 (release notes); pinned explicitly over the BOM. |
| activity-compose | 1.12.4 | Stable 2026-02-11; 1.11+ compiled against API 36; predates the compileSdk 37 bump. |
| lifecycle (runtime-compose, viewmodel-compose) | 2.10.0 | Stable 2025-11-19; 2.11.0 requires Compose compileSdk 37 + AGP ≥ 9.2 (release notes). |
| core-ktx | 1.17.0 | Stable 2025-08-13; 1.18.0 moved to compileSdk 36.1. |
| input-motionprediction | 1.0.0 | Stable 2025-11-19 (androidx.input release notes). |
| kotlinx-coroutines-android | 1.10.2 | Latest stable in Central metadata. |
| JUnit (BOM, jupiter, platform-launcher) | 5.14.4 | Latest 5.x in Central metadata (task asked for JUnit 5). |

Kotlin JVM target 17 for both modules (`:kit` also compiles with `-Xjdk-release=17`).

## Building

### `:kit` (any JDK 17+ host — verified here with JDK 21)

```bash
cd apps/android
./gradlew :kit:build :kit:test --console=plain
# Optional real JNI round trip against a host build of crates/veskra-ffi:
cargo build -p veskra-ffi
./gradlew :kit:test -Pveskra.ffi.lib="$PWD/../../target/debug/libveskra_ffi.so"
```

Without `veskra.ffi.lib` the `NativeBridgeIntegrationTest` is reported **SKIPPED** (JUnit
assumption), never passed.

### `:app` (requires Android SDK with platform 36 and NDK 27.0.12077973) — UNVERIFIED

```bash
export ANDROID_HOME=/path/to/sdk           # or sdk.dir=... in apps/android/local.properties
apps/android/scripts/build-rust.sh         # cargo-ndk → app/src/main/jniLibs/{arm64-v8a,x86_64}/libveskra_ffi.so
cd apps/android && ./gradlew :app:assembleDebug :app:lint
```

`scripts/build-rust.sh` uses `cargo ndk -t arm64-v8a -t x86_64 --platform 29 -o app/src/main/jniLibs
build -p veskra-ffi --release --locked`. Without cargo-ndk, point cargo at the NDK linker instead, e.g.
for the emulator ABI: `CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=<ndk>/toolchains/llvm/prebuilt/<host>/bin/x86_64-linux-android29-clang cargo build -p veskra-ffi --release --target x86_64-linux-android --locked`
and copy `target/x86_64-linux-android/release/libveskra_ffi.so` into `app/src/main/jniLibs/x86_64/`. Generated `.so` files are git-ignored. An APK built without
the library starts and shows "Drawing engine unavailable" instead of crashing.

## Verification (honest status)

| What | Status |
|---|---|
| `./gradlew :kit:build :kit:test` | **Run here: BUILD SUCCESSFUL. 88 tests: 87 passed, 0 failed, 1 skipped** (`NativeBridgeIntegrationTest`, no `veskra.ffi.lib`). |
| JNI integration test against a real `.so` | **Not run.** `crates/veskra-ffi` (owned by another agent, in progress) did not compile at the time of writing (`cargo build -p veskra-ffi`: 7 errors in `bridge.rs`). |
| `:app` compile, lint, APK | **Run 2026-10-10 (Windows host, SDK 36, NDK 28.2 instead of the pinned 27.0):** `assembleDebug` succeeds; `lintDebug` 0 errors, 12 warnings. Only the x86_64 `.so` was built (linker env vars, no cargo-ndk), so that APK runs on the emulator only. |
| Emulator smoke test (Pixel Tablet AVD, API 36, single-pointer `adb input`) | **Run 2026-10-10:** new canvas, all brush presets, eraser, smudge, undo/redo, layers, color panel tabs, eyedropper, fill, linear/radial gradient, rect/ellipse/wand selection with outline, text, transform, distort (move), color adjust, curves, crop + undo, symmetry (vertical, radial), line guide, rectangle/circle/path shapes, camera lock toggle, save, reopen, recovery after tool edits, PNG/PSD/SVG/PDF export through SAF, 16-bit Display P3 PSD export (file header and ICC resource checked on the host). |
| Kotlin ↔ engine contract (`CommandVocabularyIntegrationTest`, JNI on the host JVM) | **Run 2026-10-10:** every `Commands` builder, all export profiles at 8 and 16 bit. `:kit:test`: 105 tests, 0 failed, 0 skipped with `-Pveskra.ffi.lib`. |
| Not exercised on the emulator | Lasso with a real loop, image import, layer masks, ellipse guide, canvas resize dialog (typing), distort by corner, two-finger navigation inside tool overlays (no multi-touch injection). |
| Device behavior: stylus, pressure/tilt, prediction, palm, multi-touch gestures, latency, rotation, lifecycle, SAF providers | **Not run:** no physical device; the emulator was driven with single-pointer `adb input`. Needs AND-03/AND-05/AND-09 hardware evidence. |
| Accessibility (TalkBack), measured 48 dp targets | **Not run.** Targets are by construction (48 dp boxes / `heightIn(48.dp)`), not measured. |
| `scripts/build-rust.sh` | **Not run — BLOCKED:** no NDK. Syntax-checked with `bash -n` only. |

Environment-only workarounds used here (NOT part of the repo): the wrapper distribution was seeded
from the locally installed Gradle 8.14.3 (downloads.gradle.org is blocked), and a user-level Gradle
init script redirected `repo.maven.apache.org` (HTTP 429 here) to `repo1.maven.org` (same Maven
Central content).

## About the name / namespace

`org.veskra.dev` (applicationId/namespace) and `org.veskra.kit` (JNI package) are **provisional**
pending trademark, domain and package-namespace approval (root README "About the name", ADR-002).
Changing the `:kit` package changes every JNI symbol name and therefore needs a CONTRACTS §8
amendment.
