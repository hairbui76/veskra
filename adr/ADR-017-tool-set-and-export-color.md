# ADR-017: Painter tool set and export color targets
Status: Accepted (provisional)
Date: 2026-10-10
Owner: ORCH (repository owner decision)
Related tickets: DSN-01, DSN-03, DOC-03, COL-011, COL-012, NEXT-02
Amends: ADR-013 (export clause only), ADR-016 (additive contract changes)

## Context
The owner set the product target on 2026-10-10: a compact editor in the style of Infinite Painter,
with a chosen subset of its tools, and color handling on PSD / Illustrator export as the
differentiator. That needs new engine commands, two additive changes to frozen contracts, and a
decision ADR-013 had deferred: converting to an output color space other than the canvas's.

## Decision
1. **Tool set.** The engine command API gains the commands listed in `docs/COMMANDS.md` under
   "Stroke-time tools" (symmetry, guides, shapes, smudge, tool) and the raster-edit / canvas block
   (gradient, magic wand, adjust, quad transform, canvas crop / resize, selection outline, layer
   bounds, view lock). Symmetry, guide and tool are session state: not stored in `.veskra`, not in
   history. Every mutating command stays atomic and one undo step.
2. **Explicitly out of scope** for this round: pattern tools, kaleidoscope, perspective guides,
   hatching, liquify, warp, filters, straighten, panels, navigator, anchor transform, and selection
   by brush / polygon / path / color range.
3. **Text** is rasterized by the Android shell and imported as a raster layer. It is not editable
   text and exports as pixels; the UI says so.
4. **Export color targets.** `ExportOptions` gains `profile` (`document` default, `srgb`,
   `display_p3`, `adobe_rgb`) and `bitDepth` (8 default, 16 for PNG and PSD). Conversion is relative
   colorimetric with per-channel clipping; the target ICC profile is always embedded for the
   wide-gamut targets (the engine refuses to write them untagged), and preflight / report state the
   target, the intent and the number of clipped pixels. Default options produce byte-identical files
   to before (pinned by `crates/veskra-io/tests/export_default_pin.rs`).
5. **What ADR-013 still governs.** Authoring and display stay sRGB-only: there is no wide-gamut
   canvas, no P3 display path and no wide-gamut color picker. Only the *export* clause of ADR-013 is
   amended. A P3 or Adobe RGB export of an sRGB canvas carries the same colors in a larger
   container; it does not create colors the canvas could not hold.
6. **Contract changes (ADR-016).** `ExportOptions`, `ExportPreflight` and `ExportReport` gain
   fields with serde defaults; `veskra-brush` gains public layered-compositing and smudge API. All
   additive.

## Consequences
* The Kotlin `:kit` builders are contract-tested against the engine through JNI for every new
  command and for all target profiles at both bit depths.
* 16-bit and wide-gamut output roughly doubles file size and export memory for 16-bit.
* The "Adobe RGB (1998) compatible" profile is generated from published colorimetry; it is not
  Adobe's file and must keep the word "compatible" in the UI.

## Evidence still missing (why this is provisional)
* No file from this round has been opened in Photoshop or Illustrator. 16-bit PSD output has only
  been read back by Veskra's own structural reader.
* The LittleCMS and Pillow oracles were not run for 16-bit or wide-gamut output; the numeric
  evidence is the independent `png` decoder plus ΔE00 checks in `veskra-io` tests.
* Nothing was run on a physical tablet or with a stylus: smudge feel, symmetry latency and the
  two-finger navigation inside tool overlays are unmeasured.

QAP must attach that evidence before this ADR is cited as verified or the color claims appear in
release notes (AGENTS.md §3: no "no color shift" claims).

## Review
Owner 2026-10-10 (scope and export color options approved in the working session).
