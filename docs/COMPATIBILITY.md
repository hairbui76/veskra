# Veskra feature compatibility matrix

**As of:** 2026-10-09 (engine 0.1.0, schema 1). **Evidence levels:** `AUTO` = automated test in this
repository; `OSS-ORACLE` = verified with an independent open-source application/library on Linux;
`HOST-JNI` = Kotlin→JNI→Rust on a desktop JVM; `NOT TESTED` = no evidence (see docs/BLOCKERS.md).
Nothing below is verified on an Android device or in Adobe applications.

## 1. Interchange formats

| Format / feature | Status | Evidence | Notes |
|---|---|---|---|
| `.veskra` save / open / autosave / recovery | Implemented | AUTO (format: 42 tests; engine round trip bit-identical) | ZIP container, schema 1, staged atomic save, integrity hashes; no zip64 (≤ 65 534 entries / 4 GiB) |
| PNG export (RGBA8, iCCP or sRGB chunk, pHYs, scaling) | Implemented | AUTO + OSS-ORACLE (Pillow 12.3, `png` crate): pixels 0 LSB, ICC bytes exact | Default options byte-identical to the pre-color-options writer (AUTO pin) |
| PNG export 16-bit RGBA (`bitDepth: 16`) | Implemented | AUTO (`png` crate decode: samples equal the 16-bit composite; a 1024-step ramp keeps 1024 levels vs 12 in 8-bit) | Pillow oracle not run for 16-bit |
| Export target profile (`profile`: sRGB / Display P3 / Adobe-RGB-compatible) for PNG, PSD, PDF; SVG rasters | Implemented | AUTO: embedded ICC bytes equal the chosen profile; pixels equal the independent `RgbTransform` reference; worst ΔE00 over 985 sRGB swatches read back through the embedded profile: 8-bit 0.56 (P3) / 0.47 (Adobe-compatible), 16-bit 0.0034 | Relative colorimetric, per-channel clipping (counted in the report), no gamut mapping. **Photoshop / Illustrator NOT TESTED**; LittleCMS oracle not run for the Adobe-compatible profile |
| PNG import (color-managed) | In progress (wave 3) | — | see `crates/veskra-io/src/import.rs` when merged |
| PSD export 8-bit RGB: layers, names (Unicode), order, visibility, opacity | Implemented | OSS-ORACLE: psd-tools 1.24 tree match; ImageMagick reads merged image | **Photoshop NOT TESTED** |
| PSD blend modes Normal/Multiply/Screen/Overlay | Implemented | OSS-ORACLE: psd-tools recomposite ≤ 1.961 (premultiplied 8-bit units) | Photoshop NOT TESTED |
| PSD groups (isolated, pass-through), clipping, layer masks | Implemented | OSS-ORACLE: psd-tools match | **Krita 5.2.2 ignores clipping and group opacity on PSD import** (reproduced with single-feature files; Krita importer behaviour) |
| PSD ICC (resource 1039), resolution (1005), merged composite | Implemented | AUTO + OSS-ORACLE: ICC bytes equal; merged = Veskra composite 0 LSB | — |
| PSD 16-bit RGB (`bitDepth: 16`: depth-16 header, `Lr16` layer block, 16-bit channels/masks/merged image) | Implemented | AUTO only: Veskra's structural reader round-trips every layer/mask/merged sample exactly | **No independent reader has opened these files yet** (psd-tools / ImageMagick / Krita / Photoshop NOT TESTED for 16-bit) |
| PSB, layer effects, group masks, thumbnails | Not implemented | — | Unsupported content is rasterized or warned, never silently dropped |
| PSD import | Not implemented (P8 NEXT-01) | — | — |
| SVG export for Illustrator: editable outline paths | Implemented | OSS-ORACLE: rsvg-convert 2.58, Inkscape 1.2.2 re-save keeps every path | **Illustrator NOT TESTED**; paths are filled outlines, not pressure brushes |
| SVG raster appearance (embedded sRGB PNG) | Implemented | OSS-ORACLE: render identical to Veskra composite (max diff 0) | With a wide-gamut `profile` only the embedded PNGs are converted/tagged; **vector fills stay sRGB values** (reported). Images are always 8-bit |
| SVG hybrid (both toggles) | Implemented: visible appearance + hidden "Editable vectors" layer | OSS-ORACLE: no double drawing (50 % stroke alpha 128 vs 192 control) | Illustrator handling of hidden groups NOT TESTED (ADR-012) |
| PDF export (vector + raster, ICCBased, optional content) | Implemented | OSS-ORACLE: qpdf 11.9, pypdf 6.17, pdftocairo 24.02 identical render | **Illustrator NOT TESTED** |
| PDF hybrid (both toggles) | Implemented: residual strategy by default (visible vectors + raster only for non-vector content) | OSS-ORACLE | Chosen because Inkscape drops hidden PDF layers on import (ADR-012 amendment) |
| Native Adobe Illustrator `.ai` | **Not supported** | AUTO: `.ai` paths rejected everywhere | Use Illustrator *File › Save As .ai* from SVG/PDF |

