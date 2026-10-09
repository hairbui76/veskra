# Veskra — Master Development Plan

**Document status:** Execution baseline / agent handoff, revision 1.0  
**Prepared:** 2026-10-10  
**Product:** Veskra — open-source, offline-first drawing and painting app for Android tablets  
**Primary implementation:** Rust engine + Kotlin / Jetpack Compose Android shell  
**Delivery model:** Incremental, vertical-slice development; agent-parallel work under a single integration owner  
**Related files:** `ARCHITECTURE.md`, `ROADMAP.md`, `VERIFICATION_MATRIX.md`, `AGENTS.md`, `AGENT_ASSIGNMENTS.md`, `DECISIONS_AND_RISKS.md`

> **Execution rule:** This plan is an implementation specification, not a claim that a working Veskra codebase, validated dependency, or native `.ai` writer already exists. Every library recommendation is a candidate until its spike passes. Acceptance gates, not phase names or arbitrary dates, authorize release.

---

## 0. Executive directive

Build **Veskra**, a fast, pleasant, color-managed, pen-first sketching and coloring application optimized for landscape and portrait Android tablets. Prioritize a drawing feel comparable to Infinite Painter, while offering dependable PSD interchange, Illustrator-compatible editable-vector output, and significantly better control and testing of color fidelity.

**The product must be:**

1. **Sketch-first** — minimal chrome, low perceived latency, expressive pressure-sensitive brushes, one-hand shortcuts, multi-touch navigation.
2. **Color-conscious by design** — explicit working spaces, managed previews, predictable blending, correct profile conversion/embedding, measured cross-application results.
3. **Hybrid raster/vector** — user strokes retain geometry and input metadata when possible; textured pixel appearance is cached faithfully; users choose editable vector and/or appearance-preserving raster at export.
4. **Useful offline** — no account, cloud dependency, advertising, or forced telemetry.
5. **Open-source and agent-maintainable** — modular crates, stable commands, headless tests, defensible licensing, automated checks.
6. **Honest about interoperability** — layered PSD, SVG/PDF that Illustrator opens, and an explicit optional native `.ai` research track; **never** rename a PDF to `.ai` or promise Adobe-internal editability that cannot be verified.

**Recommended build strategy:** a new independent Rust codebase; do **not** automatically fork or transliterate Krita's C++/Qt code. Evaluate Hokusai for brush behavior, PhotoCraft for architecture/possible component reuse, and PSD crates for interchange; choose by measured results and license audit. Krita's GPL code may not be copied into an MIT/Apache Veskra build without resolving licensing obligations.

### 0.1 Final product outcomes

A user should be able to:

- Open Veskra on a tablet, start a 2048- or 4096-pixel canvas, sketch immediately with stylus pressure and tilt (when supported by hardware), and zoom/rotate with fingers without stray marks.
- Create expressive pencil/ink linework, add or reorder layers, fill flat colors, paint shadows/highlights, erase, select, transform, and undo reliably.
- Save a native project and reopen it with its strokes, raster tiles, layer organization, blend settings, color profile, and other project state intact.
- Export a layered PSD that opens in supported Photoshop/Krita workflows, with a correct merged preview and ICC metadata where supported.
- Export SVG or PDF for Illustrator, with **two independent checkboxes**: editable vector paths and preserved raster appearance. Both can be selected. A third checkbox governs embedded color profiles where the format allows it.
- Recognize and understand when a brush, mask, blend operation, or export setting cannot be preserved perfectly; Veskra must not silently misrepresent a raster stroke as editable vector or an exported file as native AI.

### 0.2 Primary audience

- Digital sketchers and illustrators on pen-enabled Android tablets.
- Concept artists who draw roughs, ink, flat-color, and finish work.
- Designers who finish or hand off artwork in Illustrator/Photoshop.
- Open-source users prioritizing local control and portable files.

### 0.3 Priorities

| Rank | Requirement | Reason |
|---|---|---|
| P0 | Stylus feel, touch/palm behavior, responsive canvas | Core value proposition |
| P0 | Project durability: save/reopen/recovery/undo | Users must trust their work |
| P0 | Color correctness and explicit ICC semantics | Key differentiation from perceived color shifting |
| P0 | Layered PSD + Illustrator-compatible SVG/PDF | End-to-end user workflow |
| P1 | Coloring tools, brush preset UX, hybrid vectors | Completes creation workflow |
| P1 | Battery/memory, accessibility, release reliability | Real daily-use quality |
| P2 | Advanced brush lab, complex filters, CMYK print, direct native AI writer | Attractive but not required to ship 1.0 |

---

## 1. Scope and release contract

### 1.1 Version 1.0 — committed capabilities

**Canvas / input**
- Android tablet-first interface; portrait and landscape; edge-to-edge; adaptive layouts; keyboard optional.
- Stylus draw, pressure, tilt where exposed, hover cursor where exposed, configurable stylus-button action, finger pan/zoom/rotate, palm-rejection policy, accidental-touch prevention.
- Pencil, technical pen, marker, round soft brush/airbrush, textured pencil/charcoal, eraser. Brush size/opacity/flow/spacing and preset favorites; continuous smooth stroke with stabilization toggle.
- Canvas creation with dimensions/DPI/background; pinch zoom and rotation; fit-to-canvas; mirror-flip view; eyedropper.

