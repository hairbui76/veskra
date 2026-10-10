# Engine command reference (normative)

Source of truth: `crates/veskra-engine/src/command.rs`. The Android `:kit` module's `Commands`
builders are contract-tested against the real engine through JNI
(`CommandVocabularyIntegrationTest`). Changes require ORCH approval (AGENTS.md §8).

**Envelope.** Request: one JSON object with `"cmd"` and camelCase fields. Response:
`{"ok": <result>, "version": N, "invalidated": [[x0,y0,x1,y1],…], "warnings": […], "historyChanged": bool, "viewChanged": bool}`
or `{"error": {"code": "<ErrorCode>", "key": "<snake_case>", "message": "…", "recoverable": bool}}`.
Rectangles are half-open document pixels. Mutating commands are atomic and create exactly one undo
step; failures change nothing (including the deterministic ID stream). **Unknown fields are rejected**
(`InvalidArgument`) for commands with fields, placements and samples, so a typo can never become a
silent no-op undo step. `invalidated` is guaranteed to cover every display pixel that changed
(property-tested over 2000 random command/stylus sequences).

| Command | Fields | Result / notes |
|---|---|---|
| `document.create` | `width`, `height`, `dpi`=300, `background`?: `#RRGGBB[AA]` | info; creates "Background" + "Layer 1" (active), sRGB, clears history |
| `document.info` | — | id, size, dpi, profile, blendSpace, version, savedVersion, dirty, counts, activeLayer, undo/redo state, historyBytes/Trimmed |
| `document.save` | `path` | `{bytes, savedVersion, sha256}`; atomic temp→validate→rename (ADR-009). Paths ending in `.ai/.psd/.psb/.pdf/.svg/.png/.jpg` are refused. After undo back to the saved state the document conservatively stays `dirty`. |
| `document.open` | `path` | info + `warnings` (forward-compat) |
| `document.validate` | `path` | fully decodes a `.veskra` without opening it (staged SAF save check) |
| `document.autosave` | `dir` | checkpoint `{path, seq, bytes}`; keeps newest 3 valid |
| `document.recover` | `dir` | opens newest valid checkpoint; never marked saved |
| `recovery.list` | `dir` | `{checkpoints: [{path, documentId, seq, bytes, valid, error}]}` |
| `recovery.open` | `path` | opens one checkpoint; never marked saved |
| `layer.add` | `kind`: raster\|vector\|group, `name`?, `placement`? | `{layerId}`; non-group becomes active |
| `layer.remove` / `layer.duplicate` / `layer.clear` | `id` | — / `{layerId}` / — |
| `layer.mask_from_selection` / `layer.mask_remove` / `layer.mask_apply` | `id` | raster layers only; mask from current selection (whole canvas if none); apply bakes the mask into alpha. A masked layer is `vectorFidelity: raster` |
| `layer.import_image` | `path`, `handling`?: convert (default) \| assign, `missingProfile`?: assume_srgb (default) \| reject, `name`?, `x`?, `y`? | `{layerId, width, height, sourceProfile, profileAssumed, converted, warnings}`; PNG only; corrupt ICC rejected; assumed profiles reported in `warnings` (COL-004/005) |
| `layer.move` | `id`, `placement` | rejects cycles |
| `layer.set` | `id`, `name`?, `visible`?, `opacity`?, `blend`?, `locked`?, `alphaLocked`?, `clip`?, `passThrough`? | one undo step |
| `layer.rename` | `id`, `name` | |
| `layer.select` | `id` | UI state, not undoable |
| `layer.list` | — | `{layers: [...]}` top-most first: id, parent, depth, kind, name, visible, opacity, blend, locked, alphaLocked, clip, passThrough, strokes, hasMask, vectorFidelity (`exact`\|`partially_vector`\|`raster`), active |
| `brush.list` / `brush.select` / `brush.set` | — / `preset` / `size`?, `opacity`?, `flow`?, `stabilization`? | presets with `vectorEligible` |
| `color.set` / `color.get` / `color.pick` | `hex` / — / `x`, `y`, `layer`? | sRGB hex; recent colors |
| `stroke.begin` / `stroke.samples` / `stroke.commit` / `stroke.cancel` | `eraser`? / `samples: [{x,y,t,p?,tilt?,azimuth?}]` | headless stroke path; omitted `p` = no hardware pressure (fallback 0.5, flag cleared) |
| `history.undo` / `history.redo` | — | refused while a stroke is live |
| `selection.rect` / `selection.ellipse` | `x0,y0,x1,y1`, `op`?: replace\|add\|subtract\|intersect | |
| `selection.lasso` | `points: [[x,y],…]`, `op`? | nonzero winding |
| `selection.all` / `selection.clear` / `selection.invert` | — | |
| `edit.fill` | `x`, `y`, `tolerance`=0.1, `contiguous`=true, `sampleMerged`=false | raster layers only |
| `edit.fill_selection` / `edit.delete_selection` | — | raster layers only |
| `edit.transform` | `matrix: [a,b,c,d,e,f]` | selection content or whole layer; raster only |
| `view.size` / `view.fit` / `view.set` / `view.mirror` | `width,height` / — / `zoom?,rotation?,panX?,panY?,mirror?` / `enabled` | `{zoom, rotation, panX, panY, mirror, matrix}` |
| `input.config` | `fingerDraws`?, `stylusButton`?: none\|eyedropper\|toggle_eraser\|undo, `lockView`?: bool | `{fingerDraws, stylusButton, lockView}` (the resulting configuration; call without fields to read it). `lockView: true` = camera lock: finger pan/zoom/rotate gestures are ignored (no `VIEW` flag, view unchanged) while drawing, taps and stylus buttons keep working. Not undoable |
| `input.reset` | — | lifecycle pause: commits confirmed samples of a live stroke |
| `export.preflight` | `format`: png\|psd\|svg\|pdf, `options`? | `ExportPreflight` (counts, warnings, blockedReason) |
| `export.run` | `format`, `path`, `options`? | `ExportReport`; extension must match; `.ai` rejected (ADR-008) |
| `edit.gradient` | `x0,y0,x1,y1`, `kind`=linear\|radial, `stops`?: `[{offset: 0..1, color: "#RRGGBB[AA]"}]` (2..=256), `opacity`=1.0 | `{}`; active raster layer, clipped by the selection, honours alpha lock; see *Raster-edit and canvas tools* |
| `selection.wand` | `x`, `y`, `tolerance`=0.1, `contiguous`=true, `sampleMerged`=false, `op`?: replace\|add\|subtract\|intersect | `{empty}`; same region as `edit.fill` (so wand + `edit.fill_selection` == `edit.fill`) |
| `edit.adjust` | `kind`: hsb\|brightness_contrast\|curves\|invert + the fields of that kind (`hue`, `saturation`, `brightness`, `contrast`, `rgb`, `r`, `g`, `b`) | `{}`; active raster layer, selection-weighted, alpha never changes; fields of another kind are rejected |
| `edit.transform_quad` | `quad: [[x,y]×4]` = top-left, top-right, bottom-right, bottom-left | `{sourceBounds}`; projective map of the selection bounds (or the layer's content bounds) onto the quad; raster only |
| `canvas.crop` | `x0,y0,x1,y1` (rounded to whole px) | `{width, height, offsetX, offsetY}`; whole document; `viewChanged` |
| `canvas.resize` | `width`, `height`, `resample`=false, `anchor`?: top_left\|top\|top_right\|left\|center\|right\|bottom_left\|bottom\|bottom_right (default center, only without `resample`) | `{width, height, resampled, scaleX, scaleY, offsetX, offsetY}`; whole document; `viewChanged` |
| `selection.outline` | `maxPoints`=4000 (8..=100000) | `{empty, bounds, contours, truncated, pointCount}`; read-only (no undo step) |
| `layer.bounds` | `id`? (default: active layer) | `{layerId, bounds}`; read-only (no undo step) |

`placement`: `{"at":"aboveActive"}` (default) \| `{"at":"top"}` \| `{"at":"above","id":…}` \|
`{"at":"below","id":…}` \| `{"at":"into","group":…,"index":n?}`.

`options` (`ExportOptions`): `{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"rasterScale":1.0,"profile":"document","bitDepth":8}`.
Every field after the three checkboxes is optional; JSON without `profile`/`bitDepth` behaves exactly as before
(default-option files are byte-identical, pinned by `crates/veskra-io/tests/export_default_pin.rs`).

### Export color options

* `profile`: `"document"` (default) \| `"srgb"` \| `"display_p3"` \| `"adobe_rgb"`.
  * `document` keeps the document working space: PNG/PSD embed the document's ICC bytes and write the pixel
    values unconverted. Every document Veskra can author is sRGB, so this **is** sRGB output; for those documents
    `"srgb"` produces the identical file. (For a non-sRGB document, `document` stays lossless while `srgb`
    converts.) SVG/PDF are always written as sRGB for `document`/`srgb`.
  * `srgb`, `display_p3`, `adobe_rgb`: pixels are converted from the document space to the target with the
    **relative colorimetric** intent (matrix/TRC profiles through PCS XYZ D50, computed in `f64` on the 16-bit
    composite; colors outside the target gamut are **clipped per channel**, no gamut mapping), and the target's
    ICC profile is embedded: PNG `iCCP`, PSD image resource 1039, PDF `/ICCBased` color space (images **and**
    vector fill colors), SVG `iCCP` of the embedded raster PNGs. When the document already is in the target
    space nothing is converted and the document's own profile bytes are embedded.
  * `adobe_rgb` embeds a Veskra-generated ICC v4 profile with the published colorimetry of Adobe RGB (1998)
    (description "Veskra RGB (compatible with Adobe RGB 1998)"); it is not Adobe's profile file.
  * **SVG cannot carry an ICC profile for vector fills**: with a wide-gamut target the path colors stay sRGB hex
    values and only the embedded raster images are in the target space (`vector_colors_srgb` warning).
  * `display_p3` / `adobe_rgb` with `embedColorProfile: false` is refused (`IncompatibleOutputProfile`,
    preflight `blockedReason`): untagged wide-gamut numbers would be read as sRGB everywhere.
  * With `embedColorProfile: false` the output is sRGB values as before (PNG `sRGB` chunk, untagged PSD).
* `bitDepth`: `8` (default) \| `16`. PNG: 16-bit RGBA. PSD: depth-16 header, big-endian 16-bit channel and mask
  data, layer records in an `Lr16` block. SVG/PDF images are always 8-bit; requesting 16 adds a
  `bit_depth_reduced` warning and the report says `bitDepth: 8`. Any other value is `InvalidArgument`.
* `ExportPreflight` gains `targetProfile` (name of the space the numbers will be in), `bitDepth` (what will
  actually be written), `renderingIntent` (`"relative-colorimetric"` or `null` when nothing is converted) and
  `gamutClippingPossible` (the document gamut is not contained in the target's).
* `ExportReport` gains `targetProfile`, `bitDepth`, `renderingIntent` and `outOfGamutPixels`: the number of
  pixels (alpha > 0) whose color was outside the target gamut and was clipped — counted on the flattened image
  for PNG/PSD (document pixels, before `rasterScale`) and over all embedded raster images for SVG/PDF. A pixel
  counts when a destination linear-light channel leaves [0, 1] by more than 1e-4.
* New warning kinds: `color_converted`, `gamut_clipped` (`count` = pixels or fill colors), `vector_colors_srgb`
  (`count` = paths), `bit_depth_reduced`.

Color accuracy is a measured property of these conversions (see docs/COMPATIBILITY.md §2), not a promise that
other applications or displays will show identical colors.

### Raster-edit and canvas tools

All of these are atomic, record exactly one undo step, change nothing on failure and reject unknown fields.
Layer policy for the `edit.*` commands: the active layer must be a raster layer (`UnsupportedLayerFeature` for
vector layers and groups, `LayerLocked` when it or an ancestor is locked); the layer becomes
`vectorFidelity: raster`. `invalidated` is the tight rectangle of the pixels that changed.

* **`edit.gradient`** `{"cmd":"edit.gradient","x0":0,"y0":0,"x1":200,"y1":0,"kind":"linear","stops":[{"offset":0,"color":"#FF0000"},{"offset":1,"color":"#0000FF80"}],"opacity":1.0}`.
  `kind`: `linear` (default; `t` = projection onto the axis p0→p1) or `radial` (`t` = distance from p0 divided
  by |p1 − p0|); `t` is evaluated at pixel centres and clamped to [0, 1] (the end colors pad). Without `stops`
  the gradient runs from the foreground color (`color.set`) to the same color with alpha 0. Stops are sorted
  by `offset`; two stops at one offset make a hard edge. Colors are interpolated on **premultiplied values in
  the document blend space** (gamma-encoded by default; linear light for `blendSpace: linear` documents) — the
  same numbers the compositor blends — then composited source-over through the selection coverage ×
  `opacity` (source-atop under alpha lock). Errors (`InvalidArgument`): p0 == p1, non-finite or > 1e9
  coordinates, fewer than 2 or more than 256 stops, offset outside [0, 1], bad color, `opacity` outside [0, 1].
* **`selection.wand`** `{"cmd":"selection.wand","x":120.5,"y":80,"tolerance":0.1,"contiguous":true,"sampleMerged":false,"op":"replace"}`
  → `{"empty":false}`. Tolerance metric, contiguity and the 1 px soft edge are those of `edit.fill`. The
  reference is the active raster or vector layer (locked layers are fine: nothing is painted) or the composite
  with `sampleMerged`. Errors: point outside the canvas, tolerance outside [0, 1] (`InvalidArgument`); active
  layer missing/group without `sampleMerged` (`InvalidState` / `UnsupportedLayerFeature`).
* **`edit.adjust`** acts on straight (un-premultiplied), transfer-encoded values; alpha is never changed; with
  a selection the result is blended by its coverage.
  * `{"cmd":"edit.adjust","kind":"hsb","hue":30,"saturation":-0.2,"brightness":0.1}` — `hue` −180..180°,
    `saturation`/`brightness` −1..1 (all default 0). HSV model: `S' = clamp(S·(1+saturation))`,
    `V' = V + (1−V)·b` for b ≥ 0, `V·(1+b)` for b < 0.
  * `{"cmd":"edit.adjust","kind":"brightness_contrast","brightness":0.1,"contrast":0.25}` — both −1..1:
    `v' = (v − 0.5)·k + 0.5 + brightness`, `k = (1+contrast)/(1−contrast)`.
  * `{"cmd":"edit.adjust","kind":"curves","rgb":[[0,0],[0.5,0.6],[1,1]],"r":[[0,0],[1,0.9]]}` — each curve has
    2..=64 control points `[input, output]` in 0..1 with distinct inputs (any order); monotone cubic
    (Fritsch–Carlson) interpolation, constant outside the first/last point; `rgb` is applied first, then the
    channel curve. At least one of `rgb`, `r`, `g`, `b` is required.
  * `{"cmd":"edit.adjust","kind":"invert"}` — exact (`c' = a − c` on premultiplied data); twice = identity.
  * Errors (`InvalidArgument`): missing/unknown `kind`, out-of-range values, a field that belongs to another
    kind (e.g. `hue` with `invert`), malformed curves.
* **`edit.transform_quad`** `{"cmd":"edit.transform_quad","quad":[[40,10],[80,10],[110,110],[10,110]]}` →
  `{"sourceBounds":[x0,y0,x1,y1]}`. The source rectangle is the selection's bounds (the rectangle
  `selection.outline.bounds` reports) or, without a selection, the layer's content bounds (`layer.bounds`); its
  corners map to the quad corners in the order top-left, top-right, bottom-right, bottom-left. With a selection
  only the selected pixels move and the selection is warped with them. Resampling is the affine policy
  (inverse mapping, 1/256 px, bilinear on premultiplied data, no minification prefilter); output is clipped
  to the canvas. The quad must be strictly convex (mirrored is fine): degenerate, concave or self-intersecting
  quads, non-finite or > 1e7 coordinates → `InvalidArgument`; nothing to transform → `InvalidState`.
* **`canvas.crop`** `{"cmd":"canvas.crop","x0":16,"y0":8,"x1":80,"y1":72}` →
  `{"width":64,"height":64,"offsetX":-16,"offsetY":-8}` (new = old + offset). Corners may be in any order and
  are rounded to whole pixels; the rectangle may extend beyond the canvas (new area is transparent) but must
  intersect it. Every layer (also locked/hidden), layer mask, the selection and every stroke record (samples,
  centerline, outline) move together; pixels outside the new canvas are discarded, stroke outlines are kept
  whole (their off-canvas part is simply not visible in SVG/PDF). Vector layers stay `exact`. Errors: empty
  after rounding, no intersection, equal to the canvas, non-finite (`InvalidArgument`); size beyond the canvas
  limits — 16 384 px per edge, 8192² px total — (`AllocationLimit`).
* **`canvas.resize`** `{"cmd":"canvas.resize","width":2048,"height":1536,"resample":true}` or
  `{"cmd":"canvas.resize","width":300,"height":200,"anchor":"top_left"}` →
  `{"width","height","resampled","scaleX","scaleY","offsetX","offsetY"}`. `resample: false` (default) changes
  the extent around `anchor` (an odd size difference puts the extra pixel at the right/bottom for `center`);
  it is a crop/extend. `resample: true` scales everything: pixels and masks with an area-average filter when
  shrinking and bilinear when enlarging (premultiplied, the PNG `rasterScale` policy), stroke geometry and
  samples exactly — so vector outlines agree with the resampled pixels to within the resampling blur; DPI is
  unchanged. Errors: zero/unchanged size, `anchor` together with `resample` (`InvalidArgument`); limits
  (`AllocationLimit`).
* After `canvas.crop` / `canvas.resize` — and after `history.undo` / `history.redo` of such a step — the
  receipt has `viewChanged: true` (the view is re-fitted as on open) and `invalidated` covers the larger of the
  old and new canvas; re-read `document.info` for the size.
* **`selection.outline`** `{"cmd":"selection.outline","maxPoints":4000}` →
  `{"empty":false,"bounds":[10,20,100,60],"contours":[[[10,20],[100,20],[100,60],[10,60]]],"truncated":false,"pointCount":4}`;
  no selection → `{"empty":true,"bounds":null,"contours":[],"truncated":false,"pointCount":0}`. Contours are
  closed polylines (first point not repeated) along pixel edges of the mask at ≥ 50 % coverage, in document
  px; outer boundaries run clockwise, holes counter-clockwise. `bounds` is the tight rectangle of any nonzero
  coverage (clipped to the canvas) — the rectangle `edit.transform_quad` maps from. If the exact trace has more
  than `maxPoints` points it is simplified (Douglas–Peucker, then smallest contours dropped) and
  `truncated: true`; very noisy masks (> 2²⁰ boundary edges) are traced at reduced resolution.
* **`layer.bounds`** `{"cmd":"layer.bounds"}` or `{"cmd":"layer.bounds","id":"…"}` →
  `{"layerId":"…","bounds":[x0,y0,x1,y1]}` or `"bounds":null` when empty: the tight box of non-transparent
  pixels (strokes are rendered into the layer's pixels, so they are covered); for a group, the union of its
  children. Errors: unknown id (`NotFound`), malformed id (`InvalidArgument`), no active layer (`InvalidState`).

Hot path (not JSON): `nativeInputBatch` with packed pointer events (CONTRACTS §8) → Rust input
arbitration → live stroke; returns `[flags, nRects, x0,y0,x1,y1,…]`, flags bit0 dirty, bit1 view,
bit2 stroke committed, bit3 history changed; `[-1, 0]` for a malformed batch (no state change).
`nativeGetView` affine is `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (Android Matrix
`[a, c, e, b, d, f, 0, 0, 1]`).
