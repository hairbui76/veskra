# Host performance report — HOST CPU, NOT device evidence

**Owner:** QAP · **Date:** 2026-10-09 · **Status:** context numbers only.
Every number below was measured on a shared x86_64 **container CPU**. None of it says anything
about frame rate, pen latency, memory or thermals on an Android tablet. **PERF-001…005 are
NOT_RUN (blocker B-2: no device)**; the host numbers are listed next to them only as context.

## 1. Machine and method

| Item | Value |
|---|---|
| CPU | Intel(R) Xeon(R) Processor @ 2.80GHz, `nproc` = 4 (1 thread/core), L2 4 MiB, L3 33 MiB |
| Memory | 16 GiB (no swap) |
| Toolchain | rustc 1.97.0 (2d8144b78 2026-07-07), `--release` (`lto = "thin"`, `codegen-units = 1`, `debug = 1`) |
| Load average (1/5/15 min) at start / end | 2.10 2.80 3.18 / 2.42 2.85 3.15 — **other agents were building concurrently**; timings are noisy upper bounds |
| Source | working tree on top of `04419b3` (uncommitted QAP files) |
| Script | `bash scripts/qa/perf_host.sh` → `target/qa/perf/{machine.txt, perf_runs.jsonl, *.log, perf_host.json}` |

Peak RSS: `/usr/bin/time` is not installed, so each command runs under a Python wrapper reading
`getrusage(RUSAGE_CHILDREN).ru_maxrss` (largest RSS of any waited-for descendant). For
`cargo test …` rows this maximum can be the `cargo` process rather than the test binary, so those
RSS values are upper bounds, not the engine's footprint.

## 2. Stroke engine — `veskra benchmark-stroke` (2000 samples in batches of 4, one stroke)

Engine path: `stroke.samples` JSON command → brush renderer → live tiles; `commitMs` = stroke
commit (finish, geometry, history record). Latency = time per 4-sample batch.

| Brush | Canvas | Total ms | Commit ms | Batch p50 / p95 / max ms | Peak RSS |
|---|---|---|---|---|---|
| pencil.rough | 2048² | 40.7 | 4.9 | 0.064 / 0.147 / 1.020 | 11.4 MiB |
| pencil.smooth | 2048² | 44.3 | 22.3 | 0.036 / 0.091 / 1.408 | 12.5 MiB |
| pen.technical | 2048² | 39.5 | 21.7 | 0.030 / 0.076 / 0.229 | 12.6 MiB |
| pen.brush | 1024² | 30.6 | 14.1 | 0.029 / 0.071 / 0.145 | 11.5 MiB |
| pen.brush | 2048² | 51.1 | 21.9 | 0.056 / 0.113 / 0.193 | 12.8 MiB |
| pen.brush | 4096² | 103.4 | 42.9 | 0.115 / 0.233 / 0.381 | 21.7 MiB |
| marker | 2048² | 60.9 | 21.4 | 0.070 / 0.143 / 2.487 | 13.3 MiB |
| charcoal | 2048² | 70.3 | 6.1 | 0.127 / 0.213 / 0.412 | 11.9 MiB |
| round.hard | 2048² | 59.0 | 23.2 | 0.064 / 0.151 / 0.256 | 13.5 MiB |
| round.soft | 2048² | 77.9 | 9.4 | 0.128 / 0.257 / 0.495 | 14.5 MiB |
| round.soft (8000 samples) | 4096² | 165.0 | 16.5 | 0.071 / 0.140 / 0.592 | 25.2 MiB |
| eraser.hard | 2048² | 21.4 | 0.3 | 0.041 / 0.073 / 0.227 | 11.4 MiB |

Vector-eligible presets (pen.*, pencil.smooth, round.hard, marker) spend ~20–43 ms in commit
(outline fitting); the commit runs once per stroke, after the pen lifts.

