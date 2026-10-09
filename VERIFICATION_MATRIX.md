# Veskra — Verification, Test Corpus and Release Gates

**Purpose:** Define independent proof that Veskra works, including cases a coding agent cannot honestly self-certify.  
**Evidence labels:** `AUTO` (CI/headless), `EMU` (Android emulator), `HW` (physical pen tablet), `ADOBE` (Photoshop/Illustrator manual interoperability), `COLOR` (numeric ICC/reference suite), `SEC` (security/license).

> All values below are *proposed test targets*, not measured product results. QA must collect SPK-01/05 results, decide final tolerances in an ADR and keep the versioned fixture corpus. Never use screenshots alone to conclude physical display color accuracy.

## 1. Evidence requirements per quality gate

| Gate | Required evidence | Fail conditions |
|---|---|---|
| G0 feasibility | Android/JNI, stylus traces, brush sample, ICC oracle, PSD and SVG/PDF proofs, dependencies license audit | No physical stylus proof or no independently opening exports |
| G1 vertical slice | Real stroke -> saved `.veskra` -> reopened identical composite | Strokes lost, JNI crash, unrecoverable rotation/surface lifecycle |
| G2 sketch experience | 30-minute real drawing test, pen latency report, gesture/palm/cancel test set | Lost/duplicated strokes, frequent unintended paint, unacceptable input lag |
| G3 coloring | 12-layer reference ink/flats/shadows artwork, edit/undo/selection and hybrid classification | Incorrect compositing/order, raster stroke misrepresented as editable |
| G4 color and data durability | ICC numerical suite, project recovery fault injection and native schema tests | ICC silently dropped, colors outside accepted error budget, unrecoverable dirty work without warning |
| G5 interoperability | Actual PSD test in Photoshop/Krita and SVG/PDF vector-edit/appearance test in Illustrator | Broken file, missing composite, double rendered art, fictitious native `.ai` claim |
| G6 hardening | GPU and CPU parity, device performance, memory/thermal, parser fuzz, accessibility | Persistent rendering corruption, data loss, critical OOM or device crashes |
| G7 release | Prior gate evidence, license/privacy, fresh install, Play compliance, signed build | Open P0/P1 defect or unreviewed OSS assets/license |

## 2. Test cases with acceptance and evidence owner

### 2.1 Stylus and gestures

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| STY-001 | 1000 historical/current events keep monotonic order and normalized pressure within legal range | AUTO + HW |
| STY-002 | Tilt and azimuth preserved when reported; absent axes marked `None`, not invented | AUTO + HW |
| STY-003 | Hand/palm contact beside stylus produces no accidental committed stroke in sample scenarios | HW video + event trace |
| STY-004 | ACTION_CANCEL stops pending stroke and leaves a consistent committed state | AUTO + HW |
| STY-005 | Two-finger zoom/rotate/pan has stable anchor and no stray paint stroke | HW |
| STY-006 | Stylus button/hover mapping falls back gracefully on devices without support | HW |
| STY-007 | Predicted point never persists to native project/PSD or undo log | AUTO |
| STY-008 | At 60 Hz, sketch scenario meets final signed latency budget, with measurement method | HW instrumentation |
| STY-009 | Surface rotation/pause/resume while inking produces no GPU crash and pending state is deterministic | HW + AUTO |

### 2.2 Brush, raster, layers and history

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| PNT-001 | Same confirmed samples/seed/preset produce stable deterministic brush goldens | AUTO |
| PNT-002 | Pencil, pen, marker and charcoal sample outputs match approved qualitative/golden tests | AUTO + HW |
| PNT-003 | Brush settings migration preserves old preset IDs and defaults | AUTO |
| PNT-004 | Eraser under semitransparent pixels has no dark/colored fringe | AUTO + COLOR |
| PNT-005 | Raster tile partial writes do not alter unrelated tiles | AUTO |
| PNT-006 | Group ordering, visibility, opacity, alpha lock and mask references produce correct composite | AUTO |
| PNT-007 | Normal/Multiply/Screen/Overlay CPU composite tests match documented formulas | AUTO + COLOR |
| PNT-008 | Bucket fill tolerance/selection respects bounded region and does not exhaust memory | AUTO |
| PNT-009 | 1000 operations of undo/redo restore tile/document consistency | AUTO |
| PNT-010 | Vector-eligible stroke fitting stays within separately signed geometric error bound | AUTO |
| PNT-011 | Raster-only texture keeps committed pixels; exporting it never claims vector editability | AUTO + ADOBE |
| PNT-012 | GPU/CPU renders equivalent within approved AA/float tolerances | AUTO + HW |

### 2.3 Color fidelity (numeric, not appearance guesses)

