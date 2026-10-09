# QA headless scenarios S1–S8, fault matrix and oracle run

**Owner:** QAP · **Date:** 2026-10-09 · **Evidence:** AUTO (host, headless engine, SYNTHETIC input).
Nothing here is device, Photoshop or Illustrator evidence. Sub-checks needing those are recorded
`NOT_RUN` with their `docs/BLOCKERS.md` id inside every `result.json`.

## 1. Commands

```sh
export CARGO_TARGET_DIR=/tmp/claude-0/target-qa          # any target dir works
cargo test -p veskra-qa                                   # scenarios + fault matrix + fuzz smoke (+ corpus check)
VESKRA_QA_ENOSPC=1 cargo test -p veskra-qa --test fault_matrix   # + real tmpfs ENOSPC (root/mount only)
bash scripts/interop/run_all.sh                           # every oracle + summary
```
Artifacts: `target/qa/<S1..S8|QAP-03>/` (`result.json`, `.veskra`/PSD/SVG/PDF/PNG outputs, reference
composites, `commands.jsonl` replayable with `veskra run <file> --seed <seed>`, S1 input trace).
`result.json` status: `PASS` (all checks ran and passed), `AUTO_PASS` (all automated checks passed,
some sub-checks `NOT_RUN`), `FAIL` (a check failed; known routed defects carry `knownDefect` and do
not fail `cargo test`, but they do make `run_all.sh` exit 3).

## 2. Scenario results (run 2026-10-09, `cargo test -p veskra-qa --test scenarios`: 8 passed)

| Scenario | Status | Automated checks (all real, see `result.json`) | NOT_RUN |
|---|---|---|---|
| **S1** sketch | AUTO_PASS (11 pass) | 3 stylus strokes through `pack_events → unpack_events → Session::input_batch` (historical + 164 predicted events): 3 commits, 340 stored samples, **0 predicted / 0 off-input samples stored** (STY-007), pressure ∈ [0,1] + reported tilt on all, monotonic t; `ACTION_CANCEL` leaves no trace; stylus-eraser stroke erases to white (layer α 0), undo/redo/undo bit-exact; **two-finger pinch: zoom ×2.000, anchor drift 0.0 doc px, document version unchanged, no paint**; 180 px palm: no paint, no view change; stroke drawn while a 210 px palm rests: exactly 1 stroke, mapped through the zoomed view; save → reopen identical | STY-003/005 HW, STY-008, 30-min sketch (B-2) |
| **S2** ink + color, INT-02 | AUTO_PASS (11) | 12-layer document (group, 2 clipped Multiply layers, Screen, Overlay, hidden sketch, vector Lines/Eyes) built with 46 undoable commands; bucket fill (sample merged) bounded by vector line art; alpha-locked fill keeps transparency; clipped Multiply shadow invisible outside the flat (composite = paper) and darker on it; **undo ×46 = freshly created document, redo ×46 = built document (Document equality + composite)**; save/reopen bit-exact (583 862 B); after reopen 3 edits undo exactly back to the reopened state, 4th undo `NothingToUndo`; edited save/reopen identical | HW coloring session (B-2) |
| **S3** PSD | AUTO_PASS (6) | 1024×768 12-layer PSD: preflight warnings == report warnings; own header/1039 parse (v1, 8-bit RGB, ICC bytes == document); **psd-tools 1.24.0: layer tree + ICC OK; merged image vs Veskra composite: α 0 / opaque color 0 / premul 0 (psd-tools and ImageMagick)**; psd-tools independent recomposite: α 1, opaque color 2, premul max 2.192 (2 px > 2.0) — QAP limit 1/2/3.0; cancelled export leaves no file/temp | Photoshop (B-3); Krita run by `run_all.sh` |
| **S4** vector SVG/PDF | AUTO_PASS (6) | vector-only: 4 visible `<path>`, 0 images, preflight/report vector count 4; rsvg & pdftocairo vs composite: max 146, mean 0.0947, 0.417 % px > 16, **silhouette IoU 0.956, Hausdorff 1.0 px**, 50 % stroke probe α 128 (= composite, no doubling); qpdf clean; MediaBox = px·72/dpi; Inkscape imports 4/4 PDF paths | Illustrator select/recolor/Save As (B-3) |
| **S5** appearance SVG/PDF | AUTO_PASS (6) | appearance-only: image, 0 paths, `vector_omitted` warned, ICC embedded; **renders bit-exact (0/0/0)**, transparent corners | Illustrator (B-3) |
| **S6** hybrid | AUTO_PASS (6) | SVG default: 0 visible + 4 hidden (`display="none"`) paths, render bit-exact, probe α 128; PDF (residual default, ADR-012 provisional): vectors visible, IoU 0.959 / Hausdorff 1.0 px, probe α 128 | Illustrator hide-raster/edit (B-3) |
| **S7** color sanity | AUTO_PASS (10) | 183 CLR swatches filled via selection + `edit.fill_selection`; engine pick exact; PNG with iCCP (== Veskra sRGB bytes) and with sRGB chunk: **png crate and Pillow 12.3.0: max 0 LSB, ΔE00 = 0** vs source; ΔE00 vs colour-science Lab ≤ 7.1e-7; alpha swatches 0/3/64/128/255 exact | Photoshop/Illustrator (B-3), display (B-6) |
| **S8** recovery | **FAIL (known QA-D1)**, 8 pass | 4 autosaves keep 3 (seq 2–4); newest truncated → listed invalid, temp ignored; `document.recover` opens seq 3 (Document equal), `document.info` dirty, never marked saved; original byte-identical; next autosave seq 5; ENOSPC via `checkpoint_with`/`save_atomic_with` seam → `StorageFull`, checkpoints and original unchanged, no temp; stale temp removed. **FAIL: the `document.recover` response itself reports `dirty:false, savedVersion:1`** | device force-stop, SAF, rotation/GPU loss (B-2) |