**Document / layers / coloring**
- Native `.veskra` project with a documented, versioned manifest and recoverable saves.
- Raster layers, vector-eligible stroke layers, groups, opacity, visibility, order, rename, duplicate, locking, alpha-lock, clipping behavior and a compact supported blend-mode set.
- Brush color, palette, sRGB hex/RGB entry, swatches; bucket fill with adjustable tolerance/contiguous mode; selection (rectangle, ellipse, lasso), selection transform and move; undo/redo with coherent command boundaries.
- At least: Normal, Multiply, Screen, Overlay blending after cross-application tests; warn/fallback for unsupported PSD effects.

**Color**
- Version 1.0 default working color space **sRGB**. Store color-space identity and ICC profile metadata explicitly from the first schema version; avoid silently assigning a wrong profile.
- Managed path for imported artwork and color swatches; separate canvas working values, display transform, and export transform.
- sRGB PNG output with standards-compliant color labeling/embedded ICC profile as applicable; correct PSD ICC resource writing; RGB/ICC in PDF when the PDF library and Illustrator tests prove interoperability.
- Wide-color **Display P3 preview/export** is a *gated optional* 1.x capability. No claim of color accuracy on every device without device/profile verification. See section 6.

**Interchange**
- PNG flat export with transparency and resolution; native `.veskra` save/import; PSD export with layer tree, opacity, masks where supported, merged composite/thumbnail, and ICC.
- SVG and PDF Illustrator-compatible export: preserve supported vector paths; preserve appearance via raster embed; emit explicit warnings about flattened and unsupported content.
- Export settings with three independent toggles; remember chosen settings by destination without silently changing output semantics.

**Operational**
- Crash-safe autosave/recovery, background/lifecycle handling, file permission handling (Storage Access Framework), backup/export destination failure UX.
- English UI initially, localized-string infrastructure, accessibility semantics, clear permissions/privacy, user-accessible license and acknowledgements.
- Source builds, reproducible CI to reasonable extent, verified Android artifacts, changelog and user-facing compatibility documentation.

### 1.2 Explicitly deferred beyond 1.0

- Direct writing of native `.ai` with equivalent Illustrator-private structures.
- Full PSD edit/import fidelity (PSD import can be read-only or limited in later milestone; **export** is the 1.0 contract).
- CMYK paint mode, proofing for press, Pantone/spot colors, full print preflight.
- True watercolor pigment diffusion, advanced smudge, wet mixing, liquify, non-destructive adjustment stack, Photoshop effects, sophisticated vector boolean ops, typography, perspective rulers, animation and cloud collaboration.
- Desktop/iOS release and plugin marketplace.

**Full-project horizon:** follow-on version 1.1–2.x may add PSD import, optional P3 workspace, CMYK soft proof, improved brush realism, richer vector tools and desktop portability. The architecture must permit these additions without promising them for v1.0.

### 1.3 Target platform contract

- **Launch:** Android tablets with hardware stylus, primary ABI `arm64-v8a`, landscape/portrait.
- **Proposed minimum:** Android 10 / API 29; revisit only after a device/support survey and successful build spike.
- **Google Play target as of this plan:** API 36 (Android 16) for new app submissions from 2026-08-31, subject to renewed check at release.
- **Optional:** `x86_64` Android emulator and desktop Rust-only test harness; phone UI is not a 1.0 optimization target.
- Hardware verification matrix must include at least two different tablet GPU/vendor families, a 60 Hz class and a high-refresh-rate class if hardware is available; do not claim physical-device coverage when using emulators.

---

## 2. Product behavior and UX specification

### 2.1 Product structure

1. **Gallery / home:** Recent files, New Canvas, Open, recoverable sessions, storage location, export history (local only).
2. **Editor:** Full canvas as primary content; floating minimal left tool rail; bottom configurable quick actions; layer panel and brush/color sheet drawers; export/share menu.
3. **New Canvas:** Presets (square, portrait, landscape, custom dimensions, DPI), color space badge, estimate of potential memory cost.
4. **Brush studio:** Presets, search/favorites, pressure curve, size/flow/opacity/spacing, stabilization, pen/eraser mapping.
5. **Layer sheet:** Thumbnails, groups, visibility, opacity, lock, clipping, ordering, gestures, contextual actions, clear indication of raster vs vector-eligible content.
6. **Color panel:** Color wheel, numeric inputs, saved palettes, profile badge, eyedropper and recent colors.
7. **Export sheet:** Native project, PSD, PNG, Illustrator-compatible SVG/PDF; separate Vector / Raster / ICC controls, preview and compatibility notes.
8. **Settings / About:** Gesture mapping, input calibration, color-management diagnostics, performance mode, storage, licenses, privacy.

### 2.2 Interaction invariants

