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
| `brush.list` / `brush.select` / `brush.set` | — / `preset` / `size`?, `opacity`?, `flow`?, `stabilization`? | built-in + library brushes (see *Brush library*); `brush.set` changes the session's copy of the current brush only |
| `color.set` / `color.get` / `color.pick` | `hex` / — / `x`, `y`, `layer`? | sRGB hex; recent colors |
| `color.convert` | `hex: "#RRGGBB"` + `to: "cmyk"`, **or** `cmyk: [c,m,y,k]` (each 0..=1) + `to: "srgb"`; `cmykProfile`?: path \| null (bundled), `intent`?: relative_colorimetric (default) \| perceptual, `blackPointCompensation`?: true (default) | to cmyk: `{cmyk: [c,m,y,k] (0..1, 4 decimals), inGamut, profile}`; to srgb: `{hex, clipped, profile}`. Read-only, no document needed, no undo step; see *CMYK* below |
| `profile.inspect` | `path`?: path \| null (bundled CMYK profile) | `{description, copyright, class, colorSpace, pcs, version, intents, bytes, bundled, usableForCmykExport, reason}`; read-only; see *CMYK* below |
| `stroke.begin` / `stroke.samples` / `stroke.commit` / `stroke.cancel` | `eraser`?, `tool`?: brush\|eraser\|smudge / `samples: [{x,y,t,p?,tilt?,azimuth?}]` | headless stroke path; omitted `p` = no hardware pressure (fallback 0.5, flag cleared); commit → `{committed, strokeId, strokeIds, skippedReplicas}` (see "Stroke-time tools") |
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

`options` (`ExportOptions`): `{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"rasterScale":1.0,"profile":"document","bitDepth":8,"cmykProfile":null,"renderingIntent":"relative_colorimetric","blackPointCompensation":true}`.
Every field after the three checkboxes is optional; JSON without `profile`/`bitDepth`/`cmykProfile`/`renderingIntent`/
`blackPointCompensation` behaves exactly as before (default-option files are byte-identical, pinned by
`crates/veskra-io/tests/export_default_pin.rs`). Preflight echoes `options` with every field present.

### Export color options

* `profile`: `"document"` (default) \| `"srgb"` \| `"display_p3"` \| `"adobe_rgb"` \| `"cmyk"` (PSD and PDF only, see
  *CMYK print export* below).
* `renderingIntent`: `"relative_colorimetric"` (default) \| `"perceptual"`. Applies to every conversion. RGB
  matrix/TRC profiles have no perceptual tables, so for RGB targets `perceptual` produces the same numbers as
  relative colorimetric (as LittleCMS does) and the report keeps saying `"relative-colorimetric"`.
* `blackPointCompensation`: `true` (default) \| `false`. Used by CMYK conversions. RGB conversions are unchanged by it
  (every RGB profile Veskra writes has a zero black point, where BPC is the identity).
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

### CMYK print export, `color.convert`, `profile.inspect` (ADR-018)

