# Veskra — Technical Architecture and Interface Contracts

**Status:** Proposed architecture. Must be ratified by Phase-0 ADRs.  
**Audience:** Rust, Android, file-format, color, GPU and QA agents.  
**Design principle:** A headless, testable document engine that does not know Jetpack Compose exists.

## A. Dependency directions

```text
apps/android (Kotlin/Compose, Android lifecycle, SAF, UX)
       |
       +--- crates/veskra-ffi (JNI, native window/surface adapter)
                    |
                    v
             crates/veskra-engine (commands, sessions, jobs)
              /      |         |         |         \
       v-doc     v-paint    v-color   v-compose   v-format / v-io
        |          |          |          |              |
        +------> v-core <-----+----------+--------------+
                         v-gpu [render-only adapter]

apps/cli -> veskra-engine (host headless test runner)
```

Lower-level crates must never import Android SDK, Compose, Activity, JNI or OS-specific file pickers. Keep PSD parsing independent of UI and preferably independent of document model. No module is allowed to reach into another module's private data structures to 'fix' an integration gap.

### Suggested Rust workspace layout

```text
veskra/
  Cargo.toml                     # workspace and pinned dependency versions
  rust-toolchain.toml
  AGENTS.md
  MASTER_PLAN.md
  ARCHITECTURE.md
  ROADMAP.md
  VERIFICATION_MATRIX.md
  DECISIONS_AND_RISKS.md
  agent-briefs/
  adr/
  crates/
    veskra-core/                # IDs, commands, errors, math, bounded buffers
    veskra-doc/                 # document tree, layer metadata, history
    veskra-input/               # validated samples, stroke/gesture state machine
    veskra-brush/               # Hokusai adapter or independent engine
    veskra-vector/              # geometry, fitting, variable-width outlines
    veskra-raster/              # sparse raster tiles, masks, edits
    veskra-color/               # ICC, pixel/alpha representations, transforms
    veskra-compose/             # deterministic CPU oracle
    veskra-gpu/                 # GPU renderer and viewport cache
    veskra-engine/              # service/session, command processor, jobs
    veskra-format/              # versioned .veskra and recovery format
    veskra-io/                  # PSD, PNG, SVG, PDF API adapters
    veskra-ffi/                 # JNI boundary and native Android surface
    veskra-testkit/             # golden fixtures, comparison, generators
  apps/
    android/                    # Gradle project, Kotlin, Compose, JNI integration
    cli/                        # veskra-cli convert/test/inspect commands
  tests/
    fixtures/
      color/
      brushes/
      documents/
      exports/
  scripts/
  .github/workflows/
```

Names and crate partitioning are proposals; update one ADR before codifying. Favor focused crates and typed contracts, not premature microservices.

## B. Stable domain model

### Core ID/value types

```rust
// API sketch: not yet implemented.
struct DocumentId(Uuid);
struct LayerId(Uuid);
struct StrokeId(Uuid);
struct ProfileId { icc_hash: [u8; 32], semantic_name: String }

enum PixelDepth { U8, U16, F16, F32 }
enum ChannelModel { Rgb, Gray /* extensible */ }
enum AlphaRepresentation { Straight, Premultiplied }
enum SampleEncoding { GammaEncoded, Linear }
struct PixelDescriptor {
    depth: PixelDepth,
    model: ChannelModel,
    alpha: AlphaRepresentation,
    encoding: SampleEncoding,
    profile: ProfileId,
}

struct StrokeSample {
    x_doc: f32,
    y_doc: f32,
    timestamp_ns: u64,
    pressure: f32,            // normalized after hardware calibration
    tilt_rad: Option<f32>,
    azimuth_rad: Option<f32>,
    rotation_rad: Option<f32>,
    flags: u16,               // confirmed/predicted/hover/cancel etc.
}

struct StrokeRecord {
    id: StrokeId,
    samples: Vec<StrokeSample>,
    brush_id: String,
    brush_version: u32,
    deterministic_seed: u64,
    color_profile: ProfileId,
    composition_policy: String,
    editable_geometry: Option<GeometryPath>,
    raster_authority_ref: Option<TileSetId>,
}

enum LayerNode {
    Raster(RasterLayer),
    Vector(VectorStrokeLayer),
    Group(GroupLayer),
}
```

**Important:** these examples are contracts to discuss, not code generation mandates. Physical shape changes require an ADR while several agents are active. Never serialize C memory layouts directly to `.veskra`; define a stable cross-version schema.

### Invariants