Brush crate probe (`cargo test -p veskra-brush --release --test perf -- --ignored --nocapture`,
4096² canvas, 40 px, 2000 samples): round.hard **99.1 ms total = 49.5 µs/sample** (add_samples
alone 17.1 µs/sample); round.soft (BuildUp) **85.2 ms = 42.6 µs/sample** (22.6 µs/sample).

## 3. Compositing and raster operations

`cargo test -p veskra-compose --release --test golden_perf -- --ignored --nocapture`
(2048², 12 half-filled layers cycling Normal/Multiply/Screen/Overlay):
full `composite_grid` **122.3 ms**; 512² dirty `composite_rect` **12.11 ms**; 512² `render_display_rgba8` **12.72 ms**.

`cargo test -p veskra-raster --release -- --ignored --nocapture`:

| Operation | Time |
|---|---|
| `select::rect` 4096² | 50.6 ms (3969 tiles, 1 019 904 B) |
| `select::polygon` 2000 points, ~3000 px | 229.0 ms (1884 tiles) |
| `fill::region` empty 4096² | 148.7 ms (16.8 Mpx, 4 259 864 B work) |
| `fill::apply` 4096² opaque | 15.5 ms (4096 tiles) |
| transform 1024² rotate | 133.6 ms (296 tiles) |
| 20 000 random 32×32 rect paints | 1.091 s (54.55 µs/rect, 18.8 Mpx/s) |

## 4. Document I/O and exports — 2048 × 2048, 12 layers (INT-02 document)

`cargo test -p veskra-qa --release --test perf_host -- --ignored --nocapture`: the S2 document
(12 layers incl. group, clipping, Multiply/Screen/Overlay, 17 strokes, fills) built through
engine commands at 2048² in 319.9 ms. Median of 5 repetitions:

| Operation | Median | Notes |
|---|---|---|
| `document.save` | **1 148.3 ms** | 1 759 325 B; encode + fsync + re-read + full decode + document comparison + rename + 256 px preview |
| `document.open` | **423.1 ms** | full integrity/decode/validate (MASTER_PLAN target for reopen: ≤ 3 s p95 *on device*) |
| `document.validate` | 416.2 ms | decode without opening |
| full composite (straight RGBA8) | 96.6 ms | `veskra-compose` reference CPU compositor |
| export PNG | 744.9 ms | 144 826 B |
| export PSD | 372.3 ms | 1 743 696 B |
| export SVG (both toggles) | 559.0 ms | 205 077 B |
| export PDF (both toggles) | 324.3 ms | 122 746 B |

Peak RSS of the run: 140.5 MiB (upper bound, see §1).

## 5. Mapping to VERIFICATION_MATRIX

| ID | Status | Host context (not evidence) |
|---|---|---|
| PERF-001 (≥ 60 FPS, 2048 px sketch) | **NOT_RUN — B-2** | 512² dirty composite 12.1 ms; display conversion 12.7 ms; 4-sample stroke batch p95 ≤ 0.26 ms. No GPU/presenter/frame pacing measured. |
| PERF-002 (p50/p95 pen latency) | **NOT_RUN — B-2** | engine-only batch latency above; event-to-photon needs device instrumentation (QAP-05). |
| PERF-003 (4096², 20 layers, 30–60 min, no OOM) | **NOT_RUN — B-2** | pen.brush 4096² stroke peak RSS 21.7 MiB; no long session run. |
| PERF-004 (save/export never stalls pen pipeline) | **NOT_RUN — B-2** | save 1.15 s / exports 0.32–0.74 s at 2048² must run off the input thread (snapshot API exists: `export_snapshot`). |
| PERF-005 (100 surface destroy/recreate, no handle leak) | **NOT_RUN — B-2** | no surface on host. |

Related robustness note: a single far off-canvas stroke can cost 1.6 s / 145 MiB on this host
(QA-D3, `docs/reports/qa-fuzz.md`).
