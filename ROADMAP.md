# Veskra — Agent-Ready Roadmap and Ticket Backlog

**Status:** Proposed delivery backlog. All tasks are **unchecked** and not implemented.  
**Rule:** Dependency gates apply even if a downstream ticket looks easy. A task moves to `Done` only after its acceptance tests pass.  
**Labels:** `P0/1/2` = priority; `SPK` = proof / research; `ARC` = architecture; `AND` = Android; `BRH` = brush; `REN` = renderer; `CLR` = color; `FMT` = persistence / export; `QAP` = quality; `DSN` = UI/UX; `REL` = release.  
**Roles:** ORCH = integration architect; AND = Android engineer; BRH = brush/vector engineer; REN = raster/GPU engineer; CLR = color science engineer; FMT = format/storage engineer; QAP = QA, performance/security; DSN = product/UI designer.

## 0. How to assign tickets

For every ticket, copy this minimum specification into the agent task:

```text
Ticket: <ID + title>
Owner: <role> | Priority: <P0/P1/P2> | Depends on: <IDs or none>
Goal: <user-visible or measurable outcome>
Allowed scope: <owned crates, paths, interfaces>
Explicit non-goals: <what not to touch>
Implementation constraints: <relevant ADRs and package/format rules>
Acceptance checks: <specific tests, output artifacts, hardware validation>
Handoff: code commit/PR, tests, benchmark evidence, limitations, docs/ADR update.
```

**No task may be checked off because an agent says it compiled locally.** Feature readiness requires reproducible CI, direct test artifacts and any manual cross-app/device review that the ticket demands.

## Phase P0 — Feasibility, ADRs, hardware and source audits

**Goal:** Reject untenable decisions before building dependent modules. **Parallelism:** Most spikes run concurrently; ADR resolution is serialized by ORCH.

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **SPK-01** Rust → Android proof | AND | — | Minimal Gradle+NDK app loads `cdylib` for arm64; JNI passes typed call, no crash; build instruction and device log |
| [ ] **SPK-02** Stylus event truth | AND | — | Hardware report records pressure/tilt/hover/historical samples, palm/cancel/button behavior on target devices; raw event trace fixture |
| [ ] **SPK-03** GPU surface lifecycle | REN + AND | SPK-01 | wgpu/Vulkan or alternate can create/recreate surface on rotate/pause/resume; 100-cycle loop with zero handle use-after-free |
| [ ] **SPK-04** Brush candidate benchmark | BRH | — | Hokusai Android/host build, Pencil/Ink/Texture/Erase fixtures and p95 stroke cost; adopt/reject ADR proposal |
| [ ] **SPK-05** Color baseline | CLR | SPK-01 | sRGB/ICC swatches, LittleCMS Android build, GPU/display path tracing, profile tests and initial thresholds |
| [ ] **SPK-06** PSD candidate proof | FMT | — | Two independent PSD candidates tested on 6-layer file; Photoshop/Krita opening, ICC/thumbnail/masks; comparison report |
| [ ] **SPK-07** SVG/PDF + Illustrator | FMT + BRH | — | Pressure-outline example + embedded raster opens/edits in Illustrator; validate hidden editable/reference layering |
| [ ] **SPK-08** Native file/SAF failure study | FMT + AND | SPK-01 | SAF replacement behavior, interrupted write cases, recovery strategy; proof on at least local Documents provider |
| [ ] **SPK-09** AndroidX Ink vs native path | AND + BRH | SPK-02 | Compare Ink 1.0 and raw MotionEvent->Rust for latency, compatibility and double-model risk; documented selection |
| [ ] **SPK-10** Dependency/license security audit | QAP | SPK-04, SPK-06 | License table, provenance and transitive deps; flag conflicts with MIT/Apache intent; pin reviewed versions |
| [ ] **SPK-11** Tablet workflow wireframes | DSN | — | Editor/gallery/export screens with touch/palm/landscape/portrait states; tablet interaction test script |
| [ ] **SPK-12** Reference device test plan | QAP + AND | SPK-02 | Device/GPU/OS matrix; real vs emulator separation, measurement methods, hardware gaps named |
| [ ] **ARC-01** Freeze engine/UI contracts | ORCH | SPK-01, SPK-04, SPK-05, SPK-06, SPK-07 | Reviewed Doc, Stroke, Tile, Color, Export, Command and FFI schemas with API tests |
| [ ] **ARC-02** ADR-001 to ADR-010 decisions | ORCH | SPK-03…SPK-12 | Each critical decision signed, fallback and tradeoffs recorded; master docs reconciled |
| [ ] **ARC-03** Foundation build/CI skeleton | QAP + ORCH | SPK-01, ARC-01 | Pinned Rust/Android toolchains; fmt/clippy/test and Gradle build run from clean checkout |

