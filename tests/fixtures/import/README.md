# PNG import fixtures (COL-004 / COL-005 / SEC-001)

Used by `crates/veskra-io/tests/import_png.rs`. All files are generated; nothing is copied from
third-party images, profiles or test suites.

| File | Content | Origin |
|---|---|---|
| `p3_icc_rgb8.png` | 16×16 RGB8 swatches tagged with Veskra's Display P3 v4 profile | Pillow 12.3.0, `gen_fixtures.py` |
| `srgb_chunk_rgba8.png` | 16×16 RGBA8, `sRGB` chunk, alpha ramp incl. colored alpha-0 pixels | Pillow, `gen_fixtures.py` |
| `lcms_srgb_icc_rgb8.png` | 16×16 RGB8 tagged with LittleCMS's built-in sRGB profile (independent sRGB bytes) | Pillow, `gen_fixtures.py` |
| `untagged_rgb8.png` | 16×16 RGB8, no color chunks | Pillow, `gen_fixtures.py` |
| `gray18_icc_gray8.png` | 16×16 Gray8 ramp tagged with `gray-gamma18-v4.icc` | Pillow, `gen_fixtures.py` |
| `gray-gamma18-v4.icc` | ICC v4 Gray profile, `kTRC` curv gamma 1.8, D50 `wtpt` | written byte-by-byte by `gen_fixtures.py` |
| `palette_trns.png` | 16×16 indexed, 64-entry palette with partial `tRNS` alpha | Pillow, `gen_fixtures.py` |
| `interlaced_rgba8.png` | 13×11 Adam7 RGBA8, `sRGB` chunk (Pillow cannot write Adam7; a minimal writer in the script does, and Pillow must decode it back identically) | `gen_fixtures.py` |
| `expected.json` | Pillow-decoded source pixels and LittleCMS 2.19 (Pillow `ImageCms`, relative colorimetric, `NOOPTIMIZE` and default flags) conversions to Veskra sRGB | `gen_fixtures.py` |

Source profiles come from `tests/fixtures/color/icc/` (see `tests/fixtures/color/LICENSES.md`).
Hostile inputs (corrupt `iCCP`, CMYK/LUT profiles, truncation, bad CRC, oversized headers,
mutations) and 16-bit inputs are generated at test time with the `png` crate encoder or a small
chunk writer in the test, so they are not committed.

Regenerate (requires `scripts/requirements-oracles.txt`; output is byte-for-byte reproducible):

```
python3 tests/fixtures/import/gen_fixtures.py
```

Terms: generated test data, MIT OR Apache-2.0 (Veskra project). LittleCMS and Pillow are
test-time oracles only and are not shipped.
