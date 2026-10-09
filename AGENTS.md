# AGENTS.md — Veskra Coding Agent Rules

**Read before working on Veskra.** This repository currently contains execution planning documents; do not assume an app, tests, crates or existing commits are already implemented. All implemented claims require real artifacts and test evidence.

## 1. Product intent

Veskra is an **offline-first open-source sketching and coloring app for Android tablets**. Its differentiators are good stylus feel, trustworthy color handling, layered PSD export and Illustrator-compatible SVG/PDF with distinct editable vector and preserved-raster options. Build a headless/testable Rust graphics/document core and a native Kotlin/Compose Android UI.

## 2. Document precedence

1. `MASTER_PLAN.md` — product requirements, committed scope, release gates.
2. `ARCHITECTURE.md` — contracts, crate dependencies, lifecycle, file semantics.
3. `DECISIONS_AND_RISKS.md` and accepted ADRs — supersede proposals in this document for a specific decision.
4. `ROADMAP.md` — task IDs, gates, ownership and dependencies.
5. `VERIFICATION_MATRIX.md` — how to prove completion.
6. `AGENT_ASSIGNMENTS.md` — ready-to-assign role prompts.

Conflicts are escalated to ORCH via an ADR before edits; never assume the most recently opened file wins.

## 3. Absolute guardrails

- **Do not port or copy Krita C++/Qt/GPL code** into Veskra by default. Veskra is independently implemented. GPL reuse requires explicit license/legal ADR and matching distribution commitments. Do not copy proprietary Adobe code, assets or reverse-engineered secrets.
- **Do not claim native `.ai` export.** Initial Illustrator handoff is SVG/PDF verified with Illustrator, not `.ai`. A PDF renamed `.ai` is prohibited.
- **Do not claim 'no color shift on all devices.'** Color accuracy is measured under explicitly documented profiles, displays, apps, render intents and reference conditions.
- **Do not use unvalidated libraries in production.** PhotoCraft, Hokusai, psd-great, wgpu, LittleCMS wrappers, vector/PDF crates are candidates until their spike and license audit pass.
- **Do not store predicted MotionEvents** as confirmed strokes, and do not invent pressure/tilt data absent from hardware input.
- **Do not silently flatten/edit away user content** on PSD/Illustrator export. Count and report every unsupported and rasterized feature.
- **Do not make Android Compose render pixel-by-pixel strokes per UI recomposition** or block the Android UI thread during export/ICC initialization.
- **Do not upload user artwork/brushes/paths** for telemetry, cloud processing or external APIs without a separate, explicit product requirement and consent.
- **Do not say a test passed without running it.** If not runnable, mark `Not run` with blocker and do not tick DoD.
- **Do not add transient giant generated binaries, third-party copyrighted brush packs or private Photoshop/Illustrator assets** to Git.

## 4. Task execution protocol

1. Read assigned ticket and its prerequisites. Verify all dependencies are complete and correct.
2. Identify owned files/crates and review accepted ADRs; list proposed touched paths in your brief.
3. Write an implementation plan in the PR/issue with tests and expected edge cases **before** broad changes.
4. Implement the smallest useful vertical slice. Add tests/golden fixtures near the change.
5. Use pinned versions and run local formatting/lint/tests; include device evidence if stylus/graphics/lifecycle/color is involved.
6. If a cross-crate public type must change, stop and request ORCH contract approval first.
7. Provide handoff block: commit/hash; tests actually run; metrics/fixtures; caveats; dependency/license changes; reviewer roles.
8. ORCH reviews and merges. QAP verifies milestone/release gates. You do not self-certify your own integration gate.

## 5. Coding standards

**Rust:** current agreed stable toolchain pinned via `rust-toolchain.toml`; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. Prefer safe Rust. Document every `unsafe` section with invariants and reviewer approval. Use bounded allocations, checked arithmetic, typed errors (`thiserror` or equivalent), stable public IDs, explicit pixel/color descriptors, and deterministic tests. Never use blanket `unwrap()` on imported file data.

**Android/Kotlin:** pinned Android Gradle Plugin/Kotlin/NDK versions; Material 3 controls for editor chrome; >= 48 dp touch targets; Android Back and lifecycle honored; move work to native/bounded background threads. Prefer Android Storage Access Framework rather than broad filesystem permission. Native callbacks must validate lifetime/thread ownership. Instrument real pen hardware rather than emulator-only approximation.

**Data/file:** schema version, migration/compatibility policy, integrity bounds and no partial save falsely marked successful. Export must have preflight/capabilities and typed warning report. PSD tests must use an independent application or library as oracle. Image colors must preserve documented profile semantics through import, compose, preview and export.

**UI:** tablet- and touch-first, clear raster/vector status, export checkboxes as defined in MASTER_PLAN, visible save/recovery state, keyboard optional, accessible tool names, left/right handed toolbar.

## 6. Required verification per change type

| Change | Mandatory proof |
|---|---|
| Document/history | Round-trip state + undo atomicity tests + format-version compat |
| Stylus/brush | Replay fixtures, confirmed-only storage, physical stylus behavioral review |
| Raster/GPU | CPU golden parity, rotation/context-loss on device, bounded memory |
| Color/blend | ICC/profile test, alpha fixture, numeric comparison and signed thresholds |
| Native `.veskra` | Open/save/migrate, truncated/corrupt file, force-stop recovery |
| PSD | Valid file structure, ICC/composite/layer check + actual Photoshop/Krita opener |
| SVG/PDF | Edit/recolor vector in Illustrator; appearance/raster profile; no double draw |
| Android UI | Tablet screenshot/video, orientation/size classes, accessibility and Back |
| Release | Security/license audit, signed artifact, fresh-install and offline smoke test |

## 7. Quality gates

**G0** feasibility/ADRs → **G1** working stylus-to-file vertical slice → **G2** sketch feel → **G3** layers/coloring → **G4** data reliability+color math → **G5** PSD/Illustrator interchange → **G6** performance/device coverage → **G7** signed release.

A gate may be marked done only by ORCH with QAP evidence; don't infer it from partial passing tasks.

## 8. Handling unresolved questions

Write an ADR proposal if work depends on one of the following: OSS license, Rust/Android interface, renderer, tile size, pixel depth, blending policy, ICC library, PSD writer, native format version, Illustrator visibility strategy, scope of P3/wide gamut, or native AI claims. Include alternatives, trade-offs, risk and falsifiable experiment. The integrator decides and updates docs **before** changes ripple into other agents' branches.

## 9. Issue/PR checklist

- [ ] Ticket prerequisites accepted
- [ ] Scope limited to owned surfaces or contract change approved
- [ ] Tests and fixtures added
- [ ] Source/licensing/asset notices verified
- [ ] Build/lints/tests results included (or explicit not-run reasons)
- [ ] Hardware/color/interoperability evidence where applicable
- [ ] No silent data loss/flattening/false compatibility claim
- [ ] Docs/ADRs and roadmap state updated
- [ ] ORCH + required expert review requested

## 10. Reviewers by subject

- Input/Android: AND + ORCH.
- Brush geometry: BRH + REN (and CLR for colored textures).
- Compositing/ICC: CLR + REN.
- PSD/SVG/PDF: FMT + CLR + QAP, plus BRH for vector semantics.
- Native storage: FMT + QAP.
- Release/third-party assets/security: QAP + ORCH.

**Agent mantra:** Preserve the user's artwork, tell the truth about interoperability, measure color fidelity, and leave every task verifiable by another developer.