- Document size nonzero and below configured safe maximum; no arithmetic overflow when computing image buffers.
- The document owns color profile identity; layers may have imported source profiles, which are transformed to the document representation at explicit boundaries.
- Every stroke has stable identity and deterministic seed. User actions are recorded as atomic commands.
- Pixel descriptor follows tiles through every rendering/export boundary. No untyped `[u8;4]` buffers in public graphics APIs.
- Caches can be rebuilt; authoritative document and tiles cannot be discarded to recover memory.
- Layer tree must be acyclic, reference-complete, with stable deterministic order for drawing and serialization.
- Unknown future fields in native file trigger safe migration/rejection; never reinterpret silently.

## C. Command engine contract

**Why:** UI, headless tests and future bots must execute the same operations.

```text
Document.Create
Document.Open / Save / SaveAs / Recover
Stroke.Begin / AppendSamples / Commit / Cancel
Brush.Select / UpdateSettings
Layer.Add / Remove / Rename / Reorder / Duplicate / ToggleVisibility
Layer.SetOpacity / SetBlendMode / SetAlphaLock / SetClip
Selection.Set / Clear / Invert / Transform
Canvas.SetViewTransform / FitView / MirrorView
Color.SetForeground / SetWorkingProfile (future gated)
History.Undo / Redo
Export.Start / Progress / Cancel / Result
```

**Rule:** `AppendSamples` affects a live preview and a pending transaction; `Commit` enters durable undo history. History does not store every pointer packet. A failed command is atomic: no partial layer mutation.

**Engine interfaces** (conceptual):

```rust
trait CommandService {
  fn execute(&self, session: SessionId, cmd: Command) -> Result<CommandReceipt, EngineError>;
  fn snapshot(&self, session: SessionId) -> Result<Arc<DocumentSnapshot>, EngineError>;
  fn start_export(&self, session: SessionId, request: ExportRequest) -> Result<JobId, EngineError>;
}

struct CommandReceipt { document_version: u64, invalidated_regions: Vec<RectI>, warnings: Vec<EngineWarning> }
struct ExportRequest { destination: DestinationToken, format: ExportFormat, options: ExportOptions }
struct ExportOptions { include_vector_paths: bool, preserve_raster_appearance: bool, embed_color_profile: bool }
```

No export task should reference active mutable document state. It operates on an immutable snapshot with ID/version; cancelled jobs release resources and never create a file marked completed.

## D. Android input and rendering sequence

```text
MotionEvent (historical + current)
    -> Android input adapter (tool type, pointer ID, cancel, palm handling)
    -> compact confirmed StrokeSamples; motion predictions separate
    -> JNI bounded ring buffer / batch sender
    -> engine pending stroke and brush adapter
    -> dirty tiles + visual front-buffer prediction
    -> renderer upload/compose and present
    -> Stroke.Commit -> snapshot/version -> autosave queue
```

- Kotlin/Compose manages controls and panels. Prefer a dedicated surface for GPU viewport with pointer interoperability; never use standard recomposition for per-point paint pixels.
- `MotionEvent` carries historical samples; record them, with monotonic timestamps. Preserve confirmed versus predicted data distinction.
- JNI must transfer compact batches; avoid one expensive JNI call per pixel or unbounded alloc per point.
- Render surface lifecycle: `Created -> Configured -> Active -> Paused/Lost -> Recreated -> Active -> Destroyed`. All Vulkan/wgpu operations that require a live surface check generation ID and correct owner thread. Verify native handle lifetime.
- Stylus motion prediction is optional **visual-only**, not saved or passed to final pixel oracle unless corrected by confirmed samples.
- Measure latency after input collection, brush computation, GPU submission and presentation separately.
- Evaluate AndroidX Ink 1.0 as an alternative or overlay for ultra-low latency. It must not create a second authoritative stroke format or break Rust brush semantics.

### Thread ownership

| Component | Owner | May block? | Synchronization |
|---|---|---|---|
| Compose UI | Android UI thread | No | UI state/flows |
| Raw input dispatch | Android UI / input callback | No | bounded batch to worker |
| Document/commands | Rust document actor | Short commands only | serialized mutations |
| Brush rasterization | Rust worker | Yes, bounded | document generation + pending stroke |
| GPU presentation | Render thread/context owner | Only GPU operations | generation-checked render snapshots |
| Persistence/export | Background IO pool | Yes | immutable snapshot + cancellation token |
| ICC transform/cache | Rust color worker | Yes for setup | thread-safe immutable transform cache |

**Latency escape hatch:** if full stroke rasterization takes too long, present a fast low-cost temporary stroke representation while native brush tiles catch up. Never persist inaccurate speculative strokes.

