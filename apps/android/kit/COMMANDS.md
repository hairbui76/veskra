# Engine command vocabulary (PROPOSAL from AND for `nativeExecute`)

**Status:** Proposal, not accepted. The Rust engine (`veskra-engine` / `veskra-ffi`) implements
these; ORCH reconciles names and payloads and records the result in `docs/CONTRACTS.md`. The Kotlin
builders live in `kit/src/main/kotlin/org/veskra/kit/Commands.kt` (single source on the Kotlin side;
`EngineSessionTest` pins the exact JSON).

Transport (CONTRACTS §8): `nativeExecute(h, json) -> String`.

* Request: one JSON object, `"cmd"` = dotted lowercase name (ARCHITECTURE §C `Layer.Add` → `layer.add`).
* Success: `{"ok": <payload>}`. Commands without a result return `{"ok":{}}`.
* Failure: `{"error":{"code":"<ErrorCode>","message":"<safe text>","recoverable":true|false}}`, `code`
  = `veskra_core::ErrorCode` serialized name (e.g. `InvalidArgument`, `NothingToUndo`). A failed
  command is atomic (no partial mutation). Messages never contain artwork bytes or full paths.
* Optional on any mutating command: `ok.dirty = [[x0,y0,x1,y1], …]` — document-space half-open rects
  whose pixels changed. When absent, the app redraws the whole document.
* Paths are app-private absolute file paths. The engine never sees SAF `content://` URIs.

## Document

| cmd | request fields | ok payload |
|---|---|---|
| `document.create` | `width`, `height` (px, ≤16384 edge, ≤8192² total), `dpi` (30–2400), `background` (`"#RRGGBB"` or `null` = transparent) | `{}` |
| `document.open` | `path` | `{}` (then use `document.info`) |
| `document.save` | `path` — writes a complete `.veskra` of the current committed state | `{"version":N,"bytes":B}` |
| `document.validate` | `path` — re-read manifest/checksums/decode WITHOUT replacing the open document | `{"valid":true,"version":N}` or error `CorruptData`/`UnsupportedSchema` |
| `document.autosave` | `dir` — write/refresh the recovery checkpoint (committed strokes only, never predicted) | `{"version":N}` |
| `document.recover` | `dir` — load the checkpoint written by `document.autosave` | `{}` |
| `document.info` | — | `{"width","height","dpi","version","profile","activeLayer","canUndo","canRedo"}` |

`version` is a monotonic u64 incremented on every committed history change (stroke commit, layer
command, undo, redo). The app derives dirty state from it (SAV-002).

## Layers

| cmd | request fields | ok payload |
|---|---|---|
| `layer.add` | `kind` (`raster`\|`vector`\|`group`), `name` — inserted above the active layer and made active | `{"id":"…"}` |
| `layer.remove` | `id` | `{}` |
| `layer.select` | `id` | `{}` |
| `layer.duplicate` | `id` | `{"id":"…"}` |
| `layer.rename` | `id`, `name` (≤255 UTF-8 bytes) | `{}` |
| `layer.move` | `id`, `index` (0 = bottom-most within its parent) | `{}` |
| `layer.set` | `id` + any of `visible`, `opacity` (0–1), `blend` (`normal`\|`multiply`\|`screen`\|`overlay`), `alphaLock`, `clip`, `locked` — one atomic undo step | `{}` |
| `layer.list` | — | `{"layers":[{"id","name","kind","visible","opacity","blend","alphaLock","clip","locked","depth","vectorEligible"}, …]}` top-most first |

## History, brush, color, view, input

| cmd | request fields | ok payload |
|---|---|---|
| `history.undo` / `history.redo` | — | `{}` or error `NothingToUndo`/`NothingToRedo` |
| `brush.select` | `preset`: `pencil.rough`, `pencil.smooth`, `pen.technical`, `pen.brush`, `marker`, `round.hard`, `round.soft`, `charcoal`, `eraser.hard`, `eraser.soft` | `{}` |
| `brush.set` | any of `size` (doc px > 0), `opacity`, `flow`, `stabilization` (0–1) | `{}` |
| `color.set` | `hex` (`"#RRGGBB"`, sRGB) | `{}` |
| `view.fit` | — fit document in the viewport | `{}` |
| `view.mirror` | `enabled` | `{}` |
| `input.reset` | — lifecycle pause: `InputArbiter::reset` commit/cancel policy for in-flight strokes | `{}` |

## Export (MASTER_PLAN §2.3, `veskra_core::export`)

| cmd | request fields | ok payload |
|---|---|---|
| `export.preflight` | `format` (`png`\|`psd`\|`svg`\|`pdf`), `options` = `ExportOptions` JSON (`includeVectorPaths`, `includeRasterAppearance`, `embedColorProfile`, optional `rasterScale`) | `ExportPreflight` JSON (camelCase) |
| `export.run` | `format`, `path`, `options` | `ExportReport` JSON (camelCase) |

There is no `ai` format. The app reports success only if `success && validation ==
"structurally_validated"` and the SAF copy closed without error.

## Example

```json
{"cmd":"document.create","width":2048,"height":2048,"dpi":300,"background":"#FFFFFF"}
{"cmd":"layer.add","kind":"raster","name":"Ink"}
{"cmd":"brush.select","preset":"pencil.rough"}
{"cmd":"color.set","hex":"#1A2B3C"}
{"cmd":"export.preflight","format":"svg","options":{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true}}
{"cmd":"export.run","format":"svg","path":"/data/user/0/org.veskra.dev/cache/export-staging/export-1.svg","options":{"includeVectorPaths":true,"includeRasterAppearance":true,"embedColorProfile":true}}
{"cmd":"document.save","path":"/data/user/0/org.veskra.dev/cache/save-staging/save-1-v7.veskra.tmp"}
{"cmd":"history.undo"}
```

## Open contract questions for ORCH

1. **Dirty rects of `nativeInputBatch`:** proposed half-open `[x0,x1)×[y0,y1)` in DOCUMENT pixels.
   Please confirm (vs. inclusive, or view space).
2. **Affine convention of `nativeGetView`:** proposed `x' = a·x + c·y + e`, `y' = b·x + d·y + f`
   (SVG/kurbo order). The app maps it to `android.graphics.Matrix` as `[a,c,e,b,d,f,0,0,1]`.
3. **`nativeSetViewport` parameter names:** CONTRACTS §8 lists `(h: Long, w: Int, h: Int)`; Kotlin
   uses `(h, w, hgt)`. JNI signature is unaffected — please fix the table.
4. **Null returns:** may `nativeExecute`/`nativeInputBatch` return NULL after a pending Java
   exception? Kotlin treats NULL as an `Internal` error / full redraw.
5. **Long-running export/save:** `export.run` and `document.save` currently block the engine thread
   (input stalls during export). Proposal: `export.start` → job id, `export.progress`, `export.cancel`
   (ARCHITECTURE §C `Export.Start/Progress/Cancel/Result`) operating on an immutable snapshot.
6. **Saved version:** `document.save` should return the exact `version` it encoded; the app currently
   records the version observed when the save started (conservative: may show "unsaved" spuriously,
   never "saved" wrongly).
7. **Two/three-finger tap undo/redo:** assumed handled inside `nativeInputBatch` (InputArbiter
   `TwoFingerTap`/`ThreeFingerTap`) and reported via return flag bit3 "history changed".
