# Illustrator handoff (SVG / PDF) — interim open-source evidence

**Tickets:** FMT-12 (plan), FMT-13 (SVG), FMT-14 (PDF), FMT-15 (automated part), SPK-07 (open-source part).
**Owner:** FMT-B. **Date:** 2026-10-09. **Source:** FMT-B sources as in checkpoint `c10b7f2` (measured on the identical working tree).

> **Adobe Illustrator: NOT TESTED — ILL-001/002/003/005/007/009 ADOBE evidence BLOCKED (R-14); ADR-012 remains Proposed.**
> Everything below was measured with open-source tools (librsvg, poppler, qpdf, pypdf, Inkscape). It shows the
> files are valid and render without double drawing in those programs. It is **not** evidence of Illustrator
> behaviour, and no Illustrator compatibility claim may be made from it. Manual steps:
> `scripts/interop/illustrator_manual_checklist.md`.

## 1. What was built

* `crates/veskra-io/src/plan.rs` — `IllustratorPlan::build(doc, opts, HybridStrategy)`: classifies every leaf
  layer (paint order, group structure kept for hidden resources), builds the visible stack (raster runs
  composited with `veskra-compose`'s `layer_filter`, visible vector layers), hidden vector edit resources,
  counts and typed warnings; preflight of both formats is derived from it.
* `svg.rs` (+ `svg/png.rs` private PNG encoder with `iCCP`, `svg/xml.rs` well-formedness checker) — SVG 1.1.
* `pdf.rs` (+ `pdf/validate.rs` independent parser) — hand-written PDF 1.7, OCGs, ICCBased sRGB, SMask images.

### Strategy behaviour (V = "Editable vector strokes", R = "Preserve original appearance")

| V, R | `RasterVisibleVectorsHidden` (default) | `VectorsVisibleRasterResidual` |
|---|---|---|
| on, on | One visible raster run = exact composite of all visible layers ("Original appearance"); vector forms of `Exact` + `PartiallyVector` layers in hidden "Editable vectors" (SVG `display="none"`, PDF OCG in `/OFF`) | `Exact` layers → visible vector outlines; others → raster runs; their vector forms hidden. `Exact` layers involved in clipping, under a group with opacity < 1 / non-Normal isolated blend, or with own non-Normal blend inside an isolated group stay raster (exact appearance; warned as `Other`) |
| on, off | `Exact` → visible vectors (`AppearanceOmitted`); others → **forced raster** "Raster-only content" (`LayerRasterized`, `RasterOnlyStroke`) with their vector forms hidden; clip clusters mixing both go raster as a whole | same |
| off, on | One exact raster run; `VectorOmitted` (count) | same |
| off, off | Blocked (ILL-004): preflight `blocked_reason`, `export` → `InvalidArgument`, no file | same |

`RasterAuthoritative` layers (fills, transforms, masks, pixels without strokes) never produce vectors.
Hidden layers are not exported (`HiddenLayer` count). Other warnings: `BlendModeApproximated` (non-Normal
blend in a raster run above another visible item; group opacity/blend applied per item with R off),
`ClippingApproximated` (clipped vector layers with R off), `BlendSpaceMismatch` (linear documents),
`ProfileNotEmbedded`, `Other` for non-sRGB → sRGB conversion.

### Geometry and color

* SVG: `width="{w}px" height="{h}px" viewBox="0 0 w h"`, 1 user unit = 1 document px (physical size is not
  representable in px units; dpi recorded in `data-veskra-dpi` and `<desc>`).
* PDF: `MediaBox = [0 0 w·72/dpi h·72/dpi]` pt (2048×1536 @ 300 dpi → 491.52 × 368.64 pt). Vector content under
  `72/dpi 0 0 −72/dpi 0 H cm` (document px, y down); images use upright matrices in page space.
* Colors: sRGB values of `StrokeRecord::color`; fill-opacity = `color.a × stroke opacity`; layer opacity/blend
  via SVG group `opacity`/`mix-blend-mode`, PDF isolated transparency-group forms with `/ca` + `/BM`.
* ICC (ILL-005): SVG embeds Veskra's sRGB profile only in the PNGs (`iCCP`); vector colors are plain sRGB
  values (report string says so). PDF uses `[/ICCBased <sRGB, /N 3, /Alternate /DeviceRGB>]` for images and
  fills. With the option off: no profile anywhere, `ProfileNotEmbedded` warning, `profile_embedded=false`.

## 2. Commands and tool versions

```
cargo test -p veskra-io                                  # all FMT-B unit + integration tests
scripts/interop/svg_pdf_oracles.sh                       # interop test + pypdf + Inkscape (writes target/interop/illustrator/)
VESKRA_COPY_SAMPLES=1 scripts/interop/svg_pdf_oracles.sh # also refreshes tests/fixtures/exports/illustrator/
```

| Tool | Version | Used for |
|---|---|---|
| rsvg-convert (librsvg) | 2.58.0 | SVG render at 1:1 |
| pdftocairo / pdftoppm (poppler) | 24.02.0 | PDF render (`-png -transp -r dpi -scale-to-x/y`), white-background render at `-r 72` |
| qpdf | 11.9.0 | `qpdf --check` on every PDF |
| pypdf | 6.17.0 | independent parse: 1 page, MediaBox, OCG names + `/OFF`, image data length |
| Inkscape | 1.2.2 (b0a8486541) | SVG render, SVG plain-SVG round trip, PDF import with `--pdf-poppler=false` |
| Rust toolchain | 1.97.0 | tests |

## 3. Results (measured 2026-10-09)

Fixtures (built in `tests/svg_pdf_interop.rs`): `ink_vector_only` (3 pressure-tapered ink strokes incl. a 50 %
stroke), `textured_raster_only` (fill + 2 airbrush/texture strokes), `hybrid` (paper fill, color layer with ink
+ texture, ink layer with a 50 % stroke), `blend_multiply`, `group_opacity` (group 60 % with 2 overlapping
vector layers), `large_2048x1536_300dpi`. Each exported as both-default, both-residual, vector-only,
appearance-only and both-default without ICC, in SVG and PDF: **54 files + 2 negative controls**.

Metric: render vs Veskra's own composite (`veskra-compose`), premultiplied RGBA8: max abs diff / mean abs diff
/ % pixels whose worst channel differs by > 16. "Probe" = alpha at 9 interior points of the 50 % stroke
(single composite = 128; a doubled stroke = 192).

| Case (SVG via rsvg-convert, PDF via pdftocairo — identical numbers for both formats) | max / mean / % > 16 | 50 % probe |
|---|---|---|
| Outputs whose visible content is Veskra's embedded composite (both-default, appearance-only, no-ICC, raster-only fixtures, residual where nothing is exact-vector; 38 of 54 files) | **0 / 0 / 0** (bit-exact) | 128 (all) |
| `ink_vector_only` vectors visible (residual, vector-only) | 27 / 0.0167 / 0.0142 | 128 |
| `hybrid` vectors visible (residual, vector-only) | 23 / 0.0122 / 0.0026 | 128 |
| `blend_multiply` vectors visible (Multiply outline over raster) | 16 / 0.0093 / 0 | n/a |
| `large_2048x1536_300dpi` residual | 28 / 0.0010 / 0.0011 | 128 |
| `group_opacity` vector-only (R off: group opacity applied per layer; `BlendModeApproximated` warned in preflight) | 61 / 0.80 / 2.96 | n/a |
| **Negative control**: `hybrid-both-default` with the hidden group forced visible (SVG `display` removed / PDF `/OFF` disabled) | — | **192** (doubling detected) |

Vector-visible differences are confined to anti-aliased outline edges (brush raster vs renderer AA).

* `qpdf --check`: 27/27 exported PDFs "No syntax or stream encoding errors".
* pypdf: 29/29 PDFs (incl. controls) parsed with `strict=True`; MediaBox e.g. `[0, 0, 491.52, 368.64]` for the
  300 dpi file; default-strategy hybrids list `/OFF ["Editable vectors"]`, residual lists
  `/OFF ["Editable vectors (hidden edit resources)"]`; ICCBased vs DeviceRGB images as requested.
* pdftoppm `-r 72` (Splash backend, white background) on `hybrid-both-default.pdf`: 50 % probes match the
  single composite within 12; global diff 152 / 1.32 / 4.07 %. Investigation: vectors match, but Splash
  resamples image XObjects even at 1:1 (edges shifted/blurred by up to 1 px; a pure 1-row shift on another
  fixture). pdftocairo (Cairo backend) of the same file is bit-exact, so this is a Splash rendering trait, not a
  file defect; recorded, not worked around.
* Inkscape 1.2.2 SVG render: same numbers as rsvg for all 27 SVGs (raster-visible 0/0/0). SVG plain-SVG
  round trip keeps **all** paths, visible and hidden (e.g. `hybrid-both-default.svg`: 3/3; hidden group still
  `display:none`).
* **Inkscape 1.2.2 PDF import (`--pdf-poppler=false`): visible vector paths import as editable `<path>`
  objects (counts equal), but content in OCGs that are `/OFF` is dropped**: 0 of 3 hidden paths from
  `hybrid-both-default.pdf` (0/2, 0/1, 0/3 for the other default-strategy PDFs); residual
  `hybrid-both-residual.pdf` imports its 2 visible paths, not the 1 hidden one. The negative control (OFF
  disabled) imports 3.

Automated checks (all run, all pass): plan matrix 4 option combos × 2 strategies × fidelity kinds
(`Exact`, `PartiallyVector` with/without vector strokes, `RasterAuthoritative` fill, pixels without strokes,
empty, hidden, group opacity, clip base+clipper, Multiply) asserting *no stroke both visible as vector and in a
raster* and *no content dropped*; ILL-004 blocked in preflight/export/`export_to_path`; ILL-006 preflight
counts (2 raster-only strokes) = report counts for every combination; ILL-008 `.ai` / `.pdf.ai` rejected by
`export_to_path`, no `.ai` MIME, files state "not a native .ai file"; ILL-005 iCCP present iff embedding,
PDF ICC stream inflates to exactly `IccProfile::srgb()` bytes; byte-determinism; cancellation.

## 4. Findings and risks for ORCH / QAP

1. **PDF default strategy risk (ILL-003).** A PDF importer that honours OCG `/OFF` at import time drops the
   hidden "Editable vectors" (Inkscape does). If Illustrator behaves the same, the default PDF keeps the exact
   appearance but loses the editable vectors (no data loss in the Veskra file, but the handoff goal fails).
   ADR-012's fallback (`VectorsVisibleRasterResidual`) imports visible vectors correctly in Inkscape. Decide per
   format after the Illustrator test; the SVG default is not affected in Inkscape.
2. Residual strategy shows the outline fill, not brush pixels, for exact vector layers (max 27/255 at AA edges
   in these fixtures; textured brushes are never vectorized).
3. With appearance off, group opacity/blends are applied per item (warned) — measurable on overlaps (2.96 % of
   pixels in `group_opacity`).
4. SVG has no physical units here (px); physical size is only carried by PDF MediaBox.

## 5. Limitations

* **Deviation from ADR-012 wording (needs ORCH confirmation):** "Original appearance" holds one image per
  raster *run* — in the default strategy a single exact composite of all visible layers — not one image per
  layer. This keeps blend modes, clipping, masks, pass-through/isolated groups and group opacity exactly as the
  reference compositor renders them; per-layer images would require re-expressing those semantics in SVG/PDF.
  Layer names of a run are kept in the image label (`Appearance: Paper, Color`).
* Hidden layers are omitted, not exported hidden. Embedded rasters are at document resolution
  (`raster_scale` is ignored for SVG/PDF). Non-sRGB documents are converted to sRGB (relative colorimetric).
* PDF hidden resources are flattened per layer (group nesting only in SVG); per-layer OCGs named after layers.
* `pdf::validate_pdf` supports only the classic xref tables Veskra writes (no object streams/incremental updates).
* Adobe Illustrator behaviour (selection, recolor, hidden-group survival, ICC interpretation, artboard, Save As
  `.ai`): **NOT TESTED**.
