# Adobe Illustrator manual checklist — ILL-001 … ILL-009

**Purpose:** the steps a human with a licensed Adobe Illustrator must perform to produce the ADOBE
evidence that VERIFICATION_MATRIX §2.6 requires. Automated/open-source results
(`docs/reports/illustrator-export.md`) are **not** a substitute. Until this checklist is completed and
reviewed, ADR-012 stays *Proposed* and Illustrator compatibility must be described as **unverified**.

Never record a result you did not observe. Leave `NOT_TESTED` where a step was not performed.

## 0. Prepare

1. Record: Illustrator version (Help → About, e.g. `29.x`), OS + version, machine, color settings
   (Edit → Color Settings: name of the setting, RGB working space), date, tester.
2. Generate fresh files on the commit under test (repository root):
   ```
   VESKRA_COPY_SAMPLES=1 scripts/interop/svg_pdf_oracles.sh
   ```
   Files: `target/interop/illustrator/` (all combinations) and `tests/fixtures/exports/illustrator/`
   (samples). Each fixture also has `<fixture>-reference.png` = Veskra's own composite.
3. Copy the files to the Illustrator machine **without renaming**. Never rename to `.ai`.

## 1. Opening rules

* SVG: File → Open → choose the `.svg`. Note any import dialog/warnings verbatim.
* PDF: File → Open → choose the `.pdf`. If a page/“Import PDF” dialog appears, keep defaults; note it.
* Take a screenshot at 100 % zoom of the initial display immediately after opening (before toggling
  anything). Name it `<file>-initial.png`.

## 2. Test steps

| ID | Files | Steps | Pass condition |
|---|---|---|---|
| ILL-001 | `ink_vector_only-vector-only.svg`, `.pdf`; `hybrid-vector-only.svg/.pdf` | Open. Select All (⌘/Ctrl-A). In the Layers panel expand everything. Click one stroke with the Direct Selection tool (A); change its fill color to pure green; move one anchor point. | Each stroke is a separate selectable filled path (count paths in Layers = report's `vector_paths`); recolor and anchor edit work; no embedded image is required for the ink. |
| ILL-002 | `textured_raster_only-appearance-only.svg/.pdf`, `hybrid-appearance-only.svg/.pdf` | Open. Compare with `<fixture>-reference.png` placed on top (File → Place, align to artboard 0,0, Difference blend mode). Check Links/Document Info for the embedded image size. | Image is at x=0/y=0 offset as in Veskra (position/scale exact, no 1-px offset), transparency preserved (no white box), colors visually match; Document Info lists embedded image(s). Record any color shift (with eyedropper RGB values at 3 points). |
| ILL-003 | `hybrid-both-default.svg/.pdf` (strategy RasterVisibleVectorsHidden), `hybrid-both-residual.svg/.pdf` | Open. Screenshot initial display. Zoom to the horizontal 50 % blue stroke near the bottom. In Layers, find "Editable vectors" (default) / "Editable vectors (hidden edit resources)" (residual); toggle its visibility on; select and recolor one path; toggle off again. | Initial display shows the blue stroke at 50 % (not darker → no double rendering); the hidden vector group **exists after import**, can be shown and its paths edited. **If the hidden group is missing (expected risk for PDF: Inkscape's PDF importer drops OCG-hidden content), record FAIL and the strategy that works.** |
| ILL-004 | (Android UI) | In the app's Export for Illustrator sheet uncheck both "Editable vector strokes" and "Preserve original appearance". | Export button disabled / blocked with the actionable message. (Engine side is AUTO-tested.) |
| ILL-005 | `hybrid-both-default.svg/.pdf` and `hybrid-both-default-noicc.svg/.pdf` | Open each. Edit → Assign Profile / Document Info: note the document profile and whether Illustrator reports an embedded profile; for PDFs note any "embedded profile mismatch" dialog. Eyedropper the same 3 pixels in both files. | Behavior matches the export report's `profile` string: embedded ICC is detected for the `embedColorProfile` variant; the no-ICC variant opens as untagged/working-space RGB; sampled values do not shift beyond what the dialog explains. |
| ILL-006 | (Android UI) | Export sheet preflight for a document with airbrush/texture strokes. | Shows "M raster-only strokes" and warning before export. (Engine counts are AUTO-tested.) |
| ILL-007 | `large_2048x1536_300dpi-both-default.svg/.pdf`, `hybrid-both-default.svg/.pdf` | Document Setup / Artboard Options: record artboard width/height and units. Layers panel: record top-level layer/group names. | PDF artboard = 491.52 × 368.64 pt (6.827 × 5.12 in) for the 2048×1536 @ 300 dpi file; SVG artboard = 2048 × 1536 px (Illustrator treats 1 px = 1 pt); hybrid artboard 480 × 320. Group names "Original appearance", "Editable vectors" (and layer names "Color", "Ink lines") visible as labels. |
| ILL-008 | — | File names and Save dialog default extension of exported files. | Files are `.svg` / `.pdf` only; no `.ai` produced by Veskra (AUTO-tested). |
| ILL-009 | `hybrid-both-residual.pdf`, `hybrid-both-default.svg`, `ink_vector_only-vector-only.svg` | File → Save As → Adobe Illustrator (`.ai`), default options. Close, reopen the `.ai`. Repeat ILL-001 edits on the reopened file. | Save succeeds; reopened `.ai` keeps the same number of editable paths and appearance. Record Illustrator version. |

## 3. Results template (VERIFICATION_MATRIX §4 interop YAML) — one block per file

Fill only measured values. Unperformed fields stay `NOT_TESTED`; numbers are never estimated.

```yaml
- fixture: hybrid-both-default.svg
  test_ids: [ILL-003, ILL-005, ILL-007]
  application: Adobe Illustrator
  application_version: NOT_RECORDED
  machine: NOT_RECORDED
  color_settings: NOT_RECORDED
  opened_without_error: NOT_TESTED
  editable_paths: NOT_TESTED          # count seen in Layers panel
  hidden_vector_group_present: NOT_TESTED
  appearance_match: NOT_TESTED
  icc_interpretation: NOT_TESTED
  visible_duplicate_strokes: NOT_TESTED
  artboard_size: NOT_TESTED
  layer_labels: NOT_TESTED
  save_as_ai: NOT_TESTED              # ILL-009
  screenshot_reference: null
  reviewed_by: null
- fixture: hybrid-both-default.pdf
  test_ids: [ILL-003, ILL-005, ILL-007]
  application: Adobe Illustrator
  application_version: NOT_RECORDED
  machine: NOT_RECORDED
  color_settings: NOT_RECORDED
  opened_without_error: NOT_TESTED
  editable_paths: NOT_TESTED
  hidden_vector_group_present: NOT_TESTED
  appearance_match: NOT_TESTED
  icc_interpretation: NOT_TESTED
  visible_duplicate_strokes: NOT_TESTED
  artboard_size: NOT_TESTED
  layer_labels: NOT_TESTED
  save_as_ai: NOT_TESTED
  screenshot_reference: null
  reviewed_by: null
# Copy the block for: hybrid-both-residual.{svg,pdf}, ink_vector_only-vector-only.{svg,pdf},
# textured_raster_only-appearance-only.{svg,pdf}, hybrid-appearance-only.{svg,pdf},
# hybrid-both-default-noicc.{svg,pdf}, large_2048x1536_300dpi-both-default.{svg,pdf}.
```

## 4. After testing

* Attach screenshots and the YAML to the QAP evidence folder; update `docs/reports/illustrator-export.md`
  (Illustrator section) and, with ORCH, ADR-012 status/default strategy (per format if SVG and PDF
  behave differently).