## E. Tile/raster/composition

### Sparse store

`TileKey = (LayerId, x_index, y_index, mip_level)`; `Tile = (pixel descriptor, revision, dirty rect, encoded or decoded bytes, checksum)`. Budgeted LRU cache for composited outputs; clear ownership for raster-layer sources. 64/128/256 tile size is benchmark-gated.

**Undo design:** stroke-level command log plus compressed before/after dirty tile delta or copy-on-write immutable tile handles. Verify replay of strokes with smudge/base-color dependence does not drift; authoritative committed tiles back each undo checkpoint. Cap undo memory with explicit feedback, not silent incorrect undo.

**CPU compositor:** define base truth for Normal/Multiply/Screen/Overlay, opacity, masks, group composition and clipping. GPU results compared to CPU goldens with documented tolerances. Do not mix linear/gamma coordinate spaces accidentally. Transparent RGB hidden values have explicit policy; export must avoid unpremultiplication halos.

**GPU:** wgpu + Vulkan candidate. Start with 2D quad/tile display + document-to-display transform and viewport; no complex shaders until basic fidelity. Handle nearest vs bilinear zoom, high DPI, partial repaint, memory pressure, context loss. Integrate color transfer as last rendering stage; choose swapchain colorspace from negotiated support, not device marketing labels.

## F. Color pipeline and blend contracts

### Separation of concerns

1. Source profile assignment/decoding.
2. Source-to-document ICC transformation.
3. Brush dab generation in its well-defined math and encoding (Hokusai currently uses fix15 linear sRGB tiles; account for this exactly).
4. Document composition under documented per-document blend-space policy.
5. Document-to-display ICC/colorspace transform.
6. Document-to-export space conversion and metadata.

**No equivalence claim:** identical RGB numeric values do *not* guarantee identical appearance unless profile interpretation and display characteristics are aligned.

### Required tests

- sRGB colors at primaries, secondaries, neutrals, skin-like/desaturated values.
- CMYK/AdobeRGB/P3 fixture imported to sRGB when import support exists; absent ICC and corrupt ICC.
- Black/white/high saturation smooth gradients at 8-bit/16-bit.
- Alpha 0, alpha 1, alpha 0.01; nested mask and Multiply/Overlay semitransparent layers.
- Compare decoded PNG/PSD/PDF raster representations in a common Lab or linear RGB reference.
- Verify Actual Android surface color space, ICC transform application and screenshots/physical device under controlled brightness. The display transform cannot be inferred from a screenshot alone.

### Gated expansion

Only enable Display P3 authoring and wide-gamut output when a representative Android hardware + Vulkan/wgpu swapchain tests have passed. Before that, sRGB-managed defaults are more accurate than untested 'wide gamut'.

## G. Native `.veskra` container, autosave and SAF

Proposed ZIP-compatible layout:

```text
manifest.json                       schema/version/creation/app version/checksums
project/document.json              tree, IDs, profile refs, edit settings
project/history.index              undo/checkpoint indexes (optional)
profiles/<sha256>.icc               checked and bounded ICC bytes
brushes/presets.json               brush ids, versions, settings
strokes/<id>.bin                   length-delimited binary confirmed samples
raster/<layer>/<x>_<y>.tile         compressed tile with pixel descriptor
preview/composite.png              sRGB-tagged thumbnail for gallery
integrity.json                     hashes, byte lengths and resource manifest
```

Native storage is for preserving **Veskra's** editable geometry/brush metadata; it is not just an alias for PSD. Define migration `N -> N+1` and sample fixtures for old versions.

**Persistence protocol:**

1. Snapshot version `N` and encode to app-private temp path.
2. Validate manifest, checksums, size and decode on temp artifact.
3. For app-private destination: atomic rename if filesystem guarantees it; fsync relevant files/parent where supported.
4. For SAF destination: stream validated bytes to user-selected document; some providers cannot atomically replace. Keep previous project plus recovery checkpoint until copy is confirmed, and state limitations in UX.
5. Only then set `savedVersion=N`. New edits after `N` keep dirty state.

Autosave into app-private recovery log at configurable bounded interval and on lifecycle transition; checkpoint stroke transactions, never raw predicted events. Crash tests cover kill while encoding, kill while SAF copy, low storage, corrupt tile and future-version schema.

## H. Export architecture

All exporters implement a common capability handshake:

```rust
trait Exporter {
  fn capabilities(&self, doc: &DocumentSnapshot) -> ExportCapabilities;
  fn inspect(&self, doc: &DocumentSnapshot, opts: &ExportOptions) -> ExportPreflight;
  fn export(&self, doc: &DocumentSnapshot, opts: &ExportOptions, out: &mut dyn Write) -> Result<ExportReport, ExportError>;
}
```