## 2. Color

| Feature | Status | Evidence |
|---|---|---|
| sRGB working space, embedded/generated sRGB v4 ICC | Implemented | AUTO + OSS-ORACLE (LittleCMS 2.19 opens profile; MD5 ID valid) |
| 8-bit sRGB identity (COL-002) | Implemented | AUTO: all 16 777 216 colors 0 LSB |
| sRGB↔Display P3 transforms (COL-003/008) | Implemented (tests only; P3 authoring gated, ADR-013) | OSS-ORACLE: bit-exact vs LittleCMS unoptimized; ΔE00 median 0.088 / p95 0.300 vs colour-science |
| LUT-based / CMYK ICC profiles | Rejected with typed error | AUTO |
| Display color accuracy on devices (COL-011) | NOT TESTED | No device or colorimeter |
| Blend spaces: gamma-encoded (default, Photoshop-like) and linear | Implemented | AUTO (formula tables) |
| Adobe-RGB-(1998)-compatible profile (Veskra-generated ICC v4, export target only) | Implemented | AUTO: D50 matrix within 4e-4 of the published one; sRGB primaries/white/gray convert to the published 8-bit values (channels that are 0 in theory may read 1–2: the pure 2.2 curve magnifies s15Fixed16 colorant rounding, < 0.5 ΔE00) |
| Out-of-gamut reporting on export | Implemented | AUTO: exact clipped-pixel count for a Display P3 → sRGB export; preflight flags when clipping is possible |

## 3. Editing (engine)

| Feature | Status | Evidence |
|---|---|---|
| Pressure brushes: pencil (rough/smooth), technical pen, brush pen, marker, hard/soft round (airbrush), charcoal, hard/soft eraser | Implemented | AUTO (determinism, goldens); pen feel on hardware NOT TESTED |
| Stabilization, pressure curves, prediction preview (never persisted) | Implemented | AUTO |
| Layers: raster, vector-stroke, groups; order, rename, duplicate, lock, alpha lock, clipping, opacity, blend | Implemented | AUTO |
| Selection rect / ellipse / lasso (add/subtract/intersect/invert), fill, delete, move/transform | Implemented | AUTO |
| Bucket fill (tolerance, contiguous, sample merged) | Implemented | AUTO |
| Magic wand (`selection.wand`), selection outline query | Implemented | AUTO (wand + fill selection == bucket fill; outline bounded by `maxPoints`) |
| Gradient (linear/radial, multi-stop, blend-space interpolation) | Implemented | AUTO |
| Adjustments: HSB, brightness/contrast, curves, invert | Implemented | AUTO (alpha untouched, invert exact, deterministic integer/IEEE math) |
| Distort / perspective (4-corner quad) | Implemented | AUTO (parallelogram quads equal the affine transform bit-exactly) |
| Canvas crop / extent / resample resize (stroke geometry follows) | Implemented | AUTO (pixel offsets exact, outlines offset/scaled exactly, undo/redo exact, `.veskra` round trip) |
| Camera lock (`input.config {lockView}`) | Implemented in the Rust arbiter | AUTO with synthetic traces; device NOT TESTED |
| Undo/redo (one step per stroke/command, bounded memory with explicit trim warning) | Implemented | AUTO (1000-op exactness) |
| Two-finger pan/zoom/rotate, palm/cancel handling, stylus buttons/hover | Implemented in Rust arbiter | AUTO with synthetic traces; device NOT TESTED |
| Layer masks (editing UI), group masks, smudge/wet media, text, vector node editing | Not implemented | — |

## 4. Platform

| Item | Status |
|---|---|
| Android app (`apps/android/app`, Compose, minSdk 29 / target 36) | Source complete, **never compiled** (Android SDK host blocked); no APK |
| Kotlin `:kit` library + JNI bridge | Compiles; 89 JVM tests pass incl. HOST-JNI round trips |
| Rust engine for `aarch64-linux-android` | Type-checks (`cargo check`); not linked (no NDK) |
| GPU renderer (wgpu/Vulkan) | Not implemented (ADR-010: CPU compositor + HWUI bitmap presentation) |