- Canvas remains visible and sketchable when panels are dismissed.
- Touch controls use >= 48 dp targets; pen hit areas may be narrower but accessible equivalent controls must exist.
- Distinguish stylus and finger; when stylus draws, touch navigates by default; disable accidental marks from palm contact.
- Two-finger gestures never trigger new brush strokes. Stylus may continue painting while fingers change view only when explicitly permitted and tested.
- Undo boundaries are **per committed stroke or command**, never per MotionEvent packet.
- Predicted points are presentation-only, never persisted or exported; replace them with confirmed points when events arrive.
- An interrupted stroke (screen lock, process kill, app background) is consistently committed or cancelled with clear recovery logic.
- Screen rotations and window-size changes do not drop the document or unexpectedly reset zoom.
- Support standard system Back and file-picker behavior; responsive split-screen/multi-window is tested.

### 2.3 Illustrator export sheet — exact UI behavior

Label: **Export for Illustrator**. File format: **SVG** or **PDF** (not native `.ai`).

- `[x] Editable vector strokes` (`includeVectorPaths`, default true)
  - Emit paths for inherently vector strokes or supported geometry from stroke metadata.
  - Variable-width strokes may become editable filled-outline paths rather than pressure-aware brush objects.
  - Unsupported textured/smudge strokes cannot be reconstituted as editable vectors. Inform the user before export.
- `[x] Preserve original appearance` (`includeRasterAppearance`, default true)
  - Embed an appropriately composed raster representation for brush textures / unsupported effects; maintain placement, alpha and visible appearance.
  - **Do not accidentally double-render** strokes duplicated across vector and raster. When both are enabled, the appearance layer becomes the visible fallback/reference while editable vector paths are clearly separated and tagged/layered as edit resources; provide a visibility policy. Validate in Illustrator.
- `[x] Embed color profile / color information` (`embedColorProfile`, default true)
  - Write meaningful ICC metadata where supported. For SVG, use supported sRGB color semantics plus ICC-tagged embedded raster assets; do not assert universal SVG ICC-profile interchange.
  - If a format/library cannot preserve the profile, disclose conversion to sRGB and state the limitation in the export summary.

Validation: at least one of the first two options must be true. If neither, block export with actionable explanation. Display counts before export: `N vector paths`, `M raster-only strokes`, `K rasterized layers`, `profile: sRGB` and warnings. Checkbox state is stored as typed export preferences.

### 2.4 Artwork scenarios

- **S1 Sketch:** Pencil rough on one layer; erase/recover; palm rests on glass, two-finger view changes.
- **S2 Ink + color:** Separate line, flats and shadow layers; selection fill, alpha lock, clipping, Multiply layer.
- **S3 PSD handoff:** 12-layer RGB/sRGB document with transparency exports to PSD; expected layer names, visual composite, dimensions and ICC match.
- **S4 Illustrator vector handoff:** Brush ink lines become editable paths in SVG/PDF; artist can select/recolor/edit in Illustrator. Pressure-varying strokes must preserve silhouette, even if brush semantics change.
- **S5 Illustrator appearance handoff:** Textured painting exports with raster appearance matched to the source; layers keep placement, correct canvas size, resolution and transparent background.
- **S6 Hybrid:** Both vector and raster are present, with no doubled artwork in initial appearance; Illustrator can hide the raster fallback to work on editable paths.
- **S7 Color sanity:** Known ICC swatches compared across Veskra, PNG viewer, Photoshop and Illustrator under controlled conditions; unexpected profile shifts are diagnosed not ignored.
- **S8 Recovery:** Force-stop, storage full, provider failure, rotation, tablet sleep and GPU loss do not silently destroy the latest committed project checkpoint.

---

## 3. Recommended high-level architecture

```text
Android tablet — Kotlin / Jetpack Compose
  Gallery | Editor chrome | Brush/Layer/Color sheets | Export dialogs
    |
    | explicit typed API over JNI; opaque document/session handles
    v
Rust service/engine boundary (UI-independent)
  Commands + document session + async jobs + error model
    |
    +-- veskra-doc        layers, vector-eligible strokes, metadata, history
    +-- veskra-input      confirmed samples, stroke normalization, gesture separation
    +-- veskra-brush      preset adapter, CPU brush oracle, stroke replay/cache
    +-- veskra-raster     sparse tiled layers, edits, masks, alpha
    +-- veskra-vector     paths, variable-width outline, SVG geometry
    +-- veskra-color      ICC/color spaces, conversions, blend-space policy
    +-- veskra-compose    deterministic CPU compositor and golden reference
    +-- veskra-gpu        wgpu/Vulkan prototype, canvas tiles, viewport
    +-- veskra-format     versioned .veskra project, autosave, checkpoints
    +-- veskra-io         PNG, PSD, SVG and PDF exporters
    +-- veskra-ffi        Android bridge; no public UI types below boundary
    +-- veskra-cli        headless tests, inspect/convert, fixtures
```

**Thread/process model:** Kotlin UI thread handles controls; native input dispatcher publishes bounded samples to a stroke-worker; raster commits update a versioned document snapshot; renderer presents tiles from latest immutable snapshot; background IO/export workers read snapshots. No blocking disk IO or ICC transform initialization on Android main thread. GPU/context loss must not destroy authoritative CPU-side/native saved content.