**Gate G0:** A headless brush render, Android input proof, correct sRGB conversion demo, PSD/SVG/PDF proof and license review exist. Failure in a candidate is resolved with a documented replacement, not ignored.

## Phase P1 — Skeleton and minimum full vertical slice

**Goal:** Make a real Android pen stroke flow through the native engine, appear on screen and survive file save/reopen.

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **CORE-01** Rust workspace and cross-crate rule | ORCH | ARC-01, ARC-03 | Crates, dependency direction script, `cargo test --workspace`, ADR links and CI |
| [ ] **CORE-02** Session/Command registry | ORCH | CORE-01 | Typed `Document.Create` and session ID, command errors and immutable snapshots, unit tests |
| [ ] **CORE-03** Document v1 tree | REN + ORCH | CORE-02 | Stable IDs, simple RasterLayer, working-profile metadata, dimension bounds and serde/migration test |
| [ ] **CORE-04** Sparse tile store v1 | REN | CORE-01 | Dirty tile allocation/read/write, tile coordinate math, memory limits and golden tests |
| [ ] **CORE-05** Basic reference compositor | REN | CORE-03, CORE-04 | Normal-alpha CPU compose with generated fixture exact expected output |
| [ ] **BRH-01** Minimal pen rasterizer | BRH | CORE-04, SPK-04 | Pressure-sensitive pencil/pen on tiles, seeded deterministic output and tests |
| [ ] **CLR-01** Pixel descriptor + alpha API | CLR | ARC-01, CORE-04 | Typed depth/encoding/premultiplication/profile contract; no ambiguous buffers; freeze before blend-mode implementation |
| [ ] **AND-01** Android app shell | AND | CORE-01, SPK-01, SPK-11 | Tablet opens Gallery and Editor; app rotates; Compose controls handle Back |
| [ ] **AND-02** Safe JNI session API | AND + ORCH | CORE-02, AND-01 | Create/release native session, byte/slice lifetime rules, exception-to-error mapping, stress JNI tests |
| [ ] **AND-03** Input batch bridge | AND | AND-02, SPK-02, ARC-01 | Historical pressure event batches arrive confirmed and ordered in Rust; no per-sample heap abuse |
| [ ] **REN-01** On-screen tile presentation | REN + AND | AND-02, CORE-05, SPK-03 | Native CPU/Rust tiles visible in Android Canvas/Surface; rotation and surface loss safe |
| [ ] **FMT-01** Native v0 bundle save/open | FMT | CORE-03, CORE-04, ARC-01 | Single drawing saves and reopens, checksums and minimum manifest fixture |
| [ ] **CORE-06** Stroke atomic transaction | BRH + ORCH | BRH-01, AND-03, CORE-02 | Begin/Append/Commit/Cancel with one undo boundary and predictable callbacks |
| [ ] **INT-01** Full sketch vertical slice | ORCH | CORE-06, REN-01, FMT-01 | Physical stylus paints then saves/reopens same drawing, CI proof + device screen capture |

**Gate G1:** Real stylus -> Kotlin -> JNI -> Rust stroke -> tile -> visible canvas -> `.veskra` -> reopened identical image. A phone emulator does not substitute for tablet stylus proof.

