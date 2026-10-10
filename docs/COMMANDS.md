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
| `stroke.begin` / `stroke.samples` / `stroke.commit` / `stroke.cancel` | `eraser`?, `tool`?: brush\|eraser\|smudge / `samples: [{x,y,t,p?,tilt?,azimuth?}]` | headless stroke path; omitted `p` = no hardware pressure (fallback 0.5, flag cleared); commit → `{committed, strokeId, strokeIds, skippedReplicas}` (see "Stroke-time tools") |
| `history.undo` / `history.redo` | — | refused while a stroke is live |
| `selection.rect` / `selection.ellipse` | `x0,y0,x1,y1`, `op`?: replace\|add\|subtract\|intersect | |
| `selection.lasso` | `points: [[x,y],…]`, `op`? | nonzero winding |
| `selection.all` / `selection.clear` / `selection.invert` | — | |
| `edit.fill` | `x`, `y`, `tolerance`=0.1, `contiguous`=true, `sampleMerged`=false | raster layers only |
| `edit.fill_selection` / `edit.delete_selection` | — | raster layers only |
| `edit.transform` | `matrix: [a,b,c,d,e,f]` | selection content or whole layer; raster only |
| `view.size` / `view.fit` / `view.set` / `view.mirror` | `width,height` / — / `zoom?,rotation?,panX?,panY?,mirror?` / `enabled` | `{zoom, rotation, panX, panY, mirror, matrix}` |
| `input.config` | `fingerDraws`?, `stylusButton`?: none\|eyedropper\|toggle_eraser\|undo | |
| `input.reset` | — | lifecycle pause: commits confirmed samples of a live stroke |
| `export.preflight` | `format`: png\|psd\|svg\|pdf, `options`? | `ExportPreflight` (counts, warnings, blockedReason) |
| `export.run` | `format`, `path`, `options`? | `ExportReport`; extension must match; `.ai` rejected (ADR-008) |

`placement`: `{"at":"aboveActive"}` (default) \| `{"at":"top"}` \| `{"at":"above","id":…}` \|
`{"at":"below","id":…}` \| `{"at":"into","group":…,"index":n?}`.

`options` (`ExportOptions`): `{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true,"rasterScale":1.0}`.

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