**Core data principle:** a *hybrid stroke is not only a Bézier line*. Store ordered stylus samples (coordinates, times, normalized pressure, tilt, azimuth/orientation, optional rotation), chosen brush preset/version, color/pixel compositing context, and optional editable geometric representation. **Committed pixel tiles are authoritative for textured/smudge effects** because replay may depend on old underlying pixels or random seeds. Store deterministic random seeds and record provenance.

**Authoritative documents:** one command API for UI, testing, CLI and future automation. Every state change has a stable command ID, validated parameter schema, clear undo effect, and transaction semantics.

See `ARCHITECTURE.md` for contracts, crate boundaries, data layout, error semantics and algorithms.

---

## 4. Dependency selection and technical spikes

**No dependency may be selected because its README sounds impressive.** Agents must pin a commit/release, run independent tests, benchmark target hardware, inspect licenses, and submit a short ADR.

| Candidate | Why investigate | Hard questions / exit decision |
|---|---|---|
| **Hokusai** — `reearth/hokusai` | Pure-Rust `.myb`/MyPaint-inspired brush core; pluggable tiled surface; MIT/Apache-2.0 | Android arm64 build, event mapping, brush feel, 16f/ICC adapter, smudge behavior, speed, determinism; accept or implement a smaller custom brush core |
| **PhotoCraft** — `storytold/photocraft` | Agent-friendly layered Rust document, CPU/GPU composition, PSD/ICC architecture; MIT/Apache-2.0 | Actual crate independence and license notices; Android compatibility, integration effort, maturity, API stability, source provenance; use isolated crates only if clearly beneficial |
| **psd-great** — `ioma8/psd-great` | PSD/PSB parser/writer with broad structures | Git dependency, upstream activity, precise license review, ICC write, valid merged composite, Photoshop/Krita opening, acceptable bugs; do not assume PSD export is solved |
| **PhotoCraft PSD implementation** | Alternate PSD encoder/decoder | Compare to psd-great on same corpus; integration surface, unit tests, legal obligations |
| **lcms2 Rust bindings** | Mature ICC transforms through LittleCMS | Android static build, safety, test vectors, performance; audit C dependency and build chain |
| **wgpu/Vulkan** | GPU tile composite and viewport acceleration | Surface lifetime and rotation; shader fidelity, color space negotiation, GPU memory and driver compatibility; CPU fallback remains required |
| **AndroidX Ink (stable 1.0)** | Platform-supported low-latency inking | Does it help a custom Rust raster/vector document, or create double stroke systems? Compare against MotionEvent + native renderer and choose one fast path |
| **lyon/kurbo/tiny-skia/resvg (candidate set)** | Vector tessellation, geometry and reference rendering | Pick small, interoperable primitives. Audit license, input safety and exported path semantics. |

**Phase-0 spike matrix** (each has a pass/fail report and artifact):

- SPK-01 Android Rust FFI demo: stylus pressure/tilt → Rust → visible stroke on tablet.
- SPK-02 Surface/GPU demo: create/destroy/recreate Vulkan surface on rotation/app pause; compare wgpu and platform-native Vulkan if needed.
- SPK-03 Hokusai vs basic pen: 1000+ sample stress, pencil/ink/eraser/smudge fixtures, CPU budgets.
- SPK-04 Color truth: sRGB swatches/gradients/alpha; ICC encode/decode; display path trace; Android wide-gamut check.
- SPK-05 PSD export: 6-layer sample including masks and alpha → Photoshop/Krita open; ICC and composite round trip.
- SPK-06 SVG/PDF export: pressure-width vector outlines, transformed groups, embedded raster, open/edit in Illustrator, no duplicate visible marks.
- SPK-07 License/security review: transitive license tree and malformed-file input boundaries.

**Selection rule:** if a spike cannot meet its gate within the agreed research budget, record a fallback and proceed with the smallest verified architecture; do not block unrelated vertical slices indefinitely.

---

## 5. Hybrid painting engine specification

### 5.1 Input/stroke model

Capture `x`, `y`, monotonic timestamp, pressure, tilt, orientation/azimuth if available, tool type, button state and device metadata. Normalize to document coordinates using a versioned viewport transform. Record raw/historical confirmed samples; process samples with adaptive filtering only for the visual/paint path. Preserve enough source data to regenerate strokes and support geometry fitting. Clamp nonsensical pressure values; provide a fallback pressure curve for non-pressure devices without fabricating real hardware support.

**Drawing states:** `Idle -> Hover -> Begin -> Streaming -> Commit|Cancel -> Idle`. `ACTION_CANCEL`, pointer changes and process lifecycle are explicit branches. No duplicate initial/down/up dabs; each stroke commits exactly once.

**Gesture arbitration:** recognized finger multi-touch transforms viewport; stylus owns the drawing path; palm events are rejected before entering persistent stroke queue. Handle low-level Android palm cancellation rather than assuming pressure threshold alone rejects palms.

### 5.2 Brush modes

