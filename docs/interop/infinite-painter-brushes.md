# Infinite Painter brush files (`.prbr`, `.przp`) — what Veskra relies on

Status: import implemented (ADR-019), **approximate by design**, not verified against a real
commercial pack or the app itself (the owner will supply a `.przp`; until then every claim below
rests on the public sources listed here).

Veskra's importer is an independent implementation. No Infinite Painter code or asset is used or
shipped: the built-in heads and textures an IP brush can reference by name are **not** in the
file and are never copied from the app; the importer replaces them (round tip / procedural grain)
and says so.

## Sources

| Tag | Source | License | Used for |
|---|---|---|---|
| **CONV** | `infinite-painter.ts`, `template-properties.json`, `Projet Convertisseur ABR.md` of <https://github.com/Iurika-N-T-R/ABR-to-PRZP-Brush-converter> | CC0-1.0 | layout, field meanings, calibration readings "CAL 1–6" done in the app by that project's author |
| **MANGA** | `manga_inker.json` of <https://github.com/CanRau/infinite-painter-brushes> | Unlicense | one real `properties.json` exported by the app |

Both JSON files are committed as test fixtures in `tests/fixtures/brushes/infinite_painter/`
(see `LICENSES.md` there). Certainty levels used below:

* **confirmed** — CONV reports a calibration reading in the app (CAL n) or a check over many
  real files (e.g. "230/230 hashes verified");
* **observed** — seen in real files (CONV samples or MANGA), meaning inferred;
* **assumed** — CONV's or Veskra's guess; marked *unverified*.

## Container layout

* `.przp` (pack) = ZIP, entries STORED in IP's own exports (CONV, observed). Contents:
  `index.json` and `Brushes/<name>.prbr`.
  `index.json` = `{"version": 1, "brush-folders": [{"version": 3, "name": "<folder>",
  "selected": n, "brushes": [{"name": "<file name without .prbr>", "id": n, "hidden": bool}]}]}`
  (CONV, observed). One pack sample could not be read by JSZip but was by Python/unzip (CONV);
  Veskra's reader therefore accepts data descriptors and bytes prepended to the archive (*assumed*
  to be the cause; unverified).
* `.prbr` (one brush) = ZIP with `properties.json` (required), `preview` (PNG, 512×128,
  optional), `head` (PNG, optional) and `texture` (PNG, optional) (CONV, observed: 260/260
  previews 512×128, 32/208 brushes without preview).
* `source-properties.custom-head` / `custom-stroke texture` = BLAKE2b-512 (hex) of the `head` /
  `texture` entry (CONV, confirmed 230/230). Veskra does **not** verify these (no BLAKE2b
  implementation); it identifies assets by its own content hash after conversion.
* `source-properties.head` / `stroke texture` = names of IP built-in assets (e.g. `texture_8`,
  `brush_head_proko_pencil`), empty when a custom image is used (CONV, observed).
* Heads up to 3072 px and textures up to 2732 px were seen (CONV). Veskra keeps masks ≤ 2048 px
  and box-downscales larger images by an integer factor (reported as approximated).

## Field mapping

`exact` = same meaning; `approx` = mapped with a known or suspected difference (listed in the
brush's `approximated`); `unsupported` = active feature dropped or replaced (listed in
`unsupported`); `ignored` = no effect expected.

### `stroke-properties`

