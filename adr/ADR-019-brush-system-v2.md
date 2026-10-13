# ADR-019: Brush system v2 — schema-3 presets, brush assets, library, Infinite Painter import
Status: Accepted (provisional)
Date: 2026-10-12 (amendment 1: 2026-10-13, see the end)
Owner: BRH (with FMT for the container, ORCH for the command contract)
Related: ADR-004 (brush core), ADR-009 (native container), ADR-016 (core contracts), ADR-017 (tool set)
Amends: ADR-004 (adds image tips, textures, jitter, dynamics, wet edges), ADR-009 (container schema 2)

## Context
The owner wants many more brushes, user-made brushes, and import of community brushes from
Infinite Painter (`.prbr` single brush, `.przp` pack). The v1 engine (ADR-004) had ten round
presets with procedural grain. Strokes in existing documents must keep rendering identically, and
a stroke drawn with a custom or imported brush must re-render on a device that does not have that
brush.

## Decision
1. **Independent engine, extended.** `veskra-brush` stays Veskra's own dab engine. Preset schema 3
   adds: `category` / `source` / `folder`; a tip (`round` with hardness, or `image` = coverage-mask
   asset) with `angle` (degrees, counter-clockwise), `roundness` and `rotation`
   (`fixed | direction | pen`, pen = hardware azimuth only when reported); seeded per-dab jitter
   (size, angle, opacity, flow, scatter) and color jitter (per dab and per stroke); a texture
   (procedural grain or image, `fixed` in document space or `follow` with the dab, scale, depth,
   invert); a dynamics table (pressure / tilt / velocity × size / opacity / flow / texture /
   scatter, each with `enabled`, a shape-preserving curve and a `min..max` factor); `wet_edges`.
   Velocity comes from sample timestamps; tilt and azimuth are used only when the hardware
   reported them; pressure keeps the engine's documented fallback.
2. **Exact compatibility.** Migration 1 → 2 → 3 is exact for everything but the (never used)
   `tilt_size` factor. Presets that use only schema-2 features render through the unchanged
   schema-2 dab code: the ten old built-ins and the schema-2 JSON stored in existing stroke records
   re-render **byte-identically** (`crates/veskra-brush/tests/legacy_regression.rs`, golden
   produced from the tree before this change). v2 features use a separate rasterizer.
3. **Vector honesty.** `vector_eligible` additionally requires a circular round tip
   (`roundness = 1`), no texture, no jitter, no wet edges and no velocity dynamics. Pressure- and
   tilt-driven size stay eligible. Validation rejects dishonest claims; `brush.save` and imports
   compute the flag themselves.
4. **Assets.** An asset is an 8-bit coverage mask identified by
   `SHA-256("VSKMASK1" ‖ w ‖ h ‖ mask)` (independent of the PNG carrier), stored as a canonical
   8-bit grayscale PNG, ≤ 2048 × 2048 (larger sources up to 8192 px are box-downscaled while
   streaming; nothing larger is decoded), with a mip pyramid for rendering. Conversion: alpha,
   luminance, darkness (black = paint) or auto, optionally inverted. The 30 built-ins use eight
   assets generated procedurally in Rust with libm-free arithmetic, so their hashes are identical
   on every platform; no image comes from any app or pack.
5. **Built-ins.** 30 original presets: the ten unchanged ids plus pencils (HB, 2B, mechanical),
   inks (G-pen, fineliner, dry ink), markers (felt, alcohol/blendy), paint (round bristle, flat oil,
   gouache, palette knife), airbrush (soft, spray/speckle), textures (chalk, pastel, crayon,
   sponge), effect (glitter) and a textured eraser.
