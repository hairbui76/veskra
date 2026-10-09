# ADR-012: Hybrid vector + raster visibility strategy
Status: Proposed (Illustrator verification BLOCKED)
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

## Acceptance experiment
ILL-003 in Illustrator: initial display has no doubled marks; hidden vector group survives import and
can be shown and edited. **BLOCKED**. Interim: poppler (`pdftoppm`) and rsvg/Inkscape renders are
compared with the raster-only render to prove no double drawing in those renderers.

## Rollback / fallback
If Illustrator drops hidden content, switch the default to `VectorsVisibleRasterResidual`.

## Review
Pending Illustrator evidence.
