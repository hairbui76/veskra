# Veskra — Architecture Decisions, Risk Register and Open Questions

**Status:** Proposed / unresolved until Phase-0 gate.  
**Rule:** This document makes assumptions visible. Only a reviewed ADR makes a technical decision final. `MASTER_PLAN.md` defines non-negotiable product outcomes.

## 1. Proposed architecture decision record (ADR) index

| ADR | Question | Baseline proposal | Fallback / condition | Status |
|---|---|---|---|---|
| ADR-001 | License and re-use | Independently developed Veskra under MIT OR Apache-2.0; rigorous notices and provenance | If GPL code is required, explicitly reassess whole-product obligations/legal review | Proposed |
| ADR-002 | Android platforms | Tablet-first `arm64-v8a`, proposed min API 29, target API 36 at 2026-10-10 | Reassess Android version floor after actual tablet survey and Play policy at release | Proposed |
| ADR-003 | UI framework | Native Kotlin + Jetpack Compose, Material 3 toolbar/panels | Another native UI only after demonstrable input/lifecycle advantage | Proposed |
| ADR-004 | Brush core | Hokusai evaluation, do not freeze as dependency yet | Build small custom pencil/ink engine, or use isolated compatible component | Open spike |
| ADR-005 | Native pixel layout | Typed PixelDescriptor, high-precision compositing-ready architecture, explicit linear/encoded and alpha conventions | Determine memory/perf compromise from data, do not assume f16 everywhere | Open spike |
| ADR-006 | Color system | Managed sRGB v1, LittleCMS Rust wrapper candidate, independent numeric oracle | Pure-Rust ICC alternative if tested and no quality degradation | Open spike |
| ADR-007 | PSD writer | Compare PhotoCraft PSD implementation vs psd-great, choose working RGB 8-bit output first | Minimal custom supported PSD writer from Adobe public spec | Open spike |
| ADR-008 | Illustrator output | SVG/PDF, editable path and raster appearance toggles, no false `.ai` native promise | More conservative SVG-first export if PDF writer unvalidated | Open spike |
| ADR-009 | Native save | Versioned `.veskra` project with checksum, staged write, app-private recovery | Alternative container if random-access/durability benchmarks demand it | Proposed |
| ADR-010 | GPU/rendering | wgpu/Vulkan viewport and CPU reference compositor | Native Vulkan/OpenGL wrapper or CPU fallback if surface life/color issues | Open spike |
| ADR-011 | Tile dimensions/cache | Benchmark 64 vs 128 vs 256, sparse tiles, reversible dirty regions | Two-level adapter for MyPaint-like 64 tiles and compositor larger tiles | Open spike |
| ADR-012 | Dual export appearance visibility | Prefer raster visible as appearance reference, editable vectors available in separate group without duplicating visible marks | Switch group/visibility strategy if Illustrator drops hidden content | Open spike |
| ADR-013 | Wide gamut | Managed sRGB first; Display P3 after device-verified negotiation and ICC tests | Stay sRGB-only as supported UI | Proposed |
| ADR-014 | PSD masks/blends | Preserve verified subset, flatten unsupported content with explicit warning | Fail export if appearance cannot be safely represented | Proposed |
| ADR-015 | Minimum hardware specs | Set after real samples at G0/G6 | Publish feature/device limits rather than promising all tablets | Open spike |

## 2. High-priority risks, evidence and mitigations

| Risk | Severity / likelihood | Early evidence | Mitigation / action |
|---|---|---|---|
| R-01 Rust GPU surface fails on specific Android GPUs | Critical / medium | SPK-03 rotations, background, native handle lifetime | Abstract display renderer, keep CPU oracle, test Qualcomm/other GPUs early |
| R-02 Hokusai tile/fix15 semantics conflict with Veskra pixel precision | High / high | SPK-04 + SPK-05 brush and profile conversion | Explicit adapter and color contracts; choose limited brush core if integration dominates |
| R-03 User sees color change at export | Critical / high | COL-001…COL-012 swatches, ICC source/target checks | Stage-by-stage managed color, default sRGB, numeric regression, warnings |
| R-04 Color looks different across uncalibrated screens regardless of correct export | High / high | Controlled device review and ICC traces | Set honest scope: deterministic files and controlled reference tests, publish device limits |
| R-05 PSD writer passes parser but differs in Photoshop composite | High / medium | SPK-06 and PSD-002/004/005 manual oracle | Writer comparison, merged preview, transparency/ICC test corpus, supported-feature subset |
| R-06 Illustrator drops/moves hidden editable vectors or double-renders | High / high | SPK-07 + ILL-003 actual Illustrator test | Freeze a verified group visibility policy before shipping both-checkbox UX |
| R-07 Unrealistic user expectation of direct `.ai` | High / high | User requirement vs Adobe document semantics | UI labels `Export for Illustrator (SVG/PDF)`; save `.ai` in Illustrator; dedicated R&D only |
| R-08 Loss during app kill or SAF partial write | Critical / medium | SPK-08, SAV-003/004/005 | App-private checkpoints and staged provider copy, recovery UI and versioned dirty state |
| R-09 Large canvas/undo exhausts mobile RAM | High / high | PERF-003 and tile benchmarks | Sparse tiles, bounded history, cache eviction, low-memory warnings |
| R-10 Pen jitter/lag, palm strokes on vendor device | High / high | SPK-02, SPK-09, STY tests | Native historical samples, input arbitration, fast preview, adaptive tool fallback |
| R-11 Unsupported blend math causes color/PSD mismatch | High / medium | PNT-007, COL-006, PSD-004 | Reference CPU compositor, explicit blend-space policy, conservative export fallback |
| R-12 Copyright/license obligations from Krita or bundled brush packs | Critical / medium | SPK-10, SEC-002 | Clean-room independent code, approved licenses/asset manifest, no Krita copying |
| R-13 Many agents change same contracts, introducing merge/review chaos | High / high | PR ownership + interface diff | ORCH-owned public schemas, exclusive crate ownership, ADR before cross-boundary change |
| R-14 No access to Adobe app for actual export verification | High / medium | G0/G5 manual gates blocked | Find qualified human reviewer/license; mark gate BLOCKED rather than fake compatibility |
| R-15 App-specific external color metadata differs (SVG/PDF ICC) | High / high | ILL-005, COL-010 | Prefer sRGB with tagged raster assets and truthful per-format disclosures |
| R-16 Experimental third-party Rust crates mutate/break builds | Medium / high | Pinned dependency CI and SPK audits | Pin git commits, isolation adapters, fork only with provenance/license checks |
| R-17 AndroidX Ink causes duplicated stroke authority | Medium / medium | SPK-09 architectural prototype | Use as latency overlay only if provenance/unification and render consistency proven |
| R-18 Release/play policy changes after planning date | Medium / high | REL-04 target API verification | Recheck Play SDK and policy on release branch before submission |