6. **Library.** The app owns a directory; the engine keeps `<dir>/<id>.vbrush` per user brush and
   `<dir>/assets/<hash>.png` for images added on their own. Commands (`docs/COMMANDS.md`, "Brush
   library") never touch the document or its undo history. `.vbrush` = ZIP with `mimetype`,
   `brush.json` (schema 3) and `assets/<hash>.png` for every referenced image (self-contained,
   built-in images included).
7. **Document embedding (container schema 2).** At commit, every asset a stroke's preset
   references is copied into the document; the stroke record lists the hashes. `.veskra` gains
   `brushes/<hash>.png` entries and `brushAssets` / `brush.assets` JSON fields. Schema 2 is a strict
   superset of 1, read natively; the writer emits it (with `minReader` 1) only when a document
   embeds assets, otherwise it writes the exact schema-1 bytes as before. A missing referenced
   asset is a warning (pixels are authoritative; re-render falls back to a round tip); a listed
   asset that is missing, oversized, not a PNG, or whose content does not match its hash is a typed
   error at load (or, for a hash mismatch detected by the engine, a warning plus fallback).
   `veskra_engine::rerender_stroke` re-renders a record from the document alone.
8. **Infinite Painter import** maps everything with a sensible equivalent and reports the rest
   per brush (`approximated`, `unsupported`); the full table with sources and certainty is
   `docs/interop/infinite-painter-brushes.md`. Mapped: name, size (calibrated 3.74 px per unit),
   opacity, flow, spacing, angle, stroke / pen rotation, softness, custom head and texture images
   (with head conversion format and profile), texture depth / scale / style / invert, size /
   angle / flow / scatter jitter, per-dab and per-stroke color jitter, pressure / tilt / velocity
   dynamics for size / flow / texture / scatter (y-inverted profiles), lazy stroke, wet edges.
   Not mapped (reported as unsupported): IP built-in heads and textures (not in the file; replaced
   by a round tip / procedural grain, never copied), paint mixing (`mix-in`, wet mix, dirty
   blending), dilution and head-depth dynamics, size / flow profiles along the stroke, blend modes,
   glaze, adhesion, screen-aligned heads, head depth / structure / unknown styles, texture deform /
   softness / structure / angle / stretch, initial-angle and texture jitter, colored heads and
   textures, and the special sections (bleed, particles, pixel, filters). **Imported brushes are
   approximations rendered by Veskra's engine, not reproductions.** Archives are handled by the
   bounded ZIP reader with per-format limits; data descriptors and prefixed archives are accepted
   for third-party archives only.

## Not done
* Paint mixing / smudge-while-painting brushes (IP blend properties): not implemented; the smudge
  *tool* (ADR-017) is unchanged.
* Exporting to `.prbr` is deliberately not offered.
* Wet edges use Veskra's own model (interior lighter than the rim, from a per-stroke depth
  buffer); it is not IP's or Photoshop's algorithm.

## Alternatives considered
1. Keep presets flat (schema 2 + more fields) — rejected: the dynamics table maps cleanly onto IP
   and onto the editor UI, and migration stays exact.
2. Identify assets by the PNG file hash — rejected: re-encoding would change ids; the mask hash is
   carrier-independent and verifiable.
3. Embed only preset ids in documents — rejected: documents must re-render without the library.

## Acceptance experiment and evidence
* Automated (run on the Windows host): legacy byte-identity, migration chain, vector rule,
  determinism / batching independence of all 30 built-ins and of image tips + image textures,
  library CRUD / export / import round trip, synthetic `.prbr` / `.przp` imports including hostile
  archives (path traversal, zip bomb, > 500 brushes, nested pack, huge JSON, truncated, hash
  mismatch), document embedding round trip with a fresh session and no library, preview format.
* Host performance (release, one Windows desktop CPU, rasterization only): 64-px round tip
  ≈ 76 000 dabs/s; 64-px image tip ≈ 21 000 dabs/s; 64-px image tip + image texture + wet edges +
  color/angle jitter ≈ 8 700 dabs/s (`crates/veskra-brush/tests/perf.rs`).

## Evidence still missing (why this is provisional)
* No physical stylus or tablet was used: pen rotation, tilt and velocity response are unreviewed;
  no device performance numbers.
* No real commercial `.przp` has been imported (the owner will supply one); no visual comparison
  with Infinite Painter exists; several mapping constants are single-reading calibrations.
* The Android brush editor and library UI are built separately (lead) against this contract.

## Review
Lead (brush system v2 brief, 2026-10-12). ORCH sign-off pending.

## Amendment 1 (2026-10-13): second wave of built-in brushes, tip preview
Context: the owner wants many more brushes to choose from, the APK must stay small, and the
picker shows each brush's tip image plus a stroke sample.

Decision:
1. **40 new original built-ins** (70 in total), listed with ids, names, categories and sizes in
   `docs/COMMANDS.md` "Built-in brushes": pencils (colored, charcoal pencil, graphite stick), ink
   (ballpoint), calligraphy (dip pen, brush marker, flat nib, sumi-e), marker (highlighter), paint
   (impasto, fan), watercolor (round, wet wash, granulating wash, dry brush, bleeding edge, salt),
   airbrush (grainy), texture (conté), nature (grass, foliage, falling leaves, flowers, twigs, fur,
   hair strands), effect (sparkles, bokeh, glow, light rays, fog, rain, snow, ink splatter) and
   pattern (halftone, stipple, cross-hatch, scales, honeycomb, clouds). They use only schema-3
   features the engine already rendered — no rasterizer change — so the 30 first-wave presets,
   their eight assets and every existing stroke render exactly as before (`legacy_regression`
   and the 30 first-wave goldens pass unchanged; re-blessing rewrote those 30 PNGs byte for byte).
   Display order groups families; the first wave keeps its relative order and the erasers stay
   last. Ids are prefixed by category and stable.