## 3. QAP-03 fault matrix (`tests/fault_matrix.rs`, 100 cycles)

Every cycle edits (stroke or layer property; a layer every 25 cycles) then saves to the same path.
Cycles `i % 10 ∈ {3,5,7,9}` inject a fault: read-only directory (here: root, so `chattr +i` — `chmod`
is not enforced for root), path into a missing directory, target path is an existing directory,
parent is the user's regular file.

| Fault | Cycles | Invariants held | Error code |
|---|---|---|---|
| none (successful save) | 60 | 60 | — |
| read_only_dir | 10 | 10 | `Io` ×10 |
| missing_dir | 10 | 10 | `Io` ×10 |
| target_is_dir | 10 | 10 | `Io` ×10 |
| parent_is_file | 10 | 10 | `Io` ×10 |
| real ENOSPC, 1 MiB tmpfs (`VESKRA_QA_ENOSPC=1`, run once here) | 1 | 1 | `StorageFull` |

Invariants after each failure: typed recoverable error, previous file byte-identical, decodes to
the last saved snapshot and passes `document.validate`, `dirty` true, `savedVersion` and version
unchanged, no `*.tmp-*` leftovers, directory target and its content intact. After each success:
`dirty` false, file reloads to the session document. Final file = last saved snapshot.
**Known QA-D2:** permission failures surface as `Io`, not `FilePermissionDenied` (which
`veskra_io::export_to_path` uses), so the UI cannot show an actionable "no permission" message.

## 4. Oracle runner (`scripts/interop/run_all.sh`, final run 2026-10-09, 151 s)

`target/interop/summary.json`: **17 steps PASS, 0 FAIL, 0 NOT_RUN; overall `FAIL_KNOWN_DEFECTS`
(QA-D1, QA-D2), exit 3.** Steps: PNG fixtures + Pillow; PSD fixtures + psd-tools/ImageMagick;
SVG/PDF fixtures + rsvg/poppler/qpdf/pypdf/Inkscape (`svg_pdf_oracles.sh`); scenarios; fault
matrix; pypdf + Inkscape on S4–S6; Krita 5.2.2 (xvfb) on the 5 fixture PSDs and the S3 PSD;
color oracle regeneration byte-identical + `veskra-color` oracle test. Tool versions are in
`target/interop/tools.json`.

Krita on the S3 12-layer PSD opens and renders at the right size with exact alpha, but differs from
Veskra's composite (premul max 151, 49 386 px > 2). This is consistent with the Krita 5.2.2 PSD
import limitations already isolated by FMT-A (clipping and group opacity ignored; the S3 document
has two clipped layers and a 90 % group); it was not separately re-diagnosed for this file.
Krita's six/twelve-layer fixture numbers reproduce FMT-A's report (122 / 88).

## 5. Findings for ORCH routing

| Id | Severity | Crate / owner | Finding |
|---|---|---|---|
| QA-D1 | P2 | `veskra-engine` (ORCH) | `document.recover` and `recovery.open` return the `info` receipt computed inside `open()` *before* `saved_version` is cleared: response says `dirty:false, savedVersion:N` while the session (and `document.info`) say dirty/unsaved. A UI trusting the receipt would show a recovered checkpoint as saved. Check: `S8.recover_receipt_reports_unsaved`. |
| QA-D2 | P3 | `veskra-format` (FMT) | `FormatError::from(io::Error)` maps `PermissionDenied` to `Io`; `export_to_path` maps it to `FilePermissionDenied`. Check: `QAP-03.permission_error_code`. |
| QA-D3 | P2 | `veskra-brush` (BRH) + `veskra-input` (AND) | Far off-canvas coordinates: bounded but canvas-independent cost (1.6 s / 145 MiB for one 2-sample stroke on 128×96, host release). See `qa-fuzz.md` §3. |
| O-1 | info | AND / ORCH | `layer.list` ("top-most first") lists a group **after** its children (paint order reversed); the UI must build the tree from `parent`/`depth`. |
| O-2 | info | ORCH | Undoing back to the saved state leaves `dirty: true` (version-based, conservative). |
| O-3 | info | VEC / FMT-B | Visible vector outlines of `pen.brush` strokes deviate ≤ 1 px from the brush raster (max pixel Δ 146 at edges, max fit error 0.48 px); FMT-B's own fixtures showed max 27. PNT-010's signed bound is still pending. |
| O-4 | info | FMT-A | psd-tools' independent recomposite of the S3 stack exceeds FMT-A's ≤ 2.0 premultiplied limit on 2 px (2.192); merged image is exact. |
