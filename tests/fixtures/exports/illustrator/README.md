# Illustrator-handoff sample exports (SVG / PDF)

Small sample files written by Veskra's "Export for Illustrator" writers (`crates/veskra-io/src/svg.rs`,
`pdf.rs`, plan in `plan.rs`). They are **samples for manual interoperability testing**
(`scripts/interop/illustrator_manual_checklist.md`), not goldens: no test compares against them byte for
byte, because they legitimately change when the compositor or outline code changes.

**None of these files has been opened in Adobe Illustrator. Illustrator status: NOT TESTED (R-14).**
They are not native `.ai` files and must never be renamed to `.ai`.

## Provenance

* Generator: `scripts/interop/svg_pdf_oracles.sh` with `VESKRA_COPY_SAMPLES=1`, which runs
  `cargo test -p veskra-io --test svg_pdf_interop` (fixture documents built in
  `crates/veskra-io/tests/svg_fixtures/mod.rs` and `svg_pdf_interop.rs::fixtures()`) and copies the
  files from `target/interop/illustrator/`.
* Generated 2026-10-09 from the FMT-B working tree on top of `402193d` (sources identical to checkpoint
  `c10b7f2`; toolchain 1.97.0). Content is synthetic (procedural strokes/textures); no third-party artwork.
* Output is deterministic for a given source tree (`deterministic_bytes` test).

| File | Fixture / options / strategy | What to look for |
|---|---|---|
| `hybrid-both-default.svg` / `.pdf` | paper fill + color layer (ink + texture) + ink layer with a 50 % stroke; both checkboxes on; `RasterVisibleVectorsHidden` (default) | One visible "Original appearance" image; hidden "Editable vectors" group (3 paths, PDF OCG `/OFF`). No doubled 50 % stroke. |
| `hybrid-both-residual.svg` / `.pdf` | same, `VectorsVisibleRasterResidual` | Visible vectors for the exact "Ink lines" layer (2 paths), raster for the rest; color layer's ink path hidden. |
| `ink_vector_only-vector-only.svg` / `.pdf` | 3 pressure-tapered ink strokes; only "Editable vector strokes" on | 3 filled outline paths, no images. |
| `blend_multiply-both-residual.pdf` | filled base + Multiply ink layer; residual | Multiply vector over raster (`/BM /Multiply` ExtGState in a transparency-group form). |

SHA-256 at generation time:

```
58f5798c83c652c99de18cb1ed104b4ae6d328d460d50913e1d0126ad33251ad  blend_multiply-both-residual.pdf
caa4bdc570534b1d20e4b42fc41deb9ca3570c89eaf2fb05c622070caebbeb08  hybrid-both-default.pdf
372c92c95e25d5f05093f94480c92cadcff1ec9e69366c2b6353b12d87541027  hybrid-both-default.svg
2eb420c76da3d222c7c7ed33ac5fdcc38876e77c5d73db027e32bb0509c147a6  hybrid-both-residual.pdf
f5ea6acbcc4880fa9a3e7e65d471d0f8957e720a54fa9bceedf86a0ad5c370d4  hybrid-both-residual.svg
10be6c347eb164de9f814ab2413ca4fd0eed34226c78755ded6cf94c26e0ecf0  ink_vector_only-vector-only.pdf
05f9944d05e1143d8cc4baebcdd5829f6338242433bc55adf7a082d90c68856d  ink_vector_only-vector-only.svg
```

Total size ≈ 95 KB (limit for this directory: 300 KB).
