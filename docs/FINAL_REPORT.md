# Veskra — execution session final report (2026-10-09)

**Role:** Lead engineering agent (ORCH) with parallel specialist sub-agents (AND, BRH, REN, CLR, FMT,
QAP + an independent reviewer). **Branch:** `claude/vibrant-cori-0mnhk5` (local; push to GitHub was
denied with HTTP 403, see B-4).

## 1. Bottom line

* A working, tested **Rust engine** now exists: color management, sparse tiles, deterministic
  brushes, vector stroke geometry, compositor, document model with exact undo, `.veskra` storage
  with crash-safe saves and recovery, PNG/PSD/SVG/PDF export, color-managed PNG import, a single
  command API, a JNI bridge and a headless CLI.
* The **Android app** (Kotlin/Compose) is written. Its pure-JVM `:kit` module is built and tested,
  including real JNI round trips into the Rust library on a desktop JVM. The `:app` module has
  **never been compiled**, and there is **no APK**, because the Android SDK host (`dl.google.com`)
  is denied by this environment's network policy.
* **No quality gate (G0–G7) is signed.** Each gate requires physical stylus tablets and/or Adobe
  Photoshop/Illustrator evidence, and neither was available here. Following AGENTS.md, every
  ticket whose requirements need that evidence is `Blocked` or `In Verification`, never `Done`.
  Nothing was self-certified.

## 2. What was built (all host-verified)

| Area | Crate / path | Highlights | Tests |
|---|---|---|---|
| Contracts | `veskra-core` | IDs, geometry, typed errors, confirmed/predicted samples, pixel descriptors, export contracts (no `.ai` variant) | 18 |
| Color | `veskra-color` | ICC v2/v4 matrix/TRC parser and writer, generated sRGB/P3 profiles, transforms, Lab, CIEDE2000, alpha/depth conversions | 55 |
| Raster | `veskra-raster` | 64 px COW tiles, AA selections, bounded flood fill, affine transform | 46 |
| Input | `veskra-input` | Packed JNI event decode, stroke state machine, palm/cancel, pinch/rotate with stable anchor, taps, buttons/hover | 45 |
| Vector | `veskra-vector` | Error-bounded cubic fitting, variable-width outlines (nonzero), SVG/PDF path output | 26 |
| Brush | `veskra-brush` | 10 presets with honest vector eligibility, batch-independent determinism, stabilizer, grain, alpha lock, eraser | 37 |
| Document | `veskra-doc` | Raster/vector/group tree, clipping, masks, stroke records, fidelity classification, snapshot history with budget | 16 |
| Compositor | `veskra-compose` | W3C blend modes, gamma/linear spaces, isolated/pass-through groups, psd-tools-compatible clipping | 33 |
| Native file | `veskra-format` | Own bounded ZIP, schema 1, integrity hashes, atomic save with re-validation, autosave store | 43 |
| Export/import | `veskra-io` | PNG (iCCP), PSD (layers/groups/masks/clipping/ICC/composite), SVG/PDF hybrid export, PNG import | ~100 |
| Engine | `veskra-engine` | Command API (`docs/COMMANDS.md`), live stroke + prediction, input pipeline, exports on snapshots | 37 |
| JNI | `veskra-ffi` | Handle registry, jni 0.22 exports, export off-lock | 7 |
| CLI | `apps/cli` | run / replay-input / render / export / inspect / benchmark / compare-color | CLI tests |
| QA | `veskra-qa`, `scripts/interop` | S1–S8 headless scenarios, fault matrix, fuzzing, 17-step oracle runner | 9 suites |
| Android | `apps/android` | `:kit` (JVM) + `:app` (Compose, never compiled) | 89 JVM incl. 2 real-JNI |

**Workspace status:** `cargo fmt`, `cargo clippy -D warnings`, and `cargo test --workspace` all
pass (456 tests, 0 failed, 5 ignored benchmarks). The crate-layering check and the tracker
consistency check pass. `cargo deny` licenses/bans/sources/advisories are all ok.
`scripts/interop/run_all.sh` passes 17/17 steps.

## 3. Verification evidence (independent oracles; none of it is Adobe or device evidence)