## Phase P2 — Production-quality sketching experience

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **BRH-02** Preset schema/versions | BRH | BRH-01 | Stable preset IDs/settings/serialization; migrate unknown safe fields; asset license audit |
| [ ] **BRH-03** Pencil and ink variants | BRH | BRH-02 | Pencil (rough/smooth), ink, pressure taper, samples/goldens and physical pen feel review |
| [ ] **BRH-04** Eraser + pressure response | BRH | BRH-03 | Hard/soft eraser, editable pressure curve and edge alpha reference checks |
| [ ] **BRH-05** Stabilization/smoothing | BRH | BRH-03, AND-03 | User-toggle; replays seeded strokes; controls visible lag, no committed predicted points |
| [ ] **BRH-06** Textured pencil/charcoal adapter | BRH | BRH-02, SPK-04 | Textured brush renders deterministic raster, documents vector eligibility and memory |
| [ ] **AND-04** Two-finger view transform | AND | AND-03, REN-01 | Pan/zoom/rotate with stable anchor; no stray strokes; repeated gesture tests |
| [ ] **AND-05** Palm rejection/cancel policy | AND | AND-03, SPK-02 | Stylus/finger arbitration, palm/noise suppression and reliable ACTION_CANCEL rollback |
| [ ] **AND-06** Stylus button/hover behavior | AND | AND-03, SPK-02 | Configurable button where hardware supports, hover cursor and no false pressure assumptions |
| [ ] **REN-02** Dirty-region fast updates | REN | REN-01, CORE-04 | Invalidate/upload only changed tiles; performance traces and regression |
| [ ] **DSN-01** Brush picker + quick tools | DSN + AND | AND-01, BRH-02 | Preset list, favorites, current brush size/opacity in thumb-reachable UI |
| [ ] **DSN-02** Floating rail and panel UX | DSN + AND | AND-04, SPK-11 | Left-/right-handed modes, minimum touch targets, distraction-free canvas |
| [ ] **QAP-01** 30-minute sketch endurance | QAP | AND-05, BRH-03, REN-02 | Test report: no lost/duplicated commits, undo works, memory/latency traces |

**Gate G2:** Pencil/ink/eraser feels usable on physical tablet, gestures do not paint, 30-minute test has no unrecoverable visual/state corruption.

## Phase P3 — Painting, editing and hybrid geometry

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **DOC-01** Groups/reordering/visibility | REN | CORE-03 | Tree operations, IDs, cycle rejection, group composite golden tests |
| [ ] **DOC-02** Opacity and blend modes | REN + CLR | DOC-01, CLR-01 | Normal/Multiply/Screen/Overlay per documented math with alpha/ICC goldens |
| [ ] **DOC-03** Alpha lock/clipping/masks | REN | DOC-02 | Mask ordering/edge semantics, stable undo, reference fixtures |
| [ ] **DOC-04** Undo/redo tile delta policy | ORCH + REN | CORE-06, DOC-01 | Copy-on-write or deltas, bounded memory and exact stroke history recovery |
| [ ] **BRH-07** Marker, round and airbrush | BRH | BRH-03, DOC-02 | Raster painting with size/flow/opacity, overlap/alpha goldens |
| [ ] **EDIT-01** Bucket fill | REN | DOC-03 | Tolerance/contiguous/reference layer options, bounded memory, no infinite fill loops |
| [ ] **EDIT-02** Selections | REN | DOC-03 | Rect/ellipse/lasso add/subtract, selection mask raster and UI handles |
| [ ] **EDIT-03** Move and transform | REN + AND | EDIT-02 | Translate/scale/rotate selected content, stable preview and undo |
| [ ] **BRH-08** Persistent stroke metadata | BRH | CORE-06, DOC-01 | Confirmed samples, seed, preset/version, geometry field serialized and replay test |
| [ ] **VEC-01** Ink stroke geometry fitting | BRH | BRH-08 | Error-bounded paths generated from sample data; fit precision and speed tests |
| [ ] **VEC-02** Pressure-width outline | BRH | VEC-01 | Contour path with capped/joined ends; no self-intersection artifacts in fixtures |
| [ ] **VEC-03** Vector/raster authority rules | BRH + REN | VEC-02, DOC-03 | Editable geometry plus raster fallback without double draw, clear eligibility metadata |
| [ ] **DSN-03** Layer sheet + color picker | DSN + AND | DOC-01, BRH-07 | Group/layer thumbnails, opacity/blends, hex input, eyedropper, palettes, accessibility |
| [ ] **INT-02** Ink/flat/shadow end-to-end | ORCH | EDIT-01, EDIT-03, DOC-04, VEC-03, DSN-03 | 12-layer colored concept art saved/reopened and edited with correct undo history |