- **Vector-eligible solid brushes:** pencil/ink/pen stroke centerline, fitted path, variable-width outline, stroke color and opacity. A sketch brush with pencil noise may use geometric vector core plus raster texture overlay.
- **Raster-native brushes:** airbrush, smudge, image textures, grain, water effects. Always retain committed pixels and samples; output vector only if a defined simplification mode exists with explicit fidelity warning.
- **Erasers:** raster eraser and vector-object eraser; selected mode must be clear to the user.

Initial presets should be deliberately few and excellent (rough pencil, smooth pencil, technical pen, brush pen, hard round, soft round, marker, charcoal, eraser). Preset structure includes stable IDs/version, settings, assets and optional user overrides. Never embed brush-pack assets without license attribution.

### 5.3 Color + alpha correctness

- Document ICC profile and working-space ID are required typed fields.
- Define explicitly whether data is encoded or linear, premultiplied or straight alpha **at every boundary**.
- Prefer high-precision working compositing buffers where memory allows, but match expected brush/interchange behavior through selectable blend-space policy rather than assuming all apps blend identically.
- Brush raster adapters (including Hokusai fix15 tiles) must convert to the document compositor without accidental double-gamma or double-premultiplication.
- Golden test edge cases: 0-alpha RGB, semitransparent gradients, multiplication under partial alpha, saturated reds/blues, edge anti-aliasing, repeated alpha-lock strokes and masks.

### 5.4 Layer semantics

`RasterLayer`, `VectorStrokeLayer`, `GroupLayer` as first-class schema variants; configurable opacity, visibility, locks, blend mode, transform, mask references, ordered child collections, immutable IDs. Validate group DAG/tree, unique IDs, no cycles, legal references. Support clipping as a document relationship with testable rasterization order. A vector layer can still have optional raster fallback tiles; avoid storing conflicting authoritative render states.

### 5.5 Canvas performance

Sparse tiling, dirty rectangles, tile cache, hardware GPU upload budget and bounded undo memory. **Tile size is an ADR**, not an assumed fixed truth; benchmark 64/128/256 tile sizes with brush dab density, transfer overhead and undo memory. Layer caches are evictable; project state and autosave checkpoints are not. Provide low-memory behavior (thumbnail downgrade, evict caches, suspend background preview) before OOM.

---

## 6. Color management: non-negotiable acceptance specification

### 6.1 Why users see 'shifted' colors

A perceived shift can come from wrong/missing ICC profiles, assigning instead of converting, gamma-vs-linear blending, RGB/alpha math, screen gamut handling, an app ignoring embedded profiles, PSD recompositing differently, display settings or uncalibrated hardware. **No software can promise identical perceived output on all uncalibrated screens and viewers.** Veskra promises traceable color metadata, defined transformations, and measured behavior in validated target apps/devices.

### 6.2 Internal pipeline (v1)

1. **Open/import:** inspect embedded ICC; if missing, ask or apply explicit configurable sRGB assumption and mark `assumedProfile = true` in metadata.
2. **Document:** use defined working space (v1 sRGB) and pixel encoding/alpha policy; save ID + ICC and semantic transform history.
3. **Brush/composition:** compute per the document's documented blend-space policy; compositing has a CPU reference oracle.
4. **Display:** color-convert from working space to the negotiated output/swapchain/device color space. Detect support; for unvalidated displays use correct sRGB path, not guessed P3.
5. **Export:** transform colors for destination target space before encoding; embed profile/output intent only where genuinely supported; document any flattening or gamut mapping.
6. **Round-trip:** import emitted file via independent decoder; compare numeric samples and reference renders in a *common comparison space*.

### 6.3 Quantitative quality gates (initial engineering targets; calibrate in SPK-04)

- **Opaque sRGB reference swatches:** median CIEDE2000 <= 1.0 and 95th percentile <= 2.0 after decoded exported values and reference are converted to common Lab D50/D65-converted comparison, with reference conversion explicitly documented.
- **8-bit sRGB PNG identity fixtures:** pixel channel difference <= 1 LSB on standard opaque reference pixels where no intentional transform exists; expose exact exceptions.
- **Alpha/composite fixtures:** MAE and per-edge tolerance separately stated; anti-aliasing can legitimately change interpolation but shall not introduce uncontrolled color halos.
- **PSD fixture set:** correct ICC resource, layer tree and merged composite; pass Photoshop and Krita manual inspection with reference screenshots and numerical merged-composite comparisons. Begin with normal layers, then tested blend modes.
- **Color management consistency:** no silent ICC stripping on supported destination formats; all profile fallback situations logged and visible in export summary.
- **Device visual review:** at least two physical displays and controlled brightness/color-mode instructions. Measure with calibrated instrumentation if available; screenshots are *not* sufficient proof of physical color accuracy.

These are **proposed gates**, not measured Veskra performance. QA owner must adjust thresholds in ADR only after presenting benchmark samples and human review.

### 6.4 Working profiles, proofing and future expansion

- Default sRGB simplifies cross-app handoffs and color traceability.
- Offer selectable rendering-intent/profile handling infrastructure, but P3 authoring ships only after actual P3 surface negotiation and verified export behavior. A P3-branded button without a proper wide-gamut pipeline is prohibited.
- ICC transforms may be implemented via LittleCMS Rust bindings; physical Android build and license must be verified.
- Later: CMYK/soft-proof mode using printer ICC profiles, black-point compensation and clipping warning, as a separate tracked feature, not a hidden v1 promise.