`ExportPreflight`: supported pixel/color models, vectorizable counts, raster-only counts, flattening warnings, profile semantics, output-size estimate. `ExportReport`: what was *actually* saved, profile present/converted, flattened objects, dimensions and verification status.

**PNG:** sRGB-targeted image (explicit conversion), transparency, DPI where possible, color metadata verified on readback.

**PSD:** output 8-bit RGB initial; tree mapping; each unsupported stroke becomes a raster layer; merged composite and ICC resource 1039; readback parser test, open-in-Photoshop/Krita comparison. Use an independent fixture corpus and reject unsupported dimensions/channel counts. Group masks and blend modes only when a verified writer supports them.

**SVG:** true vector objects for fitting-supported ink/solid stroke path, raster images for textured/unsupported content. For pressure, create editable outline path. The original path centerline may be preserved in project metadata but should not be misrepresented as editable Illustrator pressure dynamics. Group exported paths in named `Editable artwork` group. For hybrid output, prefer a **visible raster appearance/reference** group plus a **hidden editable-vector** group, and verify Illustrator actually retains/imports hidden vector objects; if not, select a documented alternate strategy in an ADR. This prioritizes pixel-perfect initial appearance without double painting. If raster is unchecked, vector remains visible.

**PDF:** equivalent image/path policy, actual embedded ICC-based image/color support checked per PDF library, editability checked in Illustrator. Conformance to PDF/X, multi-artboard compatibility, and `.ai` native data are not assumed.

**Direct `.ai`:** explicitly unsupported until a dedicated feasibility proof passes. Never set export MIME, file extension or UI copy implying an Adobe Illustrator native project when it is not.

## I. Testing architecture

- `veskra-cli` implements `render-fixture`, `inspect-project`, `inspect-export`, `compare-color`, `benchmark-stroke`, `fuzz-seed` commands using the **same** engine as Android.
- Unit tests: tile math, brush sampling, pressure interpolation, alpha, RGB conversion, transform inversion, layer rules, undo atomicity.
- Golden/image tests: seeded stroke data, CPU render, GPU render and exported raster comparison.
- Interop tests: SVG/PDF edited in Illustrator; PSD opened in Photoshop/Krita; document open/reopen; ICC inspect. Store original test artwork and its license/provenance.
- Crash tests: user interruption, app lifecycle and malformed file corpus.
- Real-device tests: input lag, palm rejection, screen rotate, background, stylus-button mapping, low-memory and thermal endurance.
- Dependency/license/security: cargo-deny, cargo-audit, Gradle audit, SBOM, parser fuzzing.

See `VERIFICATION_MATRIX.md` for test IDs, thresholds, artifacts and release sign-off.

## J. Failures and API errors

Typed errors, not bare exception strings:

```text
InvalidDocument / UnsupportedSchema / CorruptTile / AllocationLimit
InvalidProfile / UnsupportedColorMode / ColorTransformFailure
UnsupportedLayerFeature / ExportLossWarning / IncompatibleOutputProfile
AndroidSurfaceLost / DeviceUnavailable / GpuOutOfMemory
FilePermissionDenied / StorageFull / ProviderWriteFailed / Cancelled
PsdEncodeError / SvgEncodeError / PdfEncodeError
```

Public error includes user-readable key, safe diagnostic code, recoverability, context and remediation, not artwork bytes or sensitive storage paths. CPU fallback should be used on GPU loss when possible without promising identical speed.

## K. Release architecture decisions to ratify

- ADR-001 OSS license (recommend MIT OR Apache-2.0 for independent code) and copyrighted resource policy.
- ADR-002 Android version floor/target, tablet test list and min device RAM.
- ADR-003 Kotlin Compose versus alternate UI; native surface interop approach.
- ADR-004 Brush adapter choice (Hokusai or simpler independent engine); own stroke schema invariant.
- ADR-005 Pixel descriptor, premultiplication, blend policy, tile size, cache budgets.
- ADR-006 Color management implementation, ICC library and supported output semantics.
- ADR-007 PSD writer selection and unsupported-layer fallback policy.
- ADR-008 Illustrator SVG/PDF hybrid visibility strategy and importer matrix.
- ADR-009 `.veskra` container, recovery semantics and migration version 1.
- ADR-010 GPU/wgpu surface strategy and fallback.

**Handoff requirement:** every feature PR cites the ADRs that define its behavior and provides tests matching those contracts.