**Gate G3:** S2 Ink + Color scenario works; raster/vector classification is consistent, no unsupported raster brush is advertised as vector editability.

## Phase P4 — Trustworthiness and measurable color

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **CLR-02** ICC parsing/transforms | CLR | CLR-01, SPK-05 | Safe profile ingestion and sRGB transforms with numerical reference fixtures |
| [ ] **CLR-03** CPU color composite oracle | CLR + REN | CLR-02, DOC-02 | CIE/linear calculations, gamma policy, masks/alpha tests, documented tolerances |
| [ ] **CLR-04** Display transform/negotiation | CLR + REN | CLR-02, REN-01 | Verified sRGB output on real tablet, no assumed P3; surface metadata diagnostics |
| [ ] **CLR-05** PNG ICC/round-trip oracle | CLR | CLR-02 | Pixel/decode comparisons and output profile verification, threshold report |
| [ ] **FMT-02** Versioned native bundle | FMT | FMT-01, DOC-04, BRH-08 | Layer/stroke/brush/profile/tile metadata round-trip and migration fixture |
| [ ] **FMT-03** Atomic save/recovery log | FMT + AND | FMT-02, SPK-08 | Background saves, valid checkpoints, force-stop/storage exhaustion scenarios |
| [ ] **FMT-04** Android SAF integration | AND + FMT | FMT-03 | Create/open/copy error handling and provider-specific documented limitations |
| [ ] **FMT-05** Forward-compat/error validation | FMT | FMT-02 | Future schema safe handling, truncated/corrupt/large archive detection |
| [ ] **QAP-02** Color swatch golden suite | QAP + CLR | CLR-03, CLR-05 | Known colors, gradients, alpha/mask matrices, CIEDE2000 metrics and CI reports |
| [ ] **QAP-03** Fault injection matrix | QAP | FMT-03, FMT-04 | 100-cycle scripted save/interruption tests; no silent lost confirmed checkpoint |
| [ ] **INT-03** Color + data-loss gate | ORCH | CLR-04, QAP-02, QAP-03, FMT-05 | G4 report signed: no unresolved ICC, corrupted save, or stale-version overwrite defects |

**Gate G4:** project reliably survives expected interruptions and holds documented sRGB reference accuracy. Wide gamut remains feature-flagged until proven.