---

## 7. File formats and exchange guarantees

### 7.1 Native `.veskra`

Use a versioned documented bundle container; propose ZIP-compatible container with: `manifest.json`, `document.json` or typed binary metadata, `profiles/`, `brushes/`, `strokes/`, `tiles/`, `previews/`, and `integrity.json`. Raster tile payloads may use zstd internally if loader verifies the content. Every entry has checksums, lengths and memory limits.

- Stable project version and migration framework.
- Crash-safe two-phase save for app-private storage; for Android SAF providers, stage and validate a local temp file, copy with appropriate backup/version/recovery strategy because third-party provider writes may not be atomic.
- Autosave checkpoint/journal stored in app-private recovery directory; avoid unbounded journal growth; recovery UI shows precise timestamp/document and never silently overwrites original.
- Unknown future schema version: reject safely or open read-only; never corrupt document on open/save.
- Save/reopen equality fixture tests on document state and rendered output.

### 7.2 PSD export (v1)

- Raster layers/groups, names, visibility, opacity and supported blend modes; rasterize unsupported features individually with warning, not silent blank layers.
- Correct image dimensions, resolution, channel depths for supported profiles (first target: RGB 8-bit), transparent pixels, merged composite preview, ICC resource (PSD image resource 1039 per Adobe specification).
- Represent vector strokes with layer-specific raster results unless a verified Photoshop-compatible vector layer writer exists; Veskra's native editable geometry remains in `.veskra`.
- Independent validate/export/re-import and open-in-Photoshop/Krita test set. If unknown custom PSD features are imported in the future, preserve-or-warn semantics.
- Large PSB export is optional expansion after PSD tests.

### 7.3 SVG export (Illustrator compatible)

- Generate valid SVG groups, paths, strokes/outlines, transforms, alpha, viewBox, dimensional units.
- Convert pressure-width stroke to filled path outlines where variable width has no portable editable stroke-width semantics.
- Embed raster images for unsupported textured regions, preserving aspect and alpha; convert embedded assets to tagged sRGB.
- Separate `Editable Vector` and `Original Appearance` group semantics to avoid visual doubling. When both selected, export a clearly labeled non-default-visible reference layer/group or another tested strategy; illustrator manual test chooses one canonical display behavior.
- SVG ICC support varies: don't imply embedded whole-document ICC is universally respected; maintain sRGB color values and embed tagged raster assets.

### 7.4 PDF export (Illustrator compatible)

- Emit vector paths and pixel images with controlled transform and clip, document dimensions, embedded ICC-based color/image profile where supported by selected writer and importer.
- Test import/editability in Illustrator explicitly; a generic valid PDF is insufficient to claim editable vector handoff.
- Some Illustrator-private structure is available only when Adobe Illustrator saves its own native document with Preserve Illustrator Editing Capabilities enabled. Do not mislabel a generic PDF as an `.ai` project.

### 7.5 Direct `.ai` format research track

A dedicated R&D ticket investigates documented, legally distributable interoperability only. Gate for any direct writer: open in current Illustrator without warning, editable paths/objects, correct color, transparent masks, multi-artboard claims only if proven, byte format validation, external interoperability corpus and legal review. If not proven, status remains **unsupported**. User-facing flow: SVG/PDF → open in Illustrator → Save As `.ai`.

### 7.6 Export report

Every export returns a typed summary: `success`, `fileURI`, `format`, `profile`, `vectorObjectCount`, `rasterizedObjectCount`, `flattenedFeatureList`, `warnings`, `durationMs`, `fileBytes` and validation status. The UI shows actionable errors and an optional compatibility report. Successful save is acknowledged **only after file flush/close and validation**.

---

## 8. Reliability, performance and accessibility

### 8.1 Performance budgets (initial targets)

| Scenario | Provisional target | Method |
|---|---|---|
| Input dispatch | No work on Android main thread that blocks frames; handle historical MotionEvents | Perf trace / frame stats |
| Interactive canvas | >= 60 FPS on reference 60 Hz tablet for normal sketch and pan/zoom; no pathological long jank | Perfetto + real hardware |
| Perceived pen latency | Aim <= 35 ms median ink-follow latency on reference device; calibrate camera/measurement in SPK-01 | High-speed video / instrumentation |
| Common stroke | No lost points; stable raster update under long pressure-varying stroke | Event replay corpus |
| Startup / first stroke | Ready to sketch <= 3 s warm start on reference mid-range tablet | Repeated device benchmark |
| Project reopen | 2048px, 12 normal layers: aim <= 3 s p95 from local storage; correct preview before full tile load | Controlled test corpus |
| Autosave | No interactive pen stall; committed stroke recoverable within <= 30 s checkpoint window | Crash fault injection |
| 4096px canvas | Can draw smoothly with 20 moderately populated layers within target device's RAM budget | 30-min endurance benchmark |

Targets are **engineering goals**, not claims about existing code. Final budgets and device profiles are decided after the hardware spike.

### 8.2 Memory, battery and graphics failures