## 3. Technical questions needing objective resolution

### Q1. What does 'exact color' mean for Veskra?

Acceptance is **numerical correctness under a defined source ICC, document ICC, target ICC, rendering intent, blend policy and comparison reference**, plus hardware-specific display review. It does not mean identical human perception on every unprofiled monitor. G0 determines sRGB color test fixtures and device measurement availability.

### Q2. Can every stroke be a vector?

No. A path/pressure curve can provide editable line geometry; watercolor/smudge/pixel textures require either an approximate vectorized style or preserved raster. Veskra must store honest per-stroke vector-eligibility data and warning counts.

### Q3. Can the same file satisfy 100% raster appearance and 100% editable vectors?

Not for arbitrary raster effects. Hybrid output can carry an appearance reference and editable vector interpretation. Original brush dynamics may not survive in Illustrator. Acceptance: preserved initial appearance, accessible vector paths, no double draw, explicit approximation limits.

### Q4. Can Veskra export native `.ai`?

Do not treat a portable SVG/PDF generated by a third party as a genuine Illustrator-native document. Adobe Illustrator can import/open supported vector formats and then save an `.ai`; preservation of Adobe-only editing attributes is a separate problem and requires the NEXT-09 research gate. The product label remains `Export for Illustrator (SVG/PDF)`.

### Q5. Should we use PhotoCraft as a core?

PhotoCraft is a useful **architecture and optional independent-module candidate**, not an assured Android port. Evaluate any reusable crates against alternative implementations, dependency size, APIs, alpha status, license and actual device build. Avoid importing a general-purpose desktop editing subsystem into a focused tablet painter solely for convenience.

### Q6. Should we build on AndroidX Ink?

AndroidX Ink 1.0 is officially stable (as of the date of this plan), but its tradeoff against preserving one Rust-centric raster/vector brush and document representation must be tested. It may help low-latency preview while Rust remains the authority. The spike may reject integration if it doubles geometry/behavior.

### Q7. Can we guarantee RGB layers look the same inside Photoshop?

Not automatically: PSD viewers may choose different blend-space policies and proprietary layer semantics. Prioritize 8-bit sRGB, documented supported blend modes and merged composite validation, with fallback rasterization and warnings for unsupported operations.

### Q8. What if a contributor wants to relicense or copy from Krita?

Stop and create an ADR requiring explicit authorization and license review. The default independent MIT/Apache track cannot quietly absorb GPL code. Attribution alone does not cure license incompatibility.

## 4. ADR template

Copy into `adr/ADR-XYZ-short-name.md`:

```markdown
# ADR-XYZ: <title>
Status: Proposed | Accepted | Superseded | Rejected
Date: YYYY-MM-DD
Owner: <role>
Related tickets: <IDs>

## Context
Problem, user need and relevant facts; include measured evidence.

## Decision
One unambiguous technical commitment and scoped version(s).

## Alternatives considered
At least 2 options with implementation, performance, licensing and reliability tradeoffs.

## Acceptance experiment
Specific reproducible test, input fixture, machine/device and pass/fail bar.

## Consequences
What the decision enables, what it prevents, new dependency/contract changes.

## Rollback / fallback
If the experiment fails, the explicit recovery path.

## Review
Approver, QA reviewer, date, linked test evidence.
```

## 5. Milestone decision record

| Checkpoint | Owner signs | Required tracked artifacts |
|---|---|---|
| G0 | ORCH + QAP | SPK-01 to 12 evidence, ADR-001 to ADR-010 |
| G1 | ORCH + AND + REN | Android vertical slice demo, project file round-trip |
| G2 | ORCH + BRH + AND | Sketch endurance and stylus behavior proof |
| G3 | ORCH + REN + BRH | 12-layer coloring fixture and hybrid classification |
| G4 | ORCH + CLR + FMT + QAP | ICC numeric report, save/recovery tests |
| G5 | ORCH + FMT + QAP | Photoshop/Krita/Illustrator matrix, export capability report |
| G6 | ORCH + AND + REN + QAP | GPU, memory, latency and device coverage |
| G7 | ORCH + QAP + release owner | Security/license, policies, signed binaries, user documentation |

**If evidence is unavailable, leave status BLOCKED.** The plan's credibility comes from explicit boundaries and recorded proof.
