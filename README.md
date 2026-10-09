# Veskra — Development Handoff Pack

**Created:** 2026-10-10 · **Implementation started:** 2026-10-09 (see `docs/FINAL_REPORT.md`)  
**Status:** Rust engine, exporters, JNI bridge, CLI and Android shell source exist and are tested on a
Linux host. **No quality gate (G0–G7) is signed**: Android device, APK build and Adobe application
evidence are blocked (see `docs/BLOCKERS.md`). Nothing here is a released product.  
**Mission:** Open-source, offline-first, Rust-powered drawing/painting app for Android tablets with native pen experience, color-managed exports, PSD and Illustrator-compatible SVG/PDF.

## Repository quick start

```sh
# Rust (toolchain pinned by rust-toolchain.toml)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_deps.py        # crate layering (ADR-016)
python3 scripts/progress.py summary  # machine-readable ticket tracker (progress/tickets.json)

# Headless CLI (same engine as Android)
cargo run --release -p veskra-cli -- benchmark-stroke --size 2048
cargo run --release -p veskra-cli -- export tests/fixtures/documents/12-layer-illustration.veskra /tmp/out.psd

# Kotlin :kit + real JNI round trip on the host JVM
cargo build -p veskra-ffi --release
(cd apps/android && ./gradlew :kit:test -Dveskra.ffi.lib=$PWD/../../target/release/libveskra_ffi.so)

# Independent-oracle interop suite (psd-tools, Krita, Inkscape, rsvg, poppler, LittleCMS)
bash scripts/interop/run_all.sh
```

Key documents added during implementation: `docs/CONTRACTS.md` (cross-crate APIs), `docs/COMMANDS.md`
(engine command API), `adr/` (ADR-001…016), `docs/COMPATIBILITY.md` (feature matrix),
`docs/BLOCKERS.md`, `docs/reports/` (color, PSD, Illustrator, QA, performance), `docs/FINAL_REPORT.md`.

## How to use this pack

1. Start with **[`MASTER_PLAN.md`](MASTER_PLAN.md)** — complete product contract, constraints, architecture overview, phase gates.
2. Read **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — module and API contracts, file structure, stroke/document/ICC/export design.
3. Open **[`ROADMAP.md`](ROADMAP.md)** — full project task backlog (Phase P0–P8), owners, dependencies and acceptance criteria.
4. Give all coding agents **[`AGENTS.md`](AGENTS.md)** as permanent execution rules.
5. Copy role prompts from **[`AGENT_ASSIGNMENTS.md`](AGENT_ASSIGNMENTS.md)** into your agent manager.
6. Keep **[`VERIFICATION_MATRIX.md`](VERIFICATION_MATRIX.md)** as QA definition of 'done'.
7. Track decisions and early blockers with **[`DECISIONS_AND_RISKS.md`](DECISIONS_AND_RISKS.md)**.

## Start here: recommended first action

Assign the ORCH agent the **Prompt 0 — Lead / Orchestrator** from `AGENT_ASSIGNMENTS.md`. Have that agent create a repository and assign **SPK-01…SPK-12**, not the full app at once. The first evidence milestone is **G0**: Android/JNI/stylus/GPU, brush, color and format export feasibility. Follow `ROADMAP.md` for subsequent gates.

## Critical product behavior

- The initial native working format is `.veskra`, with an editable project and recoverable autosave.
- Photoshop handoff: **layered `.psd` export**, limited to supported features with explicit warnings.
- Illustrator handoff: **SVG/PDF**, including independent checkboxes for `Editable vector strokes`, `Preserve original appearance`, and `Embed color profile` when supported. Both first options can be enabled simultaneously; tests must ensure no visible double rendering.
- Native `.ai` direct writing is **not** a v1.0 guarantee. Users may open Veskra SVG/PDF in Illustrator and use Illustrator **Save As `.ai`**.
- First verified color workflow is **sRGB plus ICC transforms and tagged exports**, measured across defined fixtures. No blanket promise about uncalibrated display matching.
- The engine is independent Rust code with a Kotlin/Compose Android shell. Krita is not copied or ported by default.

## What this pack does *not* do

It does not (yet) build an APK in the authoring environment, contain runnable Veskra source code, grant access to physical Android stylus hardware, or provide Adobe Illustrator/Photoshop test licenses. Those are explicit engineering tasks/gates in the roadmap. Candidate open-source Rust dependencies have not been independently built or endorsed for Veskra.

## Suggested repository tree (after initialization)

```text
veskra/
  README.md
  MASTER_PLAN.md
  ARCHITECTURE.md
  ROADMAP.md
  AGENTS.md
  AGENT_ASSIGNMENTS.md
  VERIFICATION_MATRIX.md
  DECISIONS_AND_RISKS.md
  adr/
  crates/
  apps/android/
  apps/cli/
  tests/fixtures/
  .github/workflows/
```

## Verified external starting points

- Hokusai brush engine: <https://github.com/reearth/hokusai>
- PhotoCraft: <https://github.com/storytold/photocraft>
- psd-great: <https://github.com/ioma8/psd-great>
- Android stylus documentation: <https://developer.android.com/develop/ui/compose/touch-input/stylus-input/advanced-stylus-features>
- AndroidX Ink: <https://developer.android.com/jetpack/androidx/releases/ink>
- Android wide gamut guide: <https://developer.android.com/training/wide-color-gamut/>
- Adobe PSD specification: <https://www.adobe.com/devnet-apps/photoshop/fileformatashtml/>
- Illustrator file format guide: <https://helpx.adobe.com/illustrator/using/saving-artwork.html>

## About the name

**Veskra** is the current working product name. Before public launch, check trademark, domain, package namespace and store availability. Do not invent a final publisher/package identity until approved.