| ID | Fixture and expected behavior | Evidence |
|---|---|---|
| COL-001 | Explicit sRGB profile remains attached to newly authored document and output | AUTO + COLOR |
| COL-002 | Known swatches round-trip through sRGB 8-bit PNG: <= 1 LSB for opaque no-conversion fixtures | AUTO + COLOR |
| COL-003 | Known opaque swatches in common Lab: CIEDE2000 median <= 1.0 and p95 <= 2.0 on reference set | AUTO + COLOR |
| COL-004 | Source with missing ICC triggers explicit assumption/metadata; corrupted ICC rejected/warned | AUTO |
| COL-005 | Assign profile and convert profile operations are visibly different and cannot silently substitute | AUTO + COLOR |
| COL-006 | Alpha 0, 0.01, 0.5, 1; unpremultiplication does not create halos | AUTO + COLOR |
| COL-007 | Gray/saturated/neutrals in smooth gradients produce no unexpected banding beyond encoded precision | AUTO + COLOR |
| COL-008 | At least one independent ICC transform implementation/reference used to compare selected fixture conversions | AUTO + COLOR |
| COL-009 | Photoshop/Krita PSD opens with embedded ICC and visibly/numerically expected composite | COLOR + ADOBE |
| COL-010 | Illustrator SVG/PDF vector/raster reference does not unexpectedly assign incompatible profile | COLOR + ADOBE |
| COL-011 | Android draw surface reports actual colorspace, does not blindly assume P3 support | HW + COLOR |
| COL-012 | Output of unsupported wide-gamut request uses documented conversion/warning, not mislabeled profile | AUTO + COLOR |

**Measurement protocol:** Keep source ICC, destination ICC, rendering intent, source sample RGB, converted XYZ/Lab values, alpha masks, reference decoder version and CIEDE2000 calculation in CSV/JSON fixtures. For physical displays, record device model, OS, display mode/brightness, any display calibration, app versions and observation method; never compare RGB screenshots from two different apps without first establishing transfer/ICC semantics. The numeric CIEDE2000 targets concern defined conversions, not all arbitrary art and monitors.

### 2.4 Persistence, autosave, crash recovery

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| SAV-001 | `.veskra` open/save/open preserves layers, strokes, profile, tiles and exact reference composite | AUTO |
| SAV-002 | Saving while drawing snapshots version N; document remains dirty for subsequent N+1 edits | AUTO + HW |
| SAV-003 | Force-stop during temp bundle write recovers prior good state + valid checkpoint | AUTO + HW |
| SAV-004 | Force-stop while SAF provider write is incomplete does not report a false successful save | AUTO + HW |
| SAV-005 | Low disk/storage quota produces actionable error; original remains recoverable | AUTO |
| SAV-006 | Truncated archive, corrupt manifest, oversized ICC, giant tile dimension rejected safely | AUTO + SEC |
| SAV-007 | Newer unknown schema version opens safely read-only or refuses without corrupting source | AUTO |
| SAV-008 | App background/restart and OS process eviction recover checkpoint as described | HW |
| SAV-009 | Loaded document's visual cache can be rebuilt from authoritative data and does not drift | AUTO |

### 2.5 PSD interchange

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| PSD-001 | RGB/sRGB 8-bit layered test file parses in independent PSD decoder | AUTO |
| PSD-002 | PSD opens in Photoshop and Krita with proper dimensions, layer names/order/visibility | ADOBE + manual Krita |
| PSD-003 | ICC image resource 1039 contains correct profile bytes | AUTO + COLOR |
| PSD-004 | PSD merged composite matches approved Veskra reference within agreed thresholds | AUTO + ADOBE |
| PSD-005 | Alpha/masks/group blends preserve supported semantics or generate per-layer degradation warnings | AUTO + ADOBE |
| PSD-006 | Unsupported filter/layer style is rasterized or fails preflight, never silently lost | AUTO |
| PSD-007 | Export failure/cancellation does not leave user-facing success or truncated final file | AUTO + HW |

### 2.6 Illustrator exchange — BOTH options

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| ILL-001 | Editable checkbox only: pressure-based solid strokes become selectable/recolorable outline paths | ADOBE |
| ILL-002 | Appearance checkbox only: textured artwork opens in correct position/scale/alpha/color in Illustrator | ADOBE + COLOR |
| ILL-003 | Both checkboxes: initial display has no double rendering and vectors remain accessible/editable | ADOBE |
| ILL-004 | Both unchecked: UI prevents export and gives useful error | AUTO + Android UI |
| ILL-005 | ICC checkbox: truthful metadata/profile behavior per selected SVG/PDF format | AUTO + ADOBE |
| ILL-006 | Unsupported vector brush shows raster fallback count/warning before user exports | AUTO + UI |
| ILL-007 | Document dimension, units, viewBox/artboard, layer group labels verified in Illustrator | ADOBE |
| ILL-008 | File is named/exported as SVG or PDF, not an unverified native `.ai` | AUTO + UI |
| ILL-009 | Illustrator File > Save As `.ai` succeeds from exported SVG/PDF on tested version; document retains verified editability level | ADOBE |