| Field | Veskra | Status | Source / certainty |
|---|---|---|---|
| `paint-size` × `size-maximum` | `size_px = 3.74 × paint-size × size-maximum`, clamped 0.5..1000 | approx | CONV CAL 6: one reading (47 × 4.4 → 773 px) + "~500 px" for the template; canvas-dependent per IP docs. *Unverified* beyond that |
| `paint-opacity` | `opacity` | exact | CONV CAL 6 (×100 in the app) |
| `lazy-stroke` | `stabilization` | approx | CONV CAL 6 (×100); smoothing algorithm differs |
| `wet-edges` | `wet_edges = min(1, 2 × value)` | approx | CONV CAL 6: 0.53 shows as 100 %, "0.5 = 100 %"; Veskra's wet-edge model is its own |
| `blendmode` ≠ 0 | — | unsupported (`blend mode N`) | CONV CAL 6: 0 = Normal; other values not mapped |
| `glaze-strokes` ≠ 0 | — | unsupported | CONV CAL 6 |
| `adhesion` ≠ 0 | — | unsupported | CONV CAL 6 |
| `has-size-profile` (+ `size-profile`) | — | unsupported (taper along the stroke) | CONV CAL 6 |
| `has-flow-profile` (+ `flow-profile`) | — | unsupported (fade along the stroke) | CONV CAL 6 |
| `erase-size`, `erase-opacity`, `blend-size`, `blend-opacity`, `blendmode-intensity` | — | ignored | CONV: settings of the app's eraser / blender modes |

### `head-properties` and the tip

| Field | Veskra | Status | Source / certainty |
|---|---|---|---|
| `spacing` | `spacing` (fraction of the head width; Veskra: of the dab diameter), clamped 0.005..4 | exact for square heads | CONV: IP docs say "unit = width of the head bitmap"; range 0.005..2 observed |
| `angle` (radians) | `tip.angle` in degrees, counter-clockwise | exact | CONV CAL 2: positive = counter-clockwise; radians confirmed (π, ≈2π seen) |
| `rotation` (−1..1) | ≥ 0.5 → `rotation: direction`; ≤ −0.5 → direction + 180°; other non-zero → fixed | exact at 0 / ±1, approx otherwise | CONV CAL 4 (1 = follows stroke), IP docs "−100 inverse, 0 fixed, 100 tangent" |
| `use-trajectory` | `rotation: pen` (uses hardware azimuth only when reported) | approx when combined with `rotation` | CONV CAL 5 ("Rotation du stylet") |
| `softness` | round tips: `tip.hardness = 1 − softness` | approx (falloff shape differs) | CONV: softness = 1 − Photoshop hardness (Syntetyc pair) |
| `flow` | `flow` (clamped ≥ 0.001) | exact | CONV CAL 6 |
| `screen-aligned` | — | unsupported | CONV CAL 6 |
| `depth`, `structure` ≠ 0 | — | unsupported (`head depth`, `head structure`) | CONV CAL 6 |
| `style` ≠ 0 | normal style | unsupported (`head style N (meaning unknown …)`) | CONV CAL 6: 0 = Normal; MANGA uses 1, meaning unknown |
| `head` entry (PNG) | image tip; coverage from `head - conversion format`: 1 → black = paint (darkness × alpha), 0 → white = paint (luminance × alpha), missing → auto | approx (`downscaled` when > 2048 px) | CONV CAL 6: 1 = "Inverser" (black = paint); recent packs use white-on-black with transparent border and format 0 |
| `head - profile` ≠ linear | curve applied to the mask values | approx | CONV: `[0,1,1,0]` = linear; tone-curve meaning *assumed* |
| `head - color` | mask only | unsupported (`colored head`) | CONV CAL 6 |
| custom head hash set but no `head` entry | round tip | unsupported | — |
| `source-properties.head` = built-in name | round tip | unsupported (`built-in head "<name>" replaced by round tip`) | CONV: built-in assets are not in the file |
| `parent` | — | ignored | CONV: meaning unknown |

### `texture-properties` and the texture

| Field | Veskra | Status | Source / certainty |
|---|---|---|---|
| `texture` entry (PNG) | image texture, luminance = coverage (white lets paint through) | approx | *assumed* (PS multiply convention); unverified in the app |
| `pressure` | `texture.depth = 1 − pressure` | exact | CONV CAL 6: "Profondeur = 1 − pressure" |
| `scale` (0..2) | `texture.scale` = texel size in document px | approx | CONV: range 0..2, default 1; IP's reference size *unknown* |
| `scale-size` | texture sized once to the brush size | approx | CONV CAL 6 / IP docs ("texture at brush size") |
| `style` | 0 → `mode: fixed`, 1 → `mode: follow`, 2 (deform) → fixed + unsupported | exact for 0/1 | CONV: IP docs "Fixe / Tourner / Déformer"; 0/1 assignment partly *assumed* (CONV "ponytail") |
| `invert` | `texture.invert` | approx | CONV CAL 6: the app's Invert button does **not** follow this field |
| `softness`, `structure`, `angle` ≠ 0, `stretch` ≠ 1 | — | unsupported | CONV CAL 6 |
| `stroke texture` = built-in name | procedural grain (scale ≈ 4 × IP scale) | unsupported (`built-in texture "<name>" replaced by procedural grain`) | CONV |
| `stroke texture - color` | mask only | unsupported | CONV |