- Sparse/evictable caches, lazy tile load, bounded worker queues and proportional memory warning.
- Throttle UI preview updates without throttling authoritative stroke processing.
- GPU device loss/context recreation preserves pending native state; release buffers correctly on backgrounding.
- Long drawing session tests measure RAM growth, temperature, battery and frame pacing.

### 8.3 Accessibility / localization

- Sufficient control sizes and readable hierarchy; high-contrast tool states; left-/right-handed rail mode; screen reader descriptions for all non-canvas controls; hardware keyboard shortcuts configurable later.
- Numeric color values and labels are usable without gesture-only interactions.
- Localization resource keys from day one; English first; responsive RTL/LTR concerns logged.

### 8.4 Privacy/security

- Offline-only core operation; no hidden external network calls, no account requirement or ads.
- Handle external files as hostile: filename/path sanitization, ZIP decompression limits, maximum dimensions, ICC size limits, fuzz PSD/SVG/native parsers; avoid arbitrary URLs/embedded external resources during import.
- Avoid logging user artwork bytes and file paths; optional debug logs must be explicitly enabled and sanitized.
- Dependencies scanned with cargo-audit/cargo-deny/Gradle scanners; Android permissions minimal and purpose-limited.
- All source assets and dependencies must have recorded license/attribution before distribution.

---

## 9. Agent operating model

Use **one integration architect** with merge authority and independent agent worktrees/branches. Parallel agents own distinct crates/areas after shared contracts freeze. Agents must never cherry-pick code from Krita or other proprietary implementations; any permitted open-source reuse requires compatible license and notices.

**Suggested roles:**

- **ORCH — Architecture/integration owner:** decisions, scopes, PR reviews, build gates, cross-crate APIs, milestone approval.
- **AND — Android shell/input agent:** Compose UX, JNI bridge, stylus/palm, lifecycle, SAF.
- **BRH — Brush/vector agent:** presets, Hokusai integration, stroke geometry, vector path conversion.
- **REN — Raster/GPU agent:** tile store, CPU compositor, GPU viewport, deterministic snapshots.
- **CLR — Color science agent:** ICC pipeline, alpha/composite math, device color, numeric oracles.
- **FMT — Import/export/persistence agent:** `.veskra`, PNG, PSD, SVG/PDF writers and interoperability.
- **QAP — QA/performance/security agent:** CI, hardware instrumentation, fuzzing, benchmark gates, license audits.
- **DSN — Design/usability agent:** tablet interaction specs, accessibility, UI reviews and documentation; can pair with AND.

**Parallelism rules:**
- Phase 0 probes run in parallel **without** merging unstable APIs into main.
- ORCH publishes `Command`, `Document`, `StrokeSample`, `TileBuffer`, `ColorProfile`, `ExportRequest`, `Error` boundary contracts before agents implement feature crates.
- One crate has one primary owner at a time; integration PRs require ORCH review.
- Each task must produce tests, fixtures, update docs, benchmark evidence when relevant and a short handoff. See `AGENTS.md` and `AGENT_ASSIGNMENTS.md`.
- No 'agent finished' claim counts until CI and applicable device/interoperability gates pass.

### 9.1 Definition of Ready (DoR)

An issue is assignable only when it has an owner role, business outcome, exact crate/surface scope, dependencies, interface contract, test/fixture requirements, acceptance criteria, risk level, and explicit exclusions.

### 9.2 Definition of Done (DoD)

1. Compiles on pinned toolchain; target host and Android if relevant.
2. `cargo fmt`, `clippy -D warnings`, unit/integration tests and relevant Gradle checks pass.
3. Golden fixtures and regression cases added for behavioral changes.
4. API/schema/docs updated; serialization is backwards compatible or migrated.
5. No unreviewed dependencies, copyright assets, unsafe blocks, broad permissions, or silent format fallbacks.
6. Behavior proved on physical hardware where stylus/performance/color-display integration is involved.
7. PR includes reproducible test commands, before/after measurements/screenshots when applicable, limitations and review link.
8. Integration owner accepts and merges; roadmap task state updated.

---

## 10. Program roadmap and gates

Detailed agent-ready task IDs, dependencies, outputs and acceptance criteria are in `ROADMAP.md`. These phases are **dependency-driven**, not a claim that all work fits an arbitrary calendar.

| Phase | Mission | Gate |
|---|---|---|
| **P0 Discovery/Feasibility** | Physical stylus, Rust/Android, GPU, Hokusai, ICC, PSD, SVG/PDF, licenses | Evidence-backed ADRs; no blocked cornerstone |
| **P1 Foundation + vertical slice** | CI, Rust workspace, Kotlin app, JNI, document commands, pen-to-tile-to-screen | Sketch line visible on tablet; save/reopen simple drawing |
| **P2 Sketch excellence** | Pencil/ink/eraser, input arbitration, latency, gestures, brush/preset UI | Can sketch for 30 min without lost strokes or jarring UI |
| **P3 Layers and coloring** | Layers/groups, fill, selection, color UI, blending, editing | Ink + flats + shadows workflow passes |
| **P4 Reliability + color management** | Native format, autosave/recovery, ICC transforms, color oracles | Fault-injection recovery and numeric color gates pass |
| **P5 Interoperability** | PSD, PNG, SVG/PDF hybrid export, export UI, compatibility corpus | Photoshop/Krita/Illustrator round trips pass documented matrix |
| **P6 Optimization + device coverage** | GPU, memory/latency/battery, profile/device tests | Reference devices meet signed-off NFR thresholds |
| **P7 Beta + release** | Accessibility, docs, fuzz/security, Play/F-Droid packaging, maintenance | Release candidate fully passes DoD and no P0/P1 open bugs |
| **P8 Post-1.0 growth** | Optional PSD import, P3 workspace, CMYK proof, advanced brushes, AI feasibility | Independent ADRs and new explicit compatibility gates |