* `profile: "cmyk"` converts to CMYK with `cmykProfile` (absolute path of an ICC file of at most 32 MB) or, for
  `null`, the bundled `Coated_Fogra39L_VIGC_300.icc` (FOGRA39L, 300 % TAC, X-Rite/VIGC, redistributed unaltered).
  The file must be an ICC v2/v4 **output** (`prtr`) profile with data color space CMYK, a Lab or XYZ PCS and A2B/B2A
  LUT tables (`mft1`, `mft2`, `mAB`/`mBA`). Source space: the document's RGB profile (sRGB). Default intent relative
  colorimetric + black point compensation (Photoshop's default); `perceptual` uses the profile's perceptual tables.
* **PSD**: color mode 4, 8 or 16 bit; every layer and the merged image converted; channels transparency, C, M, Y, K
  (+ user mask), values **inverted** as Photoshop stores CMYK (255 / 65535 = no ink); merged image matted over paper
  (ink · alpha); image resource 1039 = the CMYK profile bytes (when `embedColorProfile`); resolution resource as for
  RGB. Veskra composites in RGB: layers with non-Normal blend modes get a `blend_mode_approximated` warning
  (Photoshop recomposites CMYK layers in CMYK); the merged image is Veskra's render.
* **PDF**: DeviceCMYK images (with the same soft masks) and `k` fill colors for editable vectors, DeviceCMYK
  transparency groups, and — when embedding — `/OutputIntents [<< /S /GTS_PDFX … /DestOutputProfile >>]` with the
  profile embedded once. Not claimed PDF/X conformant. "Editable vector strokes" / "Preserve original appearance"
  keep their meaning.
* **PNG / SVG** cannot carry CMYK: preflight `blockedReason` ("PNG cannot store CMYK. Export PSD or PDF …"),
  `export.run` → `IncompatibleOutputProfile`.
* `embedColorProfile: false` with CMYK is **allowed** (untagged CMYK, `profile_not_embedded` warning); untagged Display
  P3 / Adobe RGB stays refused.
* Bad `cmykProfile`: missing → `NotFound`; unreadable → `FilePermissionDenied` / `Io`; > 32 MB → `AllocationLimit`;
  not an output profile or not CMYK → `UnsupportedColorMode`; corrupt / missing LUT tables → `InvalidProfile`; empty
  string → `InvalidArgument`. Preflight reports the same message as `blockedReason`. Messages never contain the path.
* `ExportPreflight` / `ExportReport` for CMYK: `targetProfile` = profile description, `renderingIntent` =
  `"relative-colorimetric"` \| `"perceptual"`, `gamutClippingPossible: true`, `totalAreaCoverageMax` = max C+M+Y+K in
  percent over painted pixels (preflight: estimated from whole 64-px bands of the composite, at most 4 Mpx, every
  band when the document is ≤ 4 Mpx; report: exact over the flattened image / all PDF images, unmatted color, from the
  16-bit conversion), `cmykProfile: {description, bundled}`, `outOfGamutPixels` = pixels whose color does not survive
  RGB → CMYK → RGB within ΔE00 2 (PSD: merged image; PDF: all images; vector fills are counted in a warning).
  `totalAreaCoverageMax` and `cmykProfile` are `null` for RGB targets.
* `color.convert` (sRGB working space): `to: "cmyk"` → `inGamut` = the color survives RGB → CMYK → RGB within
  ΔE00 ≤ 2 (the black point mapping is undone exactly, so with BPC sRGB black is in gamut). `to: "srgb"` → `clipped` =
  the CMYK color lies outside sRGB (a linear channel beyond [0, 1] by more than 1e-4) and was clipped. Single colors use
  the full-precision pipeline (no device link). Transforms are cached per session by (path, intent, BPC) and the file's
  length + modification time (warm call ≈ 5 µs through the JSON boundary on the desktop test host).
* `profile.inspect`: never fails for a merely unsuitable profile (`usableForCmykExport: false`, `reason` says why);
  unreadable or structurally invalid files are errors (`NotFound`, `InvalidProfile`, …). `class` is the ICC device
  class signature (`prtr`, `mntr`, `scnr`, `spac`, `abst`, `link`, `nmcl`), `colorSpace` / `pcs` the unpadded
  signatures, `intents` the intents the profile has tables for (A2Bx/B2Ax; all three for matrix/TRC profiles),
  `version` `"major.minor"`.

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

## Stroke-time tools: symmetry, guides, tool (smudge), shapes

Source: `crates/veskra-engine/src/{symmetry,guide,shape}.rs`, `src/session/tools.rs`,
`crates/veskra-brush/src/smudge.rs`. Tests: `crates/veskra-engine/tests/tools_input.rs`.

Symmetry, guide and tool are **session (UI) state**: they are not written to `.veskra` files, not
part of undo history, do not change `version`, and survive `document.create/open` in the same
session (an explicit symmetry center is kept; the automatic one follows the new canvas). They are
reported by their `*.get` command and in `document.info` as `symmetry`, `guide` and `tool`. A
`*.set` replaces the whole state of that tool; a failed set changes nothing. A stroke that is
already in progress keeps the settings it began with. They apply identically to the headless
`stroke.*` commands and to the Android hot path: the UI sends the `*.set` command when the user
changes the setting and then just keeps sending input batches.

| Command | Fields | Result / notes |
|---|---|---|
| `symmetry.set` | `mode`: off\|vertical\|horizontal\|quad\|radial, `count`=6 (radial sectors, 2..=16), `mirror`=false (radial only: also mirror every sector), `cx`?, `cy`? (both or neither; default = canvas center) | `{mode, count, mirror, cx, cy, autoCenter, strokes}`; `cx`/`cy` are the center in use (`null` when automatic and no document is open); `strokes` = records one gesture produces (1, 2, 4, `count` or `2·count`, at most 32) |
| `symmetry.get` | — | same object |
| `guide.set` | `kind`: off\|line\|ellipse; line: `x0,y0,x1,y1` **or** `angle` (radians); ellipse: `x0,y0,x1,y1` (bounding box) | the guide as set: `{kind:"off"}` \| `{kind:"line",x0,y0,x1,y1}` \| `{kind:"line",angle}` \| `{kind:"ellipse",x0,y0,x1,y1}` |
| `guide.get` | — | same object |
| `tool.set` | `tool`?: brush\|eraser\|smudge, `strength`? (smudge, 0..=1, session default 0.5); omitted fields keep their value | `{tool, strength, smudgeMaxSize}` |
| `tool.get` | — | same object |
| `shape.line` | `x0,y0,x1,y1` | one undo step ("Shape"); `{committed, strokeId, strokeIds, skippedReplicas, filled, vector, samples}` |
| `shape.rect` / `shape.ellipse` | `x0,y0,x1,y1` (corners / bounding box, any order), `fill`=false | same |
| `shape.path` | `points: [[x,y],…]` (1..=10 000), `closed`=false, `smooth`=false, `fill`=false (closed paths only) | same |

**Symmetry.** `vertical` mirrors across the vertical axis through the center (left ↔ right),
`horizontal` across the horizontal axis (top ↔ bottom), `quad` across both (4 strokes), `radial`
rotates by `360°/count` (with `mirror`, each sector is mirrored as well). Every stroke — brush,
eraser, smudge, shape outline and shape fill — is replicated while it is drawn (confirmed samples
*and* the predicted tail) and committed as **one undo step**. For brush/eraser strokes each replica
is a real stroke record with its own transformed confirmed samples (position and azimuth are
transformed; pressure, tilt, time and flags are copied), so replicas of a vector-eligible brush
export as editable paths, are allowed on vector layers, and round-trip through save/open. Pixels
equal committing the replicas one after another in order (user's stroke first), so overlapping
replicas of a translucent brush build up exactly like separately drawn strokes.
`stroke.commit` returns `strokeIds` (one per record; `strokeId` is the first). A replica that lies
entirely off the canvas *and* beyond the renderer's vector reach (more than one canvas size
outside) of a vector-eligible paint brush changes no pixel and has no vector form: it is not
recorded and is counted in `skippedReplicas`. If such a replica is partly visible on a **vector**
layer the whole gesture is refused at commit (`InvalidArgument`, nothing changes), as for any
stroke without a vector form there.

**Guides.** Applied to every confirmed and predicted sample *before* symmetry; only the position
moves — pressure, tilt, azimuth, time and flags pass through unchanged. `line` with two points:
orthogonal projection onto the infinite line through them. `line` with `angle`: the line has
direction `(cos angle, sin angle)` in document space (y down, so positive is clockwise on screen)
and passes through the **first confirmed sample of each stroke**. `ellipse`: projection onto the
outline of the ellipse inscribed in the box, along the ray from its center through the sample
(exact nearest point for circles; a sample exactly at the center maps to the previous projected
point). While a guide is active the stroke is drawn with stabilization 0 (recorded in that
stroke's brush parameters), so it lies exactly on the guide. Guides do not apply to `shape.*`.

**Tool.** `brush` paints with the current preset (an eraser preset erases); `eraser` erases with
it (paint presets erase with `eraser.hard`); `smudge` is the smudge tool. Precedence at stroke
begin: `eraser: true` / stylus eraser end / `toggle_eraser` button → eraser; else
`stroke.begin {tool}`; else the session tool. The input path always uses the session tool.

**Smudge** (raster layers only): drags the layer's own pixels along the stroke; it never uses the
paint color. Diameter = `brush.set size` clamped to `smudgeMaxSize` (256 px), path stabilization =
`brush.set stabilization`, `strength` = how long picked-up color is carried (0 = no effect,
1 = the first pick-up is dragged the whole way). Hardware pressure scales the deposit when the
device reports it; without it the factor is exactly 1. Honors alpha lock (only color moves, alpha
is unchanged), the selection, the layer lock (`LayerLocked`) and the canvas bounds; on a vector
layer it is refused (`UnsupportedLayerFeature`). It is shown live while drawing, but predicted
samples are not previewed. Commit is one undo step labelled "Smudge": a raster edit (`smudge`),
not a stroke record — `{committed: true, tool: "smudge", strokeIds: []}`, and the layer becomes
`vectorFidelity: raster`. A smudge that changed no pixel (blank or flat area, strength 0) returns
`{committed: false}` and creates no undo step.

**Shapes.** Drawn with the **current brush preset, size, opacity and color** (never the smudge
tool; an eraser preset erases) as one committed stroke per symmetry replica, built from synthetic
samples along the outline. The samples carry **no hardware pressure** (`PRESSURE_REPORTED` clear,
no tilt/azimuth), so the line width is the brush's width at the documented fallback pressure 0.5.
Stabilization and guides are not applied (corners stay sharp, the record's brush parameters say
stabilization 0). Shapes honor the selection, alpha lock and layer lock exactly like strokes, and
with a vector-eligible brush they are editable paths on export (`vector: true`; allowed on vector
layers). `smooth` fits a uniform Catmull-Rom curve through the points (it passes through every
point). `fill: true` (rect, ellipse, closed path) first fills the interior with the foreground
color exactly like `edit.fill_selection` (nonzero rule, clipped by the selection, alpha lock
respected; brush opacity applies to the outline only), in the same undo step. A fill is raster content: it needs a raster
layer (`UnsupportedLayerFeature` on vector layers), marks the layer `vectorFidelity: raster`,
returns `vector: false` and a warning. `fill` with an open path or an eraser preset is
`InvalidArgument`. A zero-length line or single-point path is a dot.

**Limits and errors** (all typed, nothing changes on failure): coordinates of symmetry centers,
guides and shapes must be finite and within ±100 000 (`InvalidArgument`); `count` outside 2..=16,
`cx` without `cy`, a line guide with identical points or with both points and `angle`, an empty
ellipse box, a guide `angle` beyond ±1000 rad, `strength` outside 0..=1, an empty `points` list or
a closed path with fewer than three points (after dropping consecutive duplicates) are
`InvalidArgument`; more than 10 000 points or a smoothed path of more
than 200 000 samples is `AllocationLimit`. Work budget: path length × strokes of one gesture is
limited to 2 000 000 px for shapes, smudge strokes, ruled strokes and strokes with symmetry
replicas — a shape over it is refused, a `stroke.samples` batch over it is refused as a whole
(`AllocationLimit`; the stroke keeps what it had and can still be committed), and on the input
path the over-long part of the stroke is dropped with a warning. `shape.*` is refused while a
stroke is live (`InvalidState`).

**Invalidated rects.** A gesture that touches far-apart areas (symmetry) reports several disjoint
rects in `invalidated` / the input-batch result instead of their bounding box (overlapping rects
are merged; at most 48). Their union covers every changed display pixel (property-tested with all
tools active).

## Brush library (ADR-019)

Source: `crates/veskra-engine/src/{brushlib.rs,session/brushes.rs}`, `crates/veskra-brush`.
Tests: `crates/veskra-engine/tests/brush_library.rs`, `crates/veskra-brush/tests/{presets,brush_v2,interop_ip,legacy_regression}.rs`.

Library commands are **not document edits**: they never change the document, `version` or the
undo history. The app owns the storage directory and calls `brush.library.open` once per session;
every later change is persisted there (temp file + rename). Commands that need it fail with
`InvalidState` until then (`brush.list`, `brush.get`, `brush.select`, `brush.preview` work with the
built-ins without a library). Result `warnings` arrays are human-readable and may be shown as is.

| Command | Fields | Result / notes |
|---|---|---|
| `brush.library.open` | `dir` (absolute) | `{dir, brushes, assets, warnings}`. Creates `dir` (and `dir/assets`) if missing, loads `dir/*.vbrush` and `dir/assets/<hash>.png`; unreadable / invalid / duplicate / built-in-id files are skipped and named in `warnings`. Replaces a previously opened library |
| `brush.list` | — | `{presets: [{id, name, category, source, folder, vectorEligible, eraser, version, editable, current}], current}` — the 30 built-ins in display order, then library brushes sorted by `folder`, `name`, `id`. `editable` = not built-in. `current` (top level) = the session brush: `{id, name, size, opacity, flow, stabilization, vectorEligible, eraser, category, source, version}` |
| `brush.get` | `id` | `{preset, editable, assets: [{hash, width, height, available}]}`; `preset` is the full schema-3 JSON below |
| `brush.save` | `preset` (schema-3 object), `assets`? `{ "<key>": "/abs/file.png" \| {"path", "channel"?: auto\|alpha\|luminance\|darkness, "invert"?: bool} }` | `{id, version, vectorEligible, warnings}`. See *Saving* |
| `brush.asset.add` | `path` (absolute PNG), `channel`? = `auto`, `invert`? = false | `{hash, width, height, channel, downscaledFrom}` — converts the PNG to a coverage mask, stores it as `dir/assets/<hash>.png`; use `hash` as `tip.image` / `texture.image`. `channel` is the one actually used (`auto` resolved); `downscaledFrom` = `[w, h]` when the source was larger than 2048 px, else `null` |
| `brush.delete` | `id` | `{deleted: id}`; custom / imported only (`InvalidArgument` for a built-in, `NotFound` for an unknown id). A deleted *current* brush stays usable for the rest of the session |
| `brush.duplicate` | `id`, `name`? (default `"<name> copy"`) | `{id, name, vectorEligible}`; any brush (built-in included) → new `custom` brush, version 1 |
| `brush.import` | `path` (absolute) | `{imported: [{id, name, folder, approximated, unsupported}], failed: [{name, reason}], warnings}`. See *Import* |
| `brush.export` | `id`, `path` (must end in `.vbrush`) | `{path, bytes}`; any brush, built-ins included. Never writes `.prbr` |
| `brush.preview` | `id`, `width` 16..=1024, `height` 16..=256 | `{png, width, height, sizePx, synthetic: true}` — see *Preview* |

**Saving.** `brush.save` is strict: unknown preset fields are `InvalidArgument` (listing them).
`schema` defaults to 3. `id`: the id of an existing library brush updates it (`version` + 1); an
empty, unknown or **built-in** id creates a new brush (`custom.<12 hex>`, version 1; built-ins are
never overwritten, a warning says so). `version` and `vector_eligible` in the input are ignored:
the engine bumps the version and computes `vectorEligible` with the vector rule (returned in the
result). `source` is forced to `custom` (an edited imported brush becomes custom); `category`
defaults to `custom` when missing. Every `tip.image` / `texture.image` must be a known asset hash
(library, `brush.asset.add`, built-in) or a key of `assets`, whose files are converted and whose
keys are replaced by the resulting hashes (at most 8 per call); otherwise `NotFound`. Values out of
range are `InvalidArgument` (the preset is never silently clamped).

**Import.** The format is detected by content, not extension: a ZIP with `brush.json` is a
`.vbrush`, with `properties.json` an Infinite Painter `.prbr`, with `index.json` or `*.prbr`
entries a `.przp` pack; anything else is `InvalidArgument`. `.vbrush` keeps its id when free (else
a new one). `.prbr` lands in folder `Imported`; a `.przp` uses the folder names of its
`index.json` (empty → the pack file name), listed brushes in index order, then `*.prbr` entries the
index does not list (warning). Every brush that fails (missing entry, unreadable properties,
nested archive, …) is listed in `failed` and the others are imported. Infinite Painter brushes get
`source: imported:infinite_painter`, `category: imported`; their `approximated` /
`unsupported` lists name every difference (table: `docs/interop/infinite-painter-brushes.md`).
Limits (typed errors, never a panic): file ≤ 512 MiB (`.przp`) / 64 MiB (`.vbrush`, `.prbr`);
ZIP entries must have safe relative names (path traversal = `CorruptData`), declared sizes are
enforced and DEFLATE ratios bounded (zip bombs = `AllocationLimit`); ≤ 500 brushes per pack
(`AllocationLimit`); `properties.json` / `index.json` ≤ 1 MiB; images ≤ 16 MiB encoded and
≤ 8192 px per side (> 2048 px is downscaled); archives inside a brush entry are never opened
(`nested brush archive (depth > 1)`); a library holds ≤ 2000 brushes.

**Preview.** Renders a fixed S-curve across the image with a **synthetic** pressure ramp
(0.15 → 1 → 0.15, flagged as not hardware-reported, never stored) and 8 ms sample spacing, in
the current color, brush size capped at 45 % of the height. Erasers erase a light-gray band.
`png` is base64 (RFC 4648) of an sRGB-tagged PNG holding **straight-alpha** RGBA8 (PNG has no
premultiplied mode; Android's `BitmapFactory` premultiplies on decode).

**`.vbrush`** (Veskra brush file): ZIP with `mimetype` = `application/x-veskra-brush` (STORE,
first), `brush.json` (schema-3 preset, pretty JSON) and `assets/<hash>.png` (STORE) for every image
the preset references, built-in images included, so the file is self-contained. `<hash>` is the
asset content hash: SHA-256 of `"VSKMASK1"` ‖ width (u32 LE) ‖ height (u32 LE) ‖ mask bytes; readers
decode the PNG (8-bit grayscale, value = coverage) and verify the hash.

**Documents.** A committed stroke records the asset hashes its brush uses and the document embeds
their PNGs (`.veskra` schema 2, ADR-019 / `veskra-format` docs), so strokes drawn with custom or
imported brushes re-render on a device without that brush (`veskra_engine::rerender_stroke`).
`document.open` warns when an embedded image is damaged (pixels are kept; re-rendering such a
stroke falls back to a round tip).

### Schema-3 preset JSON

Field names are snake_case (the preset schema, unlike command fields). Angles are degrees,
counter-clockwise on screen. Missing fields take the defaults in brackets.

| Field | Type / range [default] | Meaning |
|---|---|---|
| `schema` | 3 | older schemas (1, 2) load through migration; newer → `UnsupportedSchema` |
| `id` | `[a-z0-9._-]{1,64}` | stable id |
| `version` | u32 [1] | content revision (recorded in strokes) |
| `name` | ≤ 255 bytes, no control chars [""] | |
| `category` | `pencil\|ink\|marker\|paint\|airbrush\|texture\|effect\|eraser\|smudge\|imported\|custom` [custom] | unknown values load as `custom` |
| `source` | `builtin\|custom\|imported:<format>` [custom] | |
| `folder` | ≤ 255 bytes [""] | library grouping |
| `size_px` | (0, 1000] [10] | diameter at full dynamics |
| `opacity` | 0..1 [1] | stroke-level opacity |
| `flow` | (0, 1] [1] | per-dab alpha |
| `spacing` | 0.005..4 [0.1] | fraction of the current diameter |
| `tip.shape` | `round\|image` [round] | |
| `tip.hardness` | 0..1 [1] | round tips |
| `tip.image` | 64 lowercase hex \| null [null] | asset hash; required for `image`, must be null for `round` |
| `tip.angle` | −360..360 [0] | |
| `tip.roundness` | 0.05..1 [1] | height / width |
| `tip.rotation` | `fixed\|direction\|pen` [fixed] | `direction` adds the stroke direction (0° = moving right); `pen` subtracts the hardware azimuth (0° = pen up), only when reported |
| `jitter.size`, `.opacity`, `.flow` | 0..1 [0] | per dab: value × (1 − j·u) |
| `jitter.angle` | 0..1 [0] | per dab: ± j·180° |
| `jitter.scatter` | 0..4 [0] | per dab: ± j × diameter, perpendicular to the stroke |
| `jitter.hue`, `.saturation`, `.brightness` | 0..1 [0] | per dab color (hue ± j·180°, others ± j) |
| `jitter.stroke_hue`, `.stroke_saturation`, `.stroke_brightness` | 0..1 [0] | once per stroke |
| `texture` | object \| null [null] | |
| `texture.source` | `procedural\|image` | |
| `texture.image` | hash \| null | required for `image`, null for `procedural` |
| `texture.scale` | procedural 0.5..256 (feature px); image 0.05..64 (px per texel) | |
| `texture.depth` | 0..1 | alpha × (1 − depth · t) |
| `texture.invert` | bool [false] | |
| `texture.mode` | `fixed\|follow` [fixed] | document space / moves with the dab |
| `dynamics.<pressure\|tilt\|velocity>.<size\|opacity\|flow\|texture\|scatter>` | `{enabled [false], curve [[0,0],[1,1]], min [0], max [1]}` | factor = `min + (max − min) · curve(input)`; curve: 2..32 points `[x, y]`, x from 0 to 1 strictly increasing, y in 0..1 (may fall); min/max 0..4. A brand-new preset defaults to `pressure.size` enabled. Inputs: pressure (hardware, else the 0.5 fallback), tilt / (π/2) (skipped for samples without tilt), velocity = smoothed speed / (4 px/ms) |
| `wet_edges` | 0..1 [0] | stroke interior keeps 1 − 0.75·wet of its coverage, rim keeps all |
| `accumulation` | `max\|build_up` [max] | |
| `eraser` | bool [false] | destination-out brush |
| `vector_eligible` | bool | must satisfy the vector rule (round circular tip, hardness ≥ 0.5, flow 1, `max`, no texture / jitter / wet edges / velocity dynamics, opacity & flow dynamics constant 1, not eraser/airbrush/texture/smudge/effect category); `brush.save` computes it |
| `stabilization` | 0..1 [0] | |

Example (`brush.get {"id": "paint.round_bristle"}` → `preset`, dynamics shortened):

```json
{"schema": 3, "id": "paint.round_bristle", "version": 1, "name": "Round bristle", "category": "paint",
 "source": "builtin", "folder": "", "size_px": 30.0, "opacity": 1.0, "flow": 0.9, "spacing": 0.03,
 "tip": {"shape": "image", "hardness": 1.0, "image": "06adf5f8e0234b2d5685197756e399944744f8976d49944f99877c9d3344fc64",
         "angle": 0.0, "roundness": 1.0, "rotation": "direction"},
 "jitter": {"size": 0.05, "angle": 0.0, "opacity": 0.0, "flow": 0.0, "scatter": 0.0, "hue": 0.0, "saturation": 0.0,
            "brightness": 0.0, "stroke_hue": 0.01, "stroke_saturation": 0.0, "stroke_brightness": 0.04},
 "texture": null,
 "dynamics": {"pressure": {"size": {"enabled": true, "curve": [[0.0, 0.0], [1.0, 1.0]], "min": 0.4, "max": 1.0},
                           "flow": {"enabled": true, "curve": [[0.0, 0.0], [1.0, 1.0]], "min": 0.35, "max": 1.0},
                           "opacity": {"enabled": false, "curve": [[0.0, 0.0], [1.0, 1.0]], "min": 0.0, "max": 1.0},
                           "texture": {"enabled": false, "curve": [[0.0, 0.0], [1.0, 1.0]], "min": 0.0, "max": 1.0},
                           "scatter": {"enabled": false, "curve": [[0.0, 0.0], [1.0, 1.0]], "min": 0.0, "max": 1.0}},
              "tilt": {"…": "same five targets, all disabled"}, "velocity": {"…": "same five targets, all disabled"}},
 "wet_edges": 0.0, "accumulation": "max", "eraser": false, "vector_eligible": false, "stabilization": 0.0}
```

(The engine always emits all fields; JSON object key order is not significant.)

**Errors** (all typed, nothing changes on failure): `InvalidState` (no library open),
`InvalidArgument` (bad preset field / range / unknown field, not a brush file, wrong export
extension, preview size, built-in delete, nested archive), `NotFound` (unknown id or asset hash,
missing file), `CorruptData` (broken archive or PNG, hash mismatch, unsafe entry name),
`AllocationLimit` (size / count limits above), `UnsupportedSchema` (preset schema > 3),
`Io` / `FilePermissionDenied` / `StorageFull` (file system).