## Phase P5 — PSD, PNG, Illustrator SVG/PDF interchange

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **FMT-06** Export API/preflight/report | FMT | ARC-01, CORE-02 | Typed request, capability report, cancellation and warnings; UI/API tests |
| [ ] **FMT-07** PNG exporter | FMT + CLR | FMT-06, CLR-05 | RGB/alpha export, sRGB metadata, DPI, decode round-trip thresholds |
| [ ] **FMT-08** PSD writer adapter | FMT | FMT-06, SPK-06, DOC-03 | Valid layer tree, names, opacity, supported blends, embedded composite; 8-bit RGB first |
| [ ] **FMT-09** PSD ICC/resource handling | FMT + CLR | FMT-08, CLR-02 | Resource 1039, correct profile and image mode, source independent readback |
| [ ] **FMT-10** PSD raster fallback | FMT + REN | FMT-08, VEC-03 | Unsupported features flatten to visible raster with explicit report, no missing artwork |
| [ ] **FMT-11** PSD external corpus | QAP + FMT | FMT-10, FMT-09 | Sample PSDs open in current Photoshop and Krita; layer/composite/color logs/screenshots |
| [ ] **FMT-12** SVG editable paths | FMT + BRH | FMT-06, VEC-02 | SVG paths/groups/transforms, pressure outlines, illustrator selects/recolors |
| [ ] **FMT-13** SVG raster appearance | FMT + CLR | FMT-12, CLR-05 | Tagged sRGB PNG embeds, correct alpha/size/orientation and no doubled drawing |
| [ ] **FMT-14** PDF vector/raster writer | FMT | FMT-12, FMT-13, SPK-07 | Vector objects editable in Illustrator; ICC/appearance tests and report |
| [ ] **FMT-15** Hybrid visibility/import test | FMT + DSN | FMT-13, FMT-14 | Both-checkbox behavior works in Illustrator: source appearance correct, vectors accessible |
| [ ] **AND-07** Export settings UI | AND + DSN | FMT-06, SPK-11 | Checkboxes Vector/Raster/ICC, format choice, disabled invalid state, previews/warnings |
| [ ] **AND-08** Export background job UX | AND | AND-07, FMT-07 | SAF output, progress, cancellation, actionable errors and verified completion |
| [ ] **QAP-04** Cross-app export matrix | QAP | FMT-11, FMT-15, AND-08 | 8+ fixtures tested Photoshop, Krita and Illustrator, pass rate/traces/sign-off |
| [ ] **FMT-16** Compatibility guide | FMT + DSN | QAP-04 | Clear PSD/AI limitations and supported ICC semantics in user guide |
| [ ] **INT-04** Real illustrator end-to-end | ORCH | QAP-04, FMT-16 | Sketch+color->PSD/SVG/PDF->Adobe/Krita: verify S3–S7, no P0 export defect |

**Gate G5:** real SVG/PDF editable-vector and appearance-preserving behavior verified in Illustrator; PSD verified in Photoshop/Krita; no `.ai` false labels or silent ICC loss.

## Phase P6 — GPU, hardware, QA and app polish

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **REN-03** GPU compositor parity | REN + CLR | CORE-05, CLR-03, REN-02 | CPU/GPU golden parity suite and fallback; driver-specific deviations documented |
| [ ] **REN-04** Tile/cache memory optimization | REN | REN-03, DOC-04 | Budgeted caches, 4k/20-layer endurance, graceful memory pressure |
| [ ] **AND-09** Lifecycle and background stability | AND | REN-03, FMT-03 | Pause/resume/rotate/multiwindow/task eviction with restored state |
| [ ] **AND-10** Tablet hardware adaptation | AND + DSN | AND-09 | Landscape/portrait, stylus vendor differences, external keyboard, scalable text |
| [ ] **QAP-05** Pen latency characterization | QAP + AND | QAP-01, REN-03 | High-speed-video and Perfetto evidence, p50/p95 event-to-photon methodology |
| [ ] **QAP-06** GPU/device matrix | QAP + REN | REN-04, AND-10 | Min 2 physical GPU/vendor device classes where available, context loss/tearing tests |
| [ ] **QAP-07** Extended session stress | QAP | QAP-05, QAP-06 | 60-min sketch, save while drawing, OOM/thermal/frame pacing and bug list |
| [ ] **QAP-08** Malformed-file fuzzing | QAP + FMT | FMT-05, FMT-11 | Native/PSD/ICC/SVG fuzz seed corpus, bounded input handling and crash triage |
| [ ] **DSN-04** Accessibility and first run | DSN + AND | AND-10, AND-07 | Screen readers, hit targets, tool tips, default preset onboarding, left-hand support |
| [ ] **INT-05** Performance acceptance | ORCH | QAP-05…QAP-08, DSN-04 | Signed budgets, hardware/CPU/GPU results, no critical regressions |