### `jitter-properties`

| Field | Veskra | Status | Source / certainty |
|---|---|---|---|
| `size` | `jitter.size` | approx (distribution) | CONV CAL 6 (×100) |
| `angle` (1 = 360°) | `jitter.angle` (1 = ±180°) | approx (distribution) | CONV CAL 6: 1 = 360° |
| `flow` | `jitter.flow` | approx | CONV CAL 6 |
| `scatter` | `jitter.scatter` (× dab diameter) | approx — unit *unverified* | CONV: three paired readings only |
| `color-hue` / `-saturation` / `-brightness` | per-dab `jitter.hue` / `saturation` / `brightness` | approx | CONV CAL 6 + IP docs ("continuous" variation) |
| `color-start hue` / `saturation` / `brightness` | per-stroke `jitter.stroke_*` | approx | IP docs ("initial" variation) via CONV |
| `start-angle`, `texture-angle`, `texture-position`, `texture-scale`, `texture-start-position` ≠ 0 | — | unsupported | CONV CAL 6 |

### `dynamics-properties`

`<source> - effects <target>` (booleans) and `<source> profile - <target>` (curves) for sources
`pressure`, `tilt`, `velocity` (CONV CAL 6, MANGA).

| Field | Veskra | Status | Source / certainty |
|---|---|---|---|
| `… - effects size / flow / texture / scatter` | `dynamics.<source>.<size\|flow\|texture\|scatter>.enabled` | exact switch | CONV CAL 6 |
| `… profile - <target>` `[x0,y0,x1,y1,…]` | curve points `(x, 1 − y)`, sorted, extended flat to x = 0 / 1, resampled to ≤ 32 points | exact for in-range, ≤ 32-point profiles; approx otherwise | CONV: y axis inverted, `[0,1,1,0]` = linear (confirmed for pressure; tilt/velocity layout *assumed*) |
| velocity profiles | Veskra speed scale (4 px/ms = 1) | approx | IP's speed normalization unknown |
| `tilt - offset`, `tilt - gradation` | — | approx (ignored) | CONV CAL 6 |
| `… - effects dilution`, `… - effects head depth` | — | unsupported | CONV |

### Paint mixing and special sections

| Field | Veskra | Status |
|---|---|---|
| `blend-properties.mix-in` > 0 | — | unsupported (`paint mix-in`) |
| `blend-properties.mix-wet` | — | unsupported (`wet mix`) |
| `blend-properties.dirty-blending` | — | unsupported |
| `blend-properties.smudge`, `smudge-amount`, `strength`, `flow` | — | ignored (blender-tool settings; CONV CAL 6) |
| `bleed-properties`, `particles-properties`, `pixel-properties`, `filter-properties` present | — | unsupported (section named) |
| `display-name` | `name` (control characters dropped, ≤ 255 bytes; file name when empty) | exact |

Imported brushes get `category: imported`, `source: imported:infinite_painter`, `folder` = the
pack folder (or `Imported` for a single `.prbr`), `accumulation: max`, and `vector_eligible`
computed by Veskra's rule (only plain round brushes qualify). Editing one with `brush.save` turns
it into a `custom` brush.

## What is not verified

* No real commercial `.przp` has been imported; the synthetic test archives follow the layout
  above and use the two public-domain property files.
* No side-by-side visual comparison with Infinite Painter exists. Sizes, spacing, jitter
  distributions, texture scale, wet edges and velocity response are expected to differ.
* The BLAKE2b asset hashes in `source-properties` are not checked.