| Topic | Result | Report |
|---|---|---|
| Color | 8-bit sRGB identity: 16 777 216 colors, 0 LSB. sRGB→P3 vs colour-science: ΔE00 median 0.088, p95 0.300. Bit-exact vs LittleCMS 2.19 (unoptimized). CIEDE2000 matches all 34 Sharma pairs. | `reports/color-baseline.md` |
| PNG | Pillow and the `png` crate decode Veskra PNGs with 0 LSB difference; ICC bytes are exact | `reports/psd-interop.md` |
| PSD | psd-tools layer tree and ICC match; merged image and every layer channel are 0 LSB from Veskra's composite; psd-tools' recomposite is within 1.961 (premultiplied); ImageMagick agrees. Krita 5.2.2 ignores clipping and group opacity on import (Krita-side). | `reports/psd-interop.md` |
| SVG/PDF | rsvg-convert and pdftocairo renders are identical to the composite. No double drawing (alpha 128 vs 192 in the negative control). qpdf 27/27, pypdf 29/29. Inkscape keeps hidden SVG vectors but drops hidden PDF layers, which led to the ADR-012 amendment. | `reports/illustrator-export.md` |
| Robustness | 180 000 mutated inputs over 9 parsers/boundaries: 0 panics. Every truncation of bundles and ICC data gives a typed error. | `reports/qa-fuzz.md` |
| Data safety | 690 injected write failures plus a 100-cycle save fault matrix: original file always intact, typed errors, dirty state preserved | format tests, `reports/qa-scenarios.md` |
| Undo | Exact over 1000 random operations; 46-step S2 undo back to creation; 2000-seed dirty-rect property test | doc/engine/QA tests |
| Performance (HOST CPU only) | 2000-sample stroke on 2048²: batch p95 0.07–0.26 ms. 2048² 12-layer composite ≈ 0.1–0.3 s. Save/open 1.1 s / 0.4 s. | `reports/performance-host.md` |

## 4. Independent review outcome

The reviewer found 14 defects in the orchestrator-written engine/FFI/CLI, all fixed with
regression tests. Two were high severity (stroke loss at commit). QA later found 3 more (recovery
dirty flag, permission error code, off-canvas stroke cost), also fixed. The brush prediction path
was made O(prediction) instead of O(stroke): host p50 per batch went from 1.3 ms to 0.10 ms.

## 5. Decisions recorded (ADRs)

ADR-001…016 are in `adr/`. The key ones:
* Independent brush engine (Hokusai not adopted).
* RGBA16 premultiplied gamma-encoded tiles with Photoshop-like blending.
* Pure-Rust ICC with LittleCMS as a test oracle only.
* Independent PSD writer.
* SVG/PDF handoff with no `.ai` output.
* `.veskra` ZIP schema 1.
* CPU compositor with HWUI bitmap presentation (GPU deferred).
* 64 px tiles.
* Per-format hybrid visibility: SVG hides the editable vectors; PDF uses the residual strategy.

Several of these are **Accepted (provisional)** until device or Adobe experiments run.

## 6. Ticket completion (machine-readable: `progress/tickets.json`; table: `docs/TICKET_STATUS.md`)

113 tickets: **0 Done, 61 In Verification, 26 Blocked, 26 Backlog**. Implementation is complete
on 45 tickets and partial on 34. No ticket can be `Done`: every dependency chain passes through
a hardware or Adobe spike (SPK-01/02/03/06/07/09/12) that is blocked in this environment.

## 7. Known limitations and remaining work

1. **Android build and device work (B-1, B-2):**
   * Compile `:app` and fix whatever the first SDK build reports.
   * Build the NDK cdylib (`apps/android/scripts/build-rust.sh`).
   * Run SPK-01/02/03/09/12 on two or more tablet GPU families.
   * Measure latency (QAP-05), endurance (QAP-01/07), lifecycle (AND-09), and SAF providers (FMT-04).
   * Perform the physical pen-feel review.
2. **Adobe verification (B-3):**
   * Photoshop: PSD-002/004, COL-009.
   * Illustrator: ILL-001…009 using `scripts/interop/illustrator_manual_checklist.md`.
   * Confirm or revise ADR-012 per format.
3. **Not implemented:**
   * GPU compositor (REN-03/04).
   * Group masks; layer-mask painting UI.
   * Interactive transform handles.
   * Smudge/wet media.
   * 16-bit PSD, PSB, PSD import.
   * P3 authoring (gated).
   * Localized strings beyond English.
   * Accessibility audit (DSN-04).
   * Release signing and packaging (REL-02), Play policy (REL-04), beta (REL-10).
4. **Format limits:**
   * The `.veskra` container has no zip64: at most 65 534 entries and 4 GiB (fails cleanly).
   * Strokes far off-canvas lose their vector form by design.
   * The fsync of the parent directory is best effort.
5. **Krita interop:** Krita 5.2.2 does not honour PSD clipping or group opacity on import. This
   is documented for users.
6. **Name/namespace:** `org.veskra.dev` is provisional pending trademark review (B-7).

## 8. How to reproduce

See `README.md` › Repository quick start and `CONTRIBUTING.md`. Every number in this report comes
from commands listed in the linked reports, and all of them were run in this session.