**Gate G6:** release performance budgets met or downgraded transparently with measured reasons; hardware limitations and power/memory expectations published.

## Phase P7 — Public beta and 1.0 release engineering

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **REL-01** UX/content/localization sweep | DSN | DSN-04, FMT-16 | English copy audited, localized resources, no ambiguous 'AI native' terminology |
| [ ] **REL-02** Build/sign/package pipelines | QAP + AND | INT-05 | Android AAB/APK production builds, signing controls, dependency lock and release tags |
| [ ] **REL-03** OSS compliance and SBOM | QAP | SPK-10, REL-02 | SPDX/license notices/third-party attribution, dependency and font/icon licensing verified |
| [ ] **REL-04** Play policy and target SDK check | AND + QAP | REL-02 | API target reviewed at ship date, permissions/privacy/data safety complete |
| [ ] **REL-05** Public source/documentation | ORCH + DSN | REL-03 | README, contribution guide, supported devices, build commands, user guides and known issues |
| [ ] **REL-06** Reproducible test artifact bundle | QAP | INT-05, INT-04 | Version-pinned fixtures, CI logs, color export evidence, cross-app sign-off |
| [ ] **REL-07** Bug bash + severity triage | ORCH + QAP | REL-06 | Zero known P0/P1 bugs, accepted P2 risks in release notes |
| [ ] **REL-08** Offline/privacy check | QAP | REL-02 | No network required for draw/save/export, no unexpected telemetry, permissions audited |
| [ ] **REL-09** Restore/recovery rehearsal | FMT + QAP | REL-07 | Crash restore and backup workflow validated on stable RC build |
| [ ] **REL-10** User beta and fixes | DSN + AND + ORCH | REL-07 | 5–10 consented sketchers complete S1–S6; top issues fixed or documented |
| [ ] **REL-11** Tag and publish Veskra 1.0 | ORCH | REL-01…REL-10 | Signed Git tag, binaries/source, changelog, known compatibility limitations, rollback plan |

**Gate G7:** documented 1.0 release; no correctness claims beyond evidence.

## Phase P8 — Full-scope post-1.0 roadmap (not dropped; separately scheduled)

| Ticket | Owner | Depends | Deliverable and acceptance / DoD |
|---|---|---|---|
| [ ] **NEXT-01** PSD import editing | FMT | REL-11 | Supported PSD subset imported into editable layer tree; unknown features reported |
| [ ] **NEXT-02** Display P3 authoring | CLR + REN | CLR-04, REL-11 | Hardware-supported P3 surface, profile correct export/readback and calibrated tests |
| [ ] **NEXT-03** CMYK soft proof | CLR | NEXT-02 | Printer ICC proof view and conversion diagnostics; not confused with true CMYK painting |
| [ ] **NEXT-04** Enhanced wet brushes | BRH | REL-11 | Smudge/watercolor with bounded data/memory, explicit raster-only export flags |
| [ ] **NEXT-05** Advanced vector editing | BRH + DSN | REL-11 | Edit stroke control points/pressure handles, path node UX, boolean operations separately scoped |
| [ ] **NEXT-06** Vector/raster transform parity | REN + BRH | NEXT-05 | Non-destructive transforms, masks, group semantics, interoperability suite |
| [ ] **NEXT-07** Large-document PSB | FMT | NEXT-01 | 16-bit/large PSD support tested on Photoshop/Krita corpus |
| [ ] **NEXT-08** Brush/palette interoperability | BRH | REL-11 | Import/export presets/palettes with clear license and format support |
| [ ] **NEXT-09** Native `.ai` feasibility | FMT + QAP | INT-04 | Evidence-based spec/legal study + artifact proof; no shipping commitment without gate |
| [ ] **NEXT-10** Desktop headless/GUI portability | ORCH | REL-11 | Rust core compiles desktop; desktop GUI separately planned, no Android regression |
| [ ] **NEXT-11** Optional print/production workflows | CLR + FMT | NEXT-03, NEXT-09 | Scoped research for ICC soft proof, PDF standards, color separation and preflight |