2. **Four categories** `watercolor`, `calligraphy`, `nature`, `pattern` (serde snake_case). A
   build that predates them loads them as `custom` (`#[serde(other)]`), as any unknown value.
3. **Vector rule**: `watercolor`, `nature` and `pattern` join the textural categories that are
   never vector-eligible; `calligraphy` is judged by the brush's features. Only
   `calligraphy.brush_marker` (circular hard tip, pressure-driven size, stroke opacity 0.88) is
   eligible among the new brushes.
4. **29 new procedural assets** (37 in total): 22 tips and 7 tileable textures, generated in
   `crates/veskra-brush/src/procedural/wave2.rs` with integer hashing and IEEE `+ − × ÷`, `sqrt`,
   `floor` only; angles use a fixed Taylor polynomial (`sin_cos`) instead of the platform libm.
   All 37 content hashes are pinned in `tests/fixtures/brushes/builtin_asset_hashes.txt`, so a
   platform whose arithmetic differed would fail `builtin_asset_hashes_are_pinned`. Nothing is
   copied or traced from any app or brush pack; no image files ship in the APK.
5. **`brush.tip_preview`** `{id, size? 16..=512 = 128, color? #RRGGBB = #111111}` →
   `{png, width, height}`: one dab of the tip at its default angle and roundness, scaled to fill
   the square, with a `follow` texture applied (fixed paper textures, flow, opacity, dynamics,
   jitter and wet edges are not shown); straight-alpha sRGB RGBA PNG, RGB = `color`. Rendered by
   `veskra_brush::render_tip` with the stroke dab rasterizer and encoded with the `png` crate's
   fast deflate (the export writer's adaptive filtering + strong compression cost ~1.5 ms per
   128-px tip, ~10× more). Contract: `docs/COMMANDS.md` "Tip preview".
6. `builtin_presets()` / `builtin_preset(id)` build the table once per process (identical
   values; a lookup no longer rebuilds all presets).

Evidence (Windows desktop host, release; run on 2026-10-13):
* Stripped `libveskra_ffi.so`, `aarch64-linux-android` (NDK 28.2 clang linker, `llvm-strip
  --strip-unneeded`): 12 124 456 B before, 12 246 840 B after (+122 384 B ≈ +119.5 KiB; target
  < +500 KB).
* `brush.tip_preview` for all 70 built-ins at 128 px: ≈ 15 ms in total on a quiet machine,
  ≈ 40–50 ms when the host was loaded (same code; was ≈ 115 ms quiet with the export PNG writer).
  70 `brush.preview` stroke samples at 320 × 80 for comparison: ≈ 265 ms quiet, ≈ 700 ms loaded.
* One-time built-in asset + preset construction (first `Session` of a process): ≈ 70 ms vs
  ≈ 28 ms before the change, measured interleaved under the same host load; ≈ 30 ms for the 37
  assets on a quiet machine (generation ≈ 13 ms, the rest is hashing, mip pyramids and the
  eager PNG carriers). The Android app creates sessions on its engine thread, not the UI thread.
* Tests: `crates/veskra-brush/tests/builtin_wave2.rs` (pinned hashes; every new brush
  deterministic and batching-independent with hardware pressure and with the no-pressure
  fallback; seed dependence where jittered; all built-in strokes mutually distinct by ≥ 6 alpha
  levels mean over the painted area; categories), 40 new goldens
  `tests/fixtures/goldens/brush_<id>.png`, `src/tip.rs` unit tests, `src/procedural/wave2.rs`
  (polynomial accuracy, seamless tiles), `crates/veskra-engine/tests/brush_library.rs`
  (`tip_preview_contract`, custom / imported / follow-texture cases).

Not done / caveats:
* The look of the new brushes was reviewed only on host-rendered contact sheets (mouse-like and
  synthetic-pressure strokes), not on a device with a stylus; tilt / velocity feel is unreviewed.
* PNG carriers of built-in assets are still encoded eagerly at startup; encoding them lazily
  (only when a `.vbrush` is exported or a stroke embeds them) would remove most of the extra
  startup time but touches `BrushAsset`'s equality and accessor contract, so it is left for a
  follow-up.
* A user brush saved before this amendment with one of the new built-in ids (only possible via
  an imported `.vbrush` that kept such an id) is now skipped at `brush.library.open` like any
  built-in-id file, with a warning.
