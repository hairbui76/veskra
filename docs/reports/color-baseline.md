# Color baseline report (SPK-05 host part, CLR-01, CLR-02, CLR-05 color part)

**Owner:** CLR · **Date:** 2026-10-09 · **Crate:** `crates/veskra-color` · **Decision context:** ADR-005, ADR-006

Every number below comes from runs executed on 2026-10-09 in the development container
(x86_64 Linux). Machine-readable results: `tests/fixtures/color/color_report.json`.
Nothing here is a device or display measurement.

## 1. Environment and tool versions

| Item | Version |
|---|---|
| Rust toolchain | rustc 1.97.0 (2d8144b78 2026-07-07), cargo 1.97.0 (pinned by `rust-toolchain.toml`) |
| Python | 3.13.16 |
| Pillow (ImageCms wraps LittleCMS) | 12.3.0 |
| LittleCMS (`PIL.ImageCms.core.littlecms_version`) | 2.19 |
| colour-science | 0.4.7 (SciPy/Matplotlib absent; not needed for the functions used) |
| numpy | 2.5.3 |

LittleCMS and colour-science are test-time oracles only (ADR-006); the production crate is
pure safe Rust (`unsafe_code = "deny"`) with no C dependency.

## 2. Exact commands

```sh
export CARGO_TARGET_DIR=/tmp/claude-0/target-clr
# 1. generated ICC files (deterministic; test fails if committed bytes are stale)
VESKRA_REGEN_COLOR_FIXTURES=1 cargo test -p veskra-color --test icc_fixtures
# 2. independent oracle fixtures (LittleCMS + colour-science)
python3 -I scripts/color/gen_oracle_fixtures.py
# 3. measurements + machine-readable report
VESKRA_WRITE_COLOR_REPORT=1 cargo test -p veskra-color -- --nocapture --test-threads=1
# optional local third-party corpus (files not committed)
VESKRA_LOCAL_ICC_DIR=/usr/share/color/icc/krita cargo test -p veskra-color --test icc_robustness -- --nocapture
# gates
cargo fmt -p veskra-color -- --check
cargo clippy -p veskra-color --all-targets -- -D warnings
cargo test -p veskra-color
```

Running the generator twice produced byte-identical `expected_conversions.json`,
`srgb_swatches.csv` and ICC files (verified with `sha256sum -c`).

## 3. Results

### 3.1 Gate checks (from `color_report.json`)

| Check | Definition | Samples | Measured | Threshold | Status |
|---|---|---|---|---|---|
| COL-002 | sRGB→sRGB 8-bit, identity passthrough | 16,777,216 (all 8-bit RGB) | max channel diff **0** | ≤ 1 LSB | PASS |
| COL-002 | sRGB→sRGB 8-bit, full decode→matrix→encode pipeline (identity detection disabled) | 16,777,216 | max channel diff **0** | ≤ 1 LSB | PASS |
| COL-003 | 8-bit sRGB swatch → Veskra sRGB→Display P3 (relative colorimetric, 8-bit output) → Lab D50 via generated P3 profile, vs colour-science Lab D50 of the source swatch | 183 | ΔE00 median **0.0878**, p95 **0.3004**, max **0.4139** | median ≤ 1.0, p95 ≤ 2.0 | PASS |

COL-002 here covers the in-engine transform. The PNG encode/decode leg of COL-002 belongs to the
PNG exporter/oracle (FMT-07 / CLR-05 PNG part) and was not run by CLR.

### 3.2 Veskra vs LittleCMS 2.19 (COL-008), 183 swatches, 8-bit RGB

ΔE00 is computed between Veskra's and LittleCMS's 8-bit outputs, both evaluated in Lab D50
through the destination profile.