---

## Assignment waves (dependency-correct)

**Wave A — parallel research from day 1**
- AND → SPK-01/SPK-02; REN → SPK-03 after JNI proof; BRH → SPK-04; CLR → SPK-05; FMT → SPK-06/SPK-07/SPK-08; QAP → SPK-10/SPK-12; DSN → SPK-11.
- ORCH creates ADR skeleton, review process and enforces reproducible artifact submissions. Do not begin feature UI implementations yet.

**Wave B — contract freeze + first app**
- ORCH → ARC-01…03 and CORE-01/02; REN → CORE-04/05; BRH → BRH-01; AND → AND-01/02/03; FMT → FMT-01.
- ORCH alone owns INT-01 integration and G1 sign-off.

**Wave C — independent features**
- AND → gestures/palm; BRH → presets/pencil/ink; REN → dirty update; DSN → brush/tool rail; QAP → sketch endurance.
- Later BRH vector fitting can run while REN builds layer/composition; CLR establishes pixel contracts **before** blend-mode integration.

**Wave D — reliability and export**
- CLR → ICC + color oracle; FMT → native storage + PSD/SVG/PDF; AND → export sheet + SAF; QAP → fixtures/Interop; ORCH → alignment and final gates.
- Import/export work is parallel **only after** profile, document and layer contracts are frozen.

**Wave E — hardening/release**
- REN → GPU optimization; AND → lifecycle; QAP → stress/fuzz; DSN → accessibility/docs; ORCH → release gate review.

### Allowed concurrent code ownership

| Owned surface | Primary owner | Change-control |
|---|---|---|
| `veskra-core`, `veskra-engine`, ADR/contracts | ORCH | Everyone proposes, ORCH merges |
| `apps/android`, `veskra-ffi`, `veskra-input` | AND | BRH review for stroke fields |
| `veskra-brush`, `veskra-vector` | BRH | REN/CLR review math / raster adapters |
| `veskra-raster`, `veskra-compose`, `veskra-gpu` | REN | CLR review all color operations |
| `veskra-color` | CLR | REN/FMT review boundary behavior |
| `veskra-format`, `veskra-io` | FMT | CLR reviews ICC; BRH reviews vector semantics |
| `.github`, fixtures, audit/perf harness | QAP | ORCH review release gates |
| `apps/android` layouts and UX specs | DSN/AND | DSN designs, AND integrates |

**Blocking policy:** if a ticket requires a contract change outside its owned surface, create an RFC/ADR request; do not silently edit the dependency crate and hope cherry-picking works.

### Template for a completed ticket handoff

```text
Ticket: FMT-12 — SVG editable paths
Branch/commit: <hash>
Evidence: <link to SVG fixtures and Illustrator import screenshots>
Tests: cargo test -p veskra-io ... [include exact commands/results]
Result: N vector paths, bounded path-outline error <= stated tolerance
Unsupported: textured smudge strokes -> raster preflight warning
Changed contracts: none / ADR-008 updated
New dependencies/assets: <versions/license>
Known limitations: <reproducible list>
Requesting review: ORCH + BRH + QAP
```

## Recommended first ten agent assignments

1. SPK-01 Android NDK/Rust/JNI vertical feasibility (AND)
2. SPK-02 stylus input behavior on actual device (AND)
3. SPK-04 Hokusai brush technical evaluation (BRH)
4. SPK-05 measured ICC/sRGB pipeline (CLR)
5. SPK-06 PSD writers interop proof (FMT)
6. SPK-07 SVG/PDF Illustrator editability proof (FMT/BRH)
7. SPK-03 Android native surface lifecycle (REN, gated on SPK-01)
8. SPK-11 tablet-first wireframes and export sheet states (DSN)
9. SPK-10 license/security review (QAP, after deps identified)
10. ARC-01 freeze stable core contracts (ORCH, after relevant spikes)

**Do not assign P2+ feature tickets to agents before G0/G1 unless they are explicitly isolated research tasks.**
