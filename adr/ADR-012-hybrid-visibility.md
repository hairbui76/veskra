# ADR-012: Hybrid vector + raster visibility strategy
Status: Accepted (provisional) — Illustrator verification BLOCKED
Date: 2026-10-09
Owner: FMT + DSN
Related tickets: SPK-07, FMT-13..15, ILL-003, R-06

## Context
With both `Editable vector strokes` and `Preserve original appearance` on, the file must look like the
source with no double-drawn marks while keeping vectors editable.

## Decision (default strategy `RasterVisibleVectorsHidden`, per ARCHITECTURE §H)
* Top-level group **"Original appearance"** (visible): per-layer embedded rasters.
* Top-level group **"Editable vectors"** (hidden): per-layer subgroups of outline paths.
  * SVG: `<g id="editable-vectors" display="none" inkscape:groupmode="layer" ...>` and `data-name`.
  * PDF: an Optional Content Group "Editable vectors" listed in `/OCProperties /D /OFF`.
* Alternate strategy `VectorsVisibleRasterResidual` (implemented, selectable): vectors visible and the
  raster appearance contains **only** content without a vector form, so nothing is duplicated and no
  hidden-object import is needed. Appearance of vector strokes is then the outline's, not the
  brush-pixel rendering.

## Amendment 2026-10-09 (ORCH, from FMT-B open-source evidence)
* **Appearance layer granularity:** "Original appearance" holds one image per *raster run* (in the
  default strategy a single exact composite), not one image per layer. This keeps blend modes,
  clipping, masks and group opacity exactly as Veskra's compositor renders them (renders by
  rsvg-convert 2.58 / pdftocairo 24.02 are identical to the Veskra composite, max diff 0).
* **Per-format default:** Inkscape 1.2.2 imports visible PDF vectors as editable paths but **drops
  all hidden optional-content content** (0/3 paths from `hybrid-both-default.pdf`), while it keeps
  hidden SVG groups on re-save. Until Illustrator evidence exists:
  * **SVG default = `RasterVisibleVectorsHidden`** (pixel-exact appearance; editable vectors hidden).
  * **PDF default = `VectorsVisibleRasterResidual`** (no editable content depends on hidden-layer import).
* No-double-draw evidence: a 50 %-opacity stroke measures alpha 128 in renders of every hybrid
  export; the negative control (hidden vectors forced visible) measures 192.
* Report: `docs/reports/illustrator-export.md`.

## Acceptance experiment
ILL-003 in Illustrator: initial display has no doubled marks; hidden vector group survives import and
can be shown and edited. **BLOCKED**. Interim: poppler (`pdftoppm`) and rsvg/Inkscape renders are
compared with the raster-only render to prove no double drawing in those renderers.

## Rollback / fallback
If Illustrator drops hidden content, switch the default to `VectorsVisibleRasterResidual`.

## Review
Pending Illustrator evidence.