### 2.7 Performance, security, release

| ID | Scenario and expected behavior | Evidence |
|---|---|---|
| PERF-001 | Common 2048px sketch meets >=60 FPS reference device budget with frame timing | HW |
| PERF-002 | Measured p50/p95 pen latency and rendering breakdown meets signed post-spike budget | HW |
| PERF-003 | 4096px x 4096px 20 moderate layers with 30–60 min session avoids unrecoverable OOM | HW |
| PERF-004 | Save/export work on background workers never pauses pen pipeline unboundedly | HW |
| PERF-005 | 100 repeated surface destroy/recreate cycles do not leak native handles | HW |
| SEC-001 | ZIP/PSD/ICC/SVG external inputs fuzzed within CPU/memory/sandbox limits | AUTO + SEC |
| SEC-002 | License metadata covers all crate and art assets; banned unknown/GPL-to-MIT code caught | SEC |
| SEC-003 | Network off: app opens, draws, saves, exports completely offline | HW |
| SEC-004 | No app-wide storage access permission unless justified/approved; SAF tested | AUTO + SEC |
| REL-001 | Android target SDK checked against current Play policy at submission time | SEC + AND |
| REL-002 | Clean checkout pinned tools build, lint, test, sign release artifact | AUTO |
| REL-003 | Fresh install, upgrade, project migration and rollback rehearsal pass | HW |
| REL-004 | Independent beta sketcher completes S1–S6 with documented usability observations | HW/user test |

## 3. Fixture corpus structure

```text
tests/fixtures/
  LICENSES.md                       # each artwork/brush's provenance and reuse terms
  input/
    stylus_confirmed.jsonl
    stylus_cancel_and_palm.jsonl
    pen_pressure_curve.jsonl
  brushes/
    pencil_01.json + pencil_01.ref.png
    ink_taper_01.json + ink_taper_01.ref.png
    charcoal_seeded_01.json + ...
  color/
    srgb_swatches.csv
    srgb_gradients.png
    alpha_edges.png
    icc/sRGB.icc, p3-test.icc (licensed test data only)
    expected_conversions.json
  documents/
    simple-1-layer.veskra
    12-layer-illustration.veskra
    corrupt-manifest.veskra
    future-schema.veskra
  exports/
    psd-layered-srgb.psd
    vector-pen.svg
    vector-raster-hybrid.svg
    vector-raster-hybrid.pdf
    photoshop-render-reference.png
    illustrator-visual-reference.png
  hardware/
    devices.yaml                    # specs, OS, supported stylus axes
    pen_latency_report.csv
    display_color_validation.md
```

Do not commit commercial Adobe artwork or copyrighted test brush packs without written licensing permission. Public benchmark fixtures can use original/public-domain examples. Native fixture binaries are generated/reduced when possible to avoid large Git diffs.

## 4. Automated report formats

**Color report JSON**

```json
{
  "fixture": "srgb_swatches_v1",
  "input_profile": "sRGB IEC61966-2.1",
  "output_profile": "sRGB IEC61966-2.1",
  "intent": "relative-colorimetric",
  "samples": 128,
  "median_deltaE00": null,
  "p95_deltaE00": null,
  "status": "NOT_RUN"
}
```

**Interop report YAML**

```yaml
fixture: vector-raster-hybrid.svg
application: Adobe Illustrator
application_version: NOT_RECORDED
machine: NOT_RECORDED
opened_without_error: NOT_TESTED
editable_paths: NOT_TESTED
appearance_match: NOT_TESTED
icc_interpretation: NOT_TESTED
visible_duplicate_strokes: NOT_TESTED
screenshot_reference: null
reviewed_by: null
```

**Never fill metric fields with numbers you did not measure.**

## 5. Release sign-off template

```text
Release: Veskra <version>
Gate reviewed: G<number>
Source commit: <hash>
Hardware devices validated: <manufacturer/model/OS/stylus/GPU>
Automated suites: <commands/results>
Manual test apps: Photoshop <version>, Illustrator <version>, Krita <version>
Color status: <profiles/thresholds/fixtures/pass rate>
Interchange status: <PSD/SVG/PDF matrix/pass rate>
Security/license: <SBOM/audit/notice results>
Known unsupported features: <exact list>
Open blocking bugs: <exact list>
Decision: PASS / FAIL / BLOCKED
Approved by: <owner and QA reviewer>
```

**Rule:** On an untested device/app version, describe support as unverified rather than asserting universal compatibility.