| Conversion (src → dst / intent / LittleCMS flags) | max 8-bit diff | exact match | ΔE00 median / p95 / max |
|---|---|---|---|
| Veskra sRGB → Veskra P3 / relative / default (optimized) | 1 | 98.9 % (181/183) | 0 / 0 / 0.3503 |
| Veskra sRGB → Veskra P3 / relative / NOOPTIMIZE | 0 | 100 % | 0 / 0 / 0 |
| Veskra P3 → Veskra sRGB / relative / default | 1 | 98.9 % (181/183) | 0 / 0 / 0.3239 |
| Veskra P3 → Veskra sRGB / relative / NOOPTIMIZE (108 of 183 inputs are outside sRGB and clip) | 0 | 100 % | 0 / 0 / 0 |
| Veskra sRGB → Veskra P3 / perceptual / NOOPTIMIZE | 0 | 100 % | 0 / 0 / 0 |
| Veskra sRGB → Veskra P3 / absolute colorimetric / NOOPTIMIZE | 0 | 100 % | 0 / 0 / 0 |
| LittleCMS built-in sRGB → Veskra P3 / relative / NOOPTIMIZE | 0 | 100 % | 0 / 0 / 0 |
| Veskra sRGB → Veskra sRGB / relative / default | 0 | 100 % | 0 / 0 / 0 |
| LittleCMS built-in sRGB → Veskra sRGB / relative / NOOPTIMIZE | 0 | 100 % | 0 / 0 / 0 |

The only differences (2 samples, 1 LSB) appear against LittleCMS's *optimized* 8-bit path, whose
precalculated tables trade precision for speed; against its unoptimized pipeline Veskra is
bit-identical, including per-channel clipping of out-of-gamut P3 colors and the
perceptual→relative and absolute→relative behavior for v4 display profiles.

### 3.3 Veskra vs colour-science

| Quantity | Measured max abs difference | Asserted bound |
|---|---|---|
| `srgb8_to_lab_d50` vs colour-science (sRGB, Bradford → ICC D50 white) | 2.771e-10 ΔL/Δa/Δb | < 1e-6 |
| Lab via generated P3 ICC profile vs colour-science exact Display P3 | 6.829e-3 | < 1e-2 |
| Float sRGB→P3 (`apply_f64`) vs colour-science `RGB_to_RGB` | 2.507e-4 (0.064 LSB at 8 bit; worst at sRGB (64, 0, 255)) | < 0.1 LSB |
| CIEDE2000, 34 Sharma pairs + 183 swatch pairs | 4.121e-10 | < 1e-9 |
| CIEDE2000 vs published Sharma Table 1 (4 decimals) | all 34 within 5e-5 | < 5e-5 |

The P3 and float differences are entirely the ICC `s15Fixed16` quantization of colorants and
`para` parameters (≈1.5e-5 per value) stored in the profile; LittleCMS reads the same quantized
values (its colorants equal ours to < 1e-11).

Note: colour-science's built-in `sRGB` colourspace uses the IEC 61966-2.1 matrix **rounded to 4
decimals**. Veskra and LittleCMS derive the matrix from the published chromaticities, so the oracle
uses colour-science with derived matrices. With the rounded matrix the Lab reference would move by up
to 2.24e-2 (recorded in `expected_conversions.json` → `notes`).

### 3.4 Generated ICC profiles (independent validation)

| File | Bytes | SHA-256 (prefix) | LittleCMS opens | Description read by LittleCMS | MD5 Profile ID |
|---|---|---|---|---|---|
| `veskra-srgb-v4.icc` | 564 | a4ff172a107192ea | yes, matrix-shaper, v4.3, all 4 intents supported | "sRGB IEC61966-2.1 (Veskra)" | valid |
| `veskra-display-p3-v4.icc` | 552 | 60a07ed0d1fa78c7 | yes, matrix-shaper, v4.3 | "Display P3 (Veskra)" | valid |

Structure: v4.3 display class, `desc`/`cprt` (`mluc`, en-US), `wtpt` = D50, `chad` (Bradford
D65→D50), `rXYZ/gXYZ/bXYZ` (Bradford-adapted, rows quantized so R+G+B equals the encoded D50 white
exactly), one shared `para` type-3 sRGB curve for r/g/bTRC, fixed header date, MD5 Profile ID.
The Display P3 colorants match Apple's published D50 values to < 2e-4. The LittleCMS built-in
sRGB profile is recognized by `is_srgb_equivalent()` and `RgbTransform` treats it as identity to
Veskra sRGB.

### 3.5 Parser robustness

