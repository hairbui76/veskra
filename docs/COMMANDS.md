# Engine command reference (normative)

Source of truth: `crates/veskra-engine/src/command.rs`. The Android `:kit` module's `Commands`
builders are contract-tested against the real engine through JNI
(`CommandVocabularyIntegrationTest`). Changes require ORCH approval (AGENTS.md §8).

**Envelope.** Request: one JSON object with `"cmd"` and camelCase fields. Response:
`{"ok": <result>, "version": N, "invalidated": [[x0,y0,x1,y1],…], "warnings": […], "historyChanged": bool, "viewChanged": bool}`
or `{"error": {"code": "<ErrorCode>", "key": "<snake_case>", "message": "…", "recoverable": bool}}`.
Rectangles are half-open document pixels. Mutating commands are atomic and create exactly one undo
step; failures change nothing.

| Command | Fields | Result / notes |
|---|---|---|
| `document.create` | `width`, `height`, `dpi`=300, `background`?: `#RRGGBB[AA]` | info; creates "Background" + "Layer 1" (active), sRGB, clears history |
| `document.info` | — | id, size, dpi, profile, blendSpace, version, savedVersion, dirty, counts, activeLayer, undo/redo state, historyBytes/Trimmed |
| `document.save` | `path` | `{bytes, savedVersion, sha256}`; atomic temp→validate→rename (ADR-009) |
| `document.open` | `path` | info + `warnings` (forward-compat) |
| `document.validate` | `path` | fully decodes a `.veskra` without opening it (staged SAF save check) |
| `document.autosave` | `dir` | checkpoint `{path, seq, bytes}`; keeps newest 3 valid |
| `document.recover` | `dir` | opens newest valid checkpoint; never marked saved |
| `recovery.list` | `dir` | `{checkpoints: [{path, documentId, seq, bytes, valid, error}]}` |
| `recovery.open` | `path` | opens one checkpoint; never marked saved |
| `layer.add` | `kind`: raster\|vector\|group, `name`?, `placement`? | `{layerId}`; non-group becomes active |
| `layer.remove` / `layer.duplicate` / `layer.clear` | `id` | — / `{layerId}` / — |
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
