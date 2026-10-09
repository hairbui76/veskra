# PSD / PNG export interoperability report (FMT-07..11, CLR-05)

Date: 2026-10-09 · Author: FMT-A · Status: **automated oracles run; Photoshop NOT TESTED**

**Photoshop: NOT TESTED (no access). PSD-002, the Adobe part of PSD-004, and COL-009 (ADOBE) are BLOCKED.**
Nothing in this report claims Photoshop compatibility.

## 1. What was tested

| Item | Implementation |
|---|---|
| PNG writer | `crates/veskra-io/src/png.rs` (`PngExporter`, `inspect_png`, `encode_rgba8_srgb`) |
| PSD writer | `crates/veskra-io/src/psd.rs` + `psd/packbits.rs` (independent, from Adobe's public spec) |
| PSD structural parser | `crates/veskra-io/src/psd/read.rs` (`inspect_psd`, used by `validate`) |
| Fixtures | `crates/veskra-io/tests/psd_fixtures/mod.rs` (closed-form synthetic art) |
| Oracles | `scripts/interop/png_check.py`, `scripts/interop/psd_check.py`, `scripts/interop/psd_krita_check.sh` |

Tool versions: psd-tools **1.24.0**, Pillow **12.3.0**, numpy **2.5.3**, ImageMagick **6.9.12-98 Q16**,
Krita **5.2.2** (Debian `1:5.2.2+dfsg-2build8`), Rust `png` crate **0.18.1** (dev-only).

## 2. Commands

```sh
export CARGO_TARGET_DIR=/tmp/claude-0/target-fmta
cargo test -p veskra-io                       # all tests incl. the oracle runs below
cargo test -p veskra-io --test png_export     # runs png_check.py (Pillow) on 8 PNGs
cargo test -p veskra-io --test psd_interop    # runs psd_check.py (psd-tools + ImageMagick) on 5 PSDs
bash scripts/interop/psd_krita_check.sh                         # Krita on the 5 fixture PSDs
bash scripts/interop/psd_krita_check.sh $CARGO_TARGET_DIR/interop/psd/diag   # Krita single-feature PSDs
cargo clippy -p veskra-io --all-targets --no-deps -- -D warnings
```
Outputs: `$CARGO_TARGET_DIR/interop/{png,psd,psd/diag}/` (`*.psd`, `*.png`, `*.composite.png`,
`*.expected.json`, `psd_check.results.json`, `png_check.results.json`, `*.krita.png`).

Krita note: on this host Krita's Qt ignores `QT_QPA_PLATFORM=offscreen` (aborts: "could not load
xcb"); the script therefore runs `xvfb-run -a krita --export --export-filename out.png in.psd`.

## 3. Format choices (documented)

* **PNG:** always RGBA8 straight, non-interlaced; composite in 64-row bands from `veskra-compose`, converted
  by `convert::premul16_to_straight8`. Embed on → `iCCP` (exact document ICC bytes, name = profile
  description, Latin-1 ≤ 79 B); no `sRGB` with `iCCP`; `gAMA`/`cHRM` sRGB fallback only for
  sRGB-equivalent profiles. Embed off + sRGB doc → `sRGB` (intent 1 = relative colorimetric) + `gAMA` + `cHRM`.
  Embed off + non-sRGB doc → pixels converted to sRGB (`RgbTransform`, relative colorimetric) + `sRGB`
  chunk + `ProfileNotEmbedded` warning. `pHYs` from DPI (scaled with `raster_scale`). `raster_scale`
  implemented: box filter (down), bilinear (up), premultiplied u16.
* **PSD:** v1, 8-bit RGB, ≤ 30 000 px/edge (else `PsdEncodeError`; PSB not written). Header channels = 4
  with a **negative layer count** (merged image's 4th channel = transparency); merged RGB is
  **matted over white** (Photoshop convention; psd-tools and ImageMagick un-matte it). Resources 1005 and
  1039 (when embedding). Records bottom→top; channels −1, 0, 1, 2 (+ −2 user mask); PackBits per row;
  `luni`, `lsct` (divider type 3 `</Layer group>`, folder 1/2, key `pass` for pass-through), `lspf`.
  Clipping follows `veskra-compose` (orphan clipper / group clip flag → written 0 + warning; pass-through
  group as clip base → warning). `raster_scale ≠ 1` is blocked for PSD.

## 4. Results — PNG (Pillow oracle, CLR-05 / COL-001 / COL-002)

8 PNGs (5 fixtures, 292-swatch sheet, sRGB-chunk variant, Display-P3 embedded variant):
**every file: ICC bytes == document profile (564 B sRGB / 552 B P3) or sRGB chunk intent 1 as expected;
pixel max |Δ| = 0 on all 4 channels vs Veskra composite.** Rust `png` crate decode: also 0 LSB
(COL-002: 256 grays + 216-color cube, opaque, 0 LSB with embed on and off). P3 not-embedded export equals
`RgbTransform`(P3→sRGB) of the composite exactly.

## 5. Results — PSD

### psd-tools 1.24.0 + ImageMagick (`psd_check.py`), all 5 fixtures

Layer tree (names incl. Unicode, order, groups, visibility, opacity, blend modes incl. `PASS_THROUGH`,
clipping) **matches on all 5**; ICC resource 1039 **== document bytes on all 5** (PSD-001, PSD-003).

Metrics vs Veskra composite (8-bit; `p` = premultiplied |Δ| max/mean, `a` = alpha max, `oc` = color max on opaque pixels):

| Fixture (size) | merged, psd-tools | merged, ImageMagick | psd-tools **recomposite** of layers |
|---|---|---|---|
| simple (11 KB) | a0 p0.937/0.052 oc0 | a0 p0.937 oc0 | a0 p0.875/0.008 oc0 |
| six_layer (85 KB) | a0 p0/0 oc0 | a0 p0 oc0 | a1 p1.933/0.342 oc1 |
| twelve_layer (523 KB) | a0 p0/0 oc0 | a0 p0 oc0 | a1 p1.961/0.369 oc1 |
| empty_and_inverse_mask (8 KB) | a0 p1.616/0.191 oc0 | a0 p1.616 oc0 | a1 p1.443/0.312 oc0 |
| large_sparse 2048×1536 (406 KB) | a0 p0.996/0.001 oc0 | a0 p0.996 oc0 | a1 p1.796/0.0001 oc1 |

Asserted thresholds (`psd_interop.rs`): merged — alpha 0, opaque color 0, premultiplied ≤ 2.0 (residual is
the readers' 8-bit un-matting `(m+a−255)·255/a`; the stored matted bytes are verified **bit-exact** against
the composite in `psd_export.rs`). Recomposite — simple ≤ 1.0/0/0, others premul ≤ 2.0, alpha ≤ 1,
opaque color ≤ 1. **All pass.** psd-tools' independent recomposite reproduces Multiply/Screen/Overlay,
clipping, user masks (default 0 and 255), group opacity, pass-through and hidden layers to ≤ 2 LSB: no
blend/clipping discrepancy was found with psd-tools (PSD-004 automated part, PSD-005).

### Krita 5.2.2 (`psd_krita_check.sh`), opened and rendered without error, size and alpha exact on all 5

| Fixture | alpha max | premul max / mean | opaque color max |
|---|---|---|---|
| simple | 0 | 0.0 / 0.0 | 0 |
| empty_and_inverse_mask | 0 | 0.965 / 0.010 | 0 |
| large_sparse | 0 | 0.965 / 0.00001 | 0 |
| six_layer | 0 | **122 / 8.95** (10 604 px > 2) | **122** |
| twelve_layer | 0 | **88 / 2.30** (48 598 px > 2) | **88** |

**Discrepancies (recorded, not hidden):** single-feature diagnostic PSDs (`write_krita_diagnostic_psds`)
show Krita 5.2.2 matches Veskra within 1 LSB for Multiply/Screen/Overlay layers and pass-through groups,
but **(1) ignores PSD clipping** (diag_clip_root/in_group/multiply: max 138/138/88) and **(2) ignores group
opacity** (diag_group_normal_085: 24, diag_group_screen_085: 25; at 100 % opacity: 1). Krita's render
equals psd-tools' render of the same file with clipping off and group opacity 255 (max Δ ≤ 2), and Krita's
own `.kra` conversion of our PSD shows the clipped layer without inherit-alpha and the group at opacity 255.
Control: Krita's **own** PSD of an ORA with an 85 % group (Krita writes 216 on both group records) also
loses the opacity on re-import (Δ 24). Conclusion: Krita 5.2.2 PSD-import limitations, not an encoding
error (psd-tools reads the same fields correctly). Users opening clipped layers or group opacity in Krita
will see a different render; Photoshop behaviour is unverified.

### Rust tests (structure, exactness, PSD-006/007)

`psd_export.rs` (15): tree/flags/resources; every layer channel == `premul16_to_straight8` of the layer
pixels (0 LSB), masks exact; merged == composite matted over white (0 LSB); empty layer zero rect; empty
group; inverse mask; vector layer → raster + `LayerRasterized`; locks → `lspf`; P3 embedded/converted;
`ClippingApproximated` ×3 and `BlendSpaceMismatch` (PSD-006); preflight warnings == report warnings;
deterministic bytes; validator rejects truncation/signature/trailing byte/RLE count corruption;
**PSD-007**: cancel during writing → `Cancelled`; `export_to_path` with cancelled token → no file, no
temp leftovers; thread-cancel race consistent; success only after validation; committed reference PSDs
byte-identical. `png_export.rs` (11), unit tests in png/psd/packbits (10, incl. Apple TN1023 PackBits vector).

## 6. Limitations / not done

* Photoshop (PSD-002, PSD-004 ADOBE, COL-009): **BLOCKED**, needs a human with Photoshop.
* PSD 16-bit, PSB, thumbnails (1036), group masks (no schema), layer effects: not written.
* Off-canvas layer pixels are preserved in PSD layers (Krita/psd-tools keep them); cropped only if an edge
  exceeds 30 000 px, with a warning.
* PNG always RGBA (no opaque RGB8 optimisation). Large exports buffer the compressed PSD layer section
  in memory (bounded by output size); compositing is banded.
* Device/HW evidence for PSD-007 (SAF provider, force-stop) not covered here.
