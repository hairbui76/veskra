# ADR-008: Illustrator handoff via SVG/PDF; no native `.ai`
Status: Accepted
Date: 2026-10-09
Owner: FMT + BRH
Related tickets: SPK-07, FMT-12..15, ILL-001..009

## Decision
* Formats: **SVG 1.1** and **PDF 1.7**, labelled "Export for Illustrator (SVG/PDF)". There is no
  `.ai` format variant in the code (`ExportFormat` has none) and no `.ai` extension/MIME anywhere.
* Toggles (`ExportOptions`): `includeVectorPaths`, `includeRasterAppearance`, `embedColorProfile`;
  both of the first two off → export blocked with an actionable message (ILL-004).
* Vector content = strokes with a vector form (`StrokeRecord::has_vector_form`) on layers whose
  `VectorFidelity` is `Exact` or `PartiallyVector`. Each stroke is a **filled variable-width outline
  path** (nonzero), colored with its sRGB color and opacity — editable/recolorable paths, not
  Illustrator pressure brushes (MASTER_PLAN §2.3).
* Raster-only strokes and `RasterAuthoritative` layers are **never** emitted as vectors; they are
  counted in preflight. If the user disabled raster appearance, such content is still embedded as
  tagged sRGB raster (to avoid silent data loss) and reported with `RasterOnlyStroke`/`LayerRasterized`
  warnings — the preflight states this before export.
* Embedded rasters are 8-bit straight-alpha sRGB PNG (SVG, with `iCCP`/`sRGB` chunk when
  `embedColorProfile`) or Flate images with `/SMask` and an `ICCBased` color space (PDF).
* Hybrid visibility: ADR-012.

## Acceptance experiment
ILL-001..009 in Adobe Illustrator — **BLOCKED** (no Illustrator access, R-14). Interim open-source
evidence: rsvg/Inkscape (SVG) and poppler/qpdf/Inkscape (PDF) render and parse the files; Inkscape
imports paths as editable objects. This is not Illustrator evidence.

## Rollback / fallback
SVG-first if the PDF writer fails Illustrator tests.

## Review
ORCH 2026-10-09.