| Corpus | Cases | Result |
|---|---|---|
| Every truncation length of 3 seed profiles (Veskra sRGB, P3, LittleCMS sRGB), as-is and with the header size patched | 1,704 lengths × 2 | all as-is truncations rejected; 0 panics |
| xorshift64* fixed-seed mutations (byte flips, edge-value u32 overwrites, tag-area bit flips, resize) | 15,000 | 7,195 accepted (and exercised as transform src+dst), 0 panics |
| Random bytes with valid magic/class/space | 5,000 | 0 panics |
| Local third-party corpus `/usr/share/color/icc/krita` (35 files, not committed) | 35 | 22 RGB/Gray matrix-TRC profiles parsed; `sRGB-elle-V2-srgbtrc` recognized as sRGB; CMYK, YCbCr (8), abstract Lab/XYZ (3) and the LUT-only PQ profile (13 total) rejected with typed errors; 0 panics |

Unit tests additionally cover each typed error: truncated, size mismatch, > 4 MiB
(`AllocationLimit`), bad magic, out-of-bounds/overlapping/duplicate tags, absurd tag count,
missing tag, singular matrix, bad `curv`/`para`, CMYK/Lab spaces, device-link class, v5,
LUT-only (`UnsupportedColorMode`).

## 4. Documented behavior (normative for consumers)

* **Intents:** perceptual and saturation use the colorimetric model (= relative colorimetric) for
  matrix/TRC profiles, as LittleCMS does; LittleCMS's v4 perceptual black-point compensation is a
  no-op for zero black points (verified above, 100 % match). Absolute colorimetric scales PCS XYZ by
  `src_media_white / dst_media_white`, with LittleCMS's media-white rule (D50 for v2 display
  profiles and missing `wtpt`).
* **Out of gamut:** per-channel clip of destination linear RGB to [0, 1]; no gamut mapping.
* **Identity:** colorants within 5e-4 and curves within 5e-4 (encoded round-trip) → exact passthrough.
* **sRGB equivalence:** colorants within 2e-3 and curves within 2e-3 of the reference; a 2.2-gamma
  profile with sRGB primaries is *not* equivalent.
* **Alpha/depth (`convert`):** contract rounding (CONTRACTS §1); alpha 0 → (0,0,0,0); straight
  8-bit ↔ premultiplied 16-bit round-trips exactly for every (value, alpha ≥ 1) pair (exhaustive
  test); premultiplied→straight 8-bit error ≤ 0.5 LSB for every 16-bit alpha (dense test).

## 5. Limitations and blocked items

* **No device/display measurement was possible** (no tablet, no colorimeter in this
  environment): **COL-011 remains BLOCKED**; the CLR-04 display transform/negotiation and
  SPK-05's GPU/display path tracing are not done.
* **LittleCMS Android build (SPK-05 item) not applicable:** ADR-006 selected the pure-Rust engine;
  LittleCMS is only a host-side oracle here.
* Only **RGB matrix/TRC and Gray TRC** profiles are supported. LUT-based profiles (CMYK, A2B-only,
  Lab-PCS, PQ/HLG LUT profiles), device links, abstract and named-color profiles are rejected with
  typed errors (NEXT-03). A profile carrying both LUT and matrix/TRC tags is accepted using the
  matrix/TRC model and flagged via `has_lut_tags()` (LittleCMS would use the LUT).
* No black-point compensation; profiles with raised black points may differ from LittleCMS's
  perceptual result near black. No gamut mapping beyond clipping.
* Gray profiles convert to RGB only (`GrayToRgbTransform`); RGB→Gray is not implemented.
* LittleCMS comparisons are 8-bit only (Pillow does not expose 16-bit RGB transforms); 16-bit
  paths are covered by Rust-internal tests, not by the external oracle.
* Values are clamped to [0, 1]; extended-range (scRGB-style) encodings are not supported.
* The swatch set (183) is Veskra-chosen; COL-003 thresholds remain the MASTER_PLAN §6.3 proposals
  and are not recalibrated by this report (QAP owns that ADR).
* PNG/PSD round-trip color checks (CLR-05 PNG part, COL-009/010) require exporter output and
  independent decoders/apps; not run by CLR in this task.