**Indicative planning only:** 1–2 weeks for discovery spikes, then 4–6 months for a substantial beta with a coordinated team and access to test hardware; schedule must be re-estimated at P0. An autonomous agent fleet without human Illustrator/Photoshop/device validation cannot independently guarantee final color or file-format fidelity.

---

## 11. Dependency chain / critical path

```text
P0 contracts + hardware + license clearance
  -> P1 Android FFI/input/surface vertical slice
  -> P2 authoritative stroke/raster model + responsive brush
  -> P3 layer/selection/color model
  -> P4 native file + ICC reference pipeline (can partially overlap P3)
  -> P5 PSD + SVG/PDF (format probes begin at P0; full integration waits for model)
  -> P6 interoperability + real device/color audits
  -> P7 release gates
```

**Critical-path risks:** (1) Android GPU surface stability/lifecycle, (2) color-management precision and display semantics, (3) authoritative tile+stroke model and undo consistency, (4) PSD cross-app interoperability, (5) vector appearance vs editability. Parallelize *probes* early; do not prematurely merge speculative abstractions.

---

## 12. Governance, measurement and sign-off

**Status cadence:** brief asynchronous daily issue updates; weekly integration demo (live tablet), one integration release every 2 weeks when feasible. Track blocked dependencies, regressions, performance numbers, color metrics, export corpus pass rates, unresolved format fallbacks and user-tested UX defects.

**Issue states:** `Backlog`, `Ready`, `In Progress`, `Blocked`, `PR Review`, `In Verification`, `Done`, `Deferred`. If benchmark/visual evidence is missing, issue remains `In Verification`.

**Architecture Decision Records (ADR):** immutable IDs and dated decisions; mandatory for license change, UI framework, pixel formats/blending, tile size, new library, color-profile semantics, native file version, PSD writer, Illustrator export policy, direct `.ai` support.

**Milestone release checklist:**
- Required use-case scenarios S1–S8 pass and are recorded.
- All P0/P1 bugs closed; P2 release exceptions have written risk sign-off.
- Frozen `.veskra` schema migrations and sample projects validated.
- File export matrix passes independent application tests; disclaimers accurate.
- Color swatches and gamma/alpha cases meet signed-off numeric gates; hardware limitations documented.
- Offline install/start/use/export tested with network disabled.
- SBOM, notices, license metadata, privacy docs and signed artifacts produced.
- Release candidate tagged, reproducible build instructions published, rollback procedure tested.

---

## 13. First instructions to the agent orchestrator

1. Read this file **plus all sibling docs**; do not jump into a UI rewrite or feature coding.
2. Create repo with pinned Rust/Android tools, issue labels, template, CI skeleton, `AGENTS.md`, ADR directory and fixture licenses.
3. Open and assign all P0 spike tickets from `ROADMAP.md` with single owners and explicit outputs.
4. Ask the Android/hardware owner to report actual supported devices before freezing latency and wide-gamut targets.
5. Produce first ADR set: license, Android floor/target, UI toolkit, renderer, brush, PSD library, working pixel format/color blend behavior, native container.
6. Merge only the smallest Android stylus -> Rust stroke -> tile -> screen -> file vertical slice after P0 gates.
7. Drive subsequent phases using task dependencies; refuse claims of native `.ai` output and universal color identity until empirically validated.

**Source references (checked 2026-10-10):**

- Hokusai project / brush model: <https://github.com/reearth/hokusai>
- PhotoCraft source / architectural candidate: <https://github.com/storytold/photocraft>
- psd-great source / limitations: <https://github.com/ioma8/psd-great>
- Android advanced stylus: <https://developer.android.com/develop/ui/compose/touch-input/stylus-input/advanced-stylus-features>
- AndroidX Ink release page: <https://developer.android.com/jetpack/androidx/releases/ink>
- Android wide color: <https://developer.android.com/training/wide-color-gamut/>
- Adobe PSD specification: <https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/>
- Illustrator saving artwork: <https://helpx.adobe.com/illustrator/using/saving-artwork.html>
- LittleCMS Rust binding: <https://docs.rs/lcms2/>
- Google Play target API requirements: <https://developer.android.com/google/play/requirements/target-sdk>
- Krita color workflow reference: <https://docs.krita.org/en/general_concepts/colors/color_managed_workflow.html>

**End of execution baseline.** Change scope through an ADR and roadmap revision rather than allowing agents to improvise incompatible architecture.
