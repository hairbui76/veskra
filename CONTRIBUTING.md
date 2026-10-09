# Contributing to Veskra

Read `AGENTS.md` first — its guardrails apply to every contributor, human or agent.

## Ground rules
* Independent implementation: no Krita/GPL or Adobe proprietary code or assets (ADR-001).
* Never claim a test passed without running it; never fabricate device, Adobe or color evidence.
  Mark missing evidence `NOT_RUN` with a blocker id from `docs/BLOCKERS.md`.
* Cross-crate public API changes need ORCH approval (`docs/CONTRACTS.md`, ADR-016).
* Every behavior change ships with tests; parsers of external data must not panic on any input.

## Before sending a change
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_deps.py
python3 scripts/progress.py check
cargo deny check                      # licenses, bans, sources, advisories
bash scripts/interop/run_all.sh       # when touching color, PSD, PNG, SVG or PDF
(cd apps/android && ./gradlew :kit:test -Dveskra.ffi.lib=<abs path to libveskra_ffi.so>)  # JNI/Kotlin changes
```

## Layout
| Path | Owner role | Purpose |
|---|---|---|
| `crates/veskra-core` | ORCH | Frozen contracts |
| `crates/veskra-color` | CLR | ICC, transforms, Lab/ΔE |
| `crates/veskra-raster`, `veskra-compose` | REN | Tiles, selections, fill, compositor |
| `crates/veskra-brush`, `veskra-vector` | BRH | Brushes, stroke geometry |
| `crates/veskra-input`, `veskra-ffi`, `apps/android` | AND | Input arbitration, JNI, Android shell |
| `crates/veskra-doc`, `veskra-engine` | ORCH | Document model, command API |
| `crates/veskra-format`, `veskra-io` | FMT | `.veskra`, PNG/PSD/SVG/PDF, import |
| `crates/veskra-testkit`, `veskra-qa`, `scripts/interop` | QAP | Test support, scenarios, fuzz, oracles |

## Tracking work
`progress/tickets.json` is the machine-readable ticket state (from `ROADMAP.md`). Update it with
`python3 scripts/progress.py set <ID> <Status> --impl <none|partial|complete> --evidence "..."`.
`Done` requires evidence and all dependencies done; gate sign-off is ORCH + QAP only.
