# ADR-018: CMYK output (print export and CMYK color picker)
Status: Accepted (provisional)
Date: 2026-10-12
Owner: CLR + FMT
Related tickets: NEXT-03 (soft proof stays open), NEXT-11, COL-008, COL-012
Amends: ADR-006 (LUT-based CMYK *output* profiles are supported for export and conversion),
ADR-017 (export color targets gain `cmyk`; `renderingIntent` and `blackPointCompensation` options)

## Context
The owner asked for a CMYK color system for print export. ADR-006 limited the color engine to
matrix/TRC RGB and Gray profiles and rejected every LUT-based profile. Print shops expect CMYK PSD
or PDF files converted with a known press profile; the Android color picker needs CMYK numbers and
an out-of-gamut indication for the same profile.

## Decision
1. **Scope.** CMYK is an **output** space. The source / working space stays RGB (sRGB in v1):
   documents, layers, compositing and display are unchanged, and `IccProfile::parse` still rejects
   LUT-only and CMYK profiles as working profiles. Abstract (`abst`) and device link (`link`)
   profiles stay rejected.
2. **Parser** (`veskra_color::icc::lut`, safe Rust, no new crates): `mft1`, `mft2`, `mAB `, `mBA `
   with `curv` (identity / gamma / table) and `para` (types 0–4) curves, 3×3 matrix (+ offset for
   v4), 8- and 16-bit CLUTs (uniform or per-dimension grid), ≤ 4 CLUT inputs, Lab (v4 encoding;
   legacy 16-bit encoding for `lut16Type`, L* = 100 at `0xFF00`) and XYZ PCS. Every count, offset
   and table size is checked against the tag size before use; CLUT bodies are read in place (no
   copy), so hostile files cannot cause large allocations. Output profiles chosen by the user may
   be up to 32 MiB (`MAX_OUTPUT_PROFILE_BYTES`); embedded document profiles keep the 4 MiB limit.
3. **Transforms** (`veskra_color::cmyk`): RGB matrix/TRC → PCS → `B2Ax` → CMYK and CMYK → `A2Bx` →
   PCS → RGB, evaluated in `f64` with LittleCMS-equivalent interpolation (tetrahedral for 3 inputs,
   linear × tetrahedral for 4). Intents: perceptual (`A2B0`/`B2A0`), relative colorimetric
   (`A2B1`/`B2A1`), saturation (`A2B2`/`B2A2`, internal only); a missing intent tag falls back to the
   perceptual one as in LittleCMS (then to any present table, a Veskra extension).
4. **Black point compensation** reproduces LittleCMS 2.19: black points detected with
   `cmsDetectBlackPoint` / `cmsDetectDestinationBlackPoint` semantics (CMYK output, relative: Adobe
   round-trip ramp, "nearly straight mid-range" test or least-squares quadratic fit, seeded by the
   neutralized perceptual black; CMYK input, perceptual: the darker colorant with a*/b* **kept**
   and L* clipped — the neutral variant of older LittleCMS releases differs from 2.19 by up to
   6 ΔE00 on CMYK → sRGB perceptual, which the oracle caught); v4 perceptual/saturation forces BPC.
   **Export default: relative colorimetric + BPC** (Photoshop's default). For RGB matrix/TRC
   conversions perceptual = relative colorimetric (no perceptual tables) and BPC is a no-op for
   every RGB profile Veskra writes, so RGB output bytes are unchanged.
5. **Gamut.** A color is in gamut when RGB → CMYK → PCS, mapped back through the exact inverse of the
   forward black point mapping, is within **ΔE00 2.0** of the source. The same definition drives
   `color.convert.inGamut`, preflight warnings and `outOfGamutPixels`.
6. **Performance.** Bulk conversion uses a precomputed 33³ device link (LittleCMS's default grid for
   RGB input) of `f32` (C, M, Y, K, round-trip ΔE00) at gamma-encoded RGB nodes, tetrahedral
   interpolation, built once per (profile, document profile, intent, BPC) on up to 8 threads and
   cached per process; large buffers are converted on up to 8 threads (output independent of
   scheduling). Single colors (`color.convert`, PDF vector fills) use the direct pipeline.
7. **Bundled profile.** `crates/veskra-color/profiles/Coated_Fogra39L_VIGC_300.icc` — X-Rite, Inc.,
   made available by VIGC through the ICC profile registry: FOGRA39L, ISO 12647-2 coated offset,
   300 % TAC, ICC v2.1 `prtr`, CMYK → Lab, `mft2` A2B0–2 (grid 25) / B2A0–2 (grid 37). 8 652 444
   bytes, SHA-256 `6df43849…92d47e3`. Terms: may be used, embedded, exchanged and shared without
   restriction; may not be altered or sold without written permission of VIGC. It is compiled in
   (`include_bytes!`, one `static`) and embedded byte for byte; it is never re-encoded.
8. **Formats.**
   * **PSD**: color mode 4, 8/16 bit, layers and merged image converted per pixel; channels
     transparency, C, M, Y, K (+ mask); values inverted (Photoshop: max = no ink); merged image matted
     over paper (ink · alpha); resource 1039 = the CMYK profile. Veskra composites in RGB, so layers
     with non-Normal blend modes are reported (`blend_mode_approximated`): Photoshop recomposites CMYK
     layers in CMYK. Layers are not flattened (ADR-014 policy: warn and count, never drop).
   * **PDF**: **DeviceCMYK + a PDF/X-style output intent** (`/S /GTS_PDFX`, `/DestOutputProfile` with
     the profile, embedded once), chosen over `/ICCBased` N=4: print RIPs and Acrobat preflight pass
     DeviceCMYK numbers through unchanged (no CMYK → CMYK re-separation; a 100 % K black stays
     K-only) and Illustrator opens DeviceCMYK objects as editable CMYK colors. Images are DeviceCMYK
     with the same soft masks, vector fills use `k`, all transparency groups blend in DeviceCMYK, the
     "Editable vector strokes" / "Preserve original appearance" semantics are unchanged. The file is
     not claimed PDF/X conformant.
   * **PNG / SVG** cannot carry CMYK: preflight `blockedReason`, `export.run` →
     `IncompatibleOutputProfile`.
   * Untagged CMYK (`embedColorProfile: false`) is allowed with a warning; untagged Display P3 /
     Adobe RGB stays refused.
9. **Commands** (`docs/COMMANDS.md`): `color.convert` and `profile.inspect` (read-only, no undo
   step); export options `cmykProfile`, `renderingIntent`, `blackPointCompensation`; preflight /
   report fields `totalAreaCoverageMax` and `cmykProfile`.

## Acceptance experiment and results (2026-10-12, Windows host)
Independent oracle: LittleCMS 2.19 through Pillow 12.3.0 `ImageCms` (test-time tool only),
`scripts/color/gen_cmyk_oracle.py` → `tests/fixtures/color/cmyk_expected.json`, 707 sRGB swatches
(primaries, neutral ramp, near-black, near-white, skin, saturated, 18 out-of-gamut colors, 5³ and
8³ grids). All LittleCMS buffers are 8-bit (0.39 % ink quantization). Test:
`crates/veskra-color/tests/cmyk_oracle.rs`. CMYK channel difference |Veskra − LittleCMS| in percent
ink over all C, M, Y, K values (target: median ≤ 1, p95 ≤ 2, max ≤ 4):

| Intent | Veskra direct vs LittleCMS full precision (NOOPTIMIZE) | Veskra 33³ link vs LittleCMS 33³ link (default) |
|---|---|---|
| relative + BPC (default) | 0.070 / 0.340 / 3.807 | 0.070 / 0.303 / 2.527 |
| relative, no BPC | 0.063 / 0.318 / 2.353 | 0.061 / 0.286 / 2.839 |
| perceptual + BPC | 0.093 / 0.288 / 1.220 | 0.092 / 0.289 / 1.321 |
| perceptual, no BPC | 0.092 / 0.295 / 1.286 | 0.091 / 0.286 / 0.863 |

For scale, LittleCMS's own 33³ link differs from its full-precision pipeline by up to 3.9 % on the
same swatches.

* CMYK → Lab (5⁴ CMYK grid): ΔE00 median 0.256 / p95 0.526 / max 0.768 (relative), 0.250 / 0.529 /
  0.731 (perceptual) — LittleCMS's 8-bit Lab output alone quantizes by up to 0.2 L*, 0.5 a*/b*.
* Round trip sRGB → CMYK → Lab vs LittleCMS's (relative + BPC): ΔE00 median 0.228 / p95 0.485 /
  max 1.509. CMYK → sRGB: 8-bit codes equal (median 0, max 1 code) for relative ± BPC and
  perceptual + BPC.
* Synthetic ICC v4 profile (`mAB `/`mBA `, curv + para, matrix with offset, 16- and 8-bit CLUTs,
  non-uniform grid; written by the oracle script, evaluated by LittleCMS): ink max 0.29 %, Lab
  ΔE00 max 0.65. A second v4 profile with analytic tables is built in
  `tests/cmyk_robustness.rs`; truncations and 3 600 random mutations never panic.
* **Accuracy cost of the 33³ link** vs the direct pipeline (140 608 colors, 5-code steps, off the
  grid): worst channel ink median 0.075 % / p95 0.514 % / max 4.13 % (relative + BPC), 0.084 /
  0.385 / 1.94 % (perceptual + BPC); ΔE00 between the two inks median 0.03, p95 0.19, max 1.16 /
  1.23; the gamut flag (interpolated round-trip ΔE) disagrees for 241 / 463 of 140 608 colors.
* Interop run locally (Windows; psd-tools 1.24.0, pypdf 6.17.0, Pillow 12.3.0 / LittleCMS 2.19,
  poppler pdftoppm 24.04, `scripts/interop/cmyk_check.py`): psd-tools opens 8- and 16-bit CMYK PSDs
  (mode CMYK, ICC 1039 = the profile); written ink vs LittleCMS's ink for the same pixels median 0 %,
  p95 0.39 %, max 1.96 %; pypdf reads the output intent (GTS_PDFX, N 4) and DeviceCMYK images;
  poppler renders the PDFs.
* Performance (release build, desktop, 16 threads; two full-canvas layers with a different color in
  almost every pixel, `crates/veskra-io/tests/export_cmyk_perf.rs`): 2048² CMYK PSD 0.31 s (RGB
  0.16 s), 4096² CMYK PSD 1.60 s (RGB 0.92 s), 4096² 16-bit 1.99 s, 4096² CMYK PDF 3.29 s (RGB
  1.81 s); preflight estimate 0.08–0.13 s (first use additionally builds the link).
  `color.convert` warm call ≈ 5 µs through the JSON boundary (test profile).
* Size: the stripped arm64 `libveskra_ffi.so` grows from 2 735 856 to 11 542 888 bytes (+8.8 MB =
  the profile + ~155 KB of code).

## What is NOT verified
* No physical print, press proof or measured print was made; no colorimeter was used.
* No file was opened in Photoshop, Acrobat, Illustrator or InDesign. In particular unverified:
  Photoshop's reading of the inverted CMYK channels, transparency channel and 16-bit CMYK `Lr16`;
  Photoshop's recomposite of CMYK layers; Acrobat's use of the output intent; Illustrator's handling
  of DeviceCMYK transparency groups.
* The CI Linux interop step (`run_all.sh` 3b and the CMYK oracle regeneration) has not run yet;
  Krita and Inkscape were not run on CMYK files.
* The ICC v4 `lut16Type` Lab encoding follows the ICC specification (legacy encoding in both
  directions); LittleCMS skips that adjustment for v4 output tables. No real v4 `lut16` CMYK profile
  was tested. Saturation intent and XYZ-PCS CMYK profiles are implemented but have no oracle data.
* Preflight total area coverage is an estimate from a downsampled composite on large documents.

## Alternatives considered
1. **lcms2 C library** — reference behaviour, but a C build for Android (NDK) and an `unsafe` FFI
   surface (ADR-006 rejected it for the same reasons); stays the fallback behind `CmykTransform`.
2. **PDF `/ICCBased` N=4** — self-describing without output-intent awareness, but invites CMYK →
   CMYK re-separation in print workflows; rejected for print (decision 8).
3. **Flatten CMYK PSDs to the merged image only** — exact appearance but loses layers; rejected
   (ADR-014: never silently flatten). Compositing in CMYK inside Veskra is out of scope for v1.
4. **Direct pipeline per pixel** (no link) — ~10× slower; kept for single colors.

## Consequences
* `ExportOptions` is no longer `Copy` (it carries the profile path). JSON stays backward compatible;
  default-option files are byte-identical (`export_default_pin.rs`).
* The APK grows by ~8.8 MB per ABI. The profile's terms must stay in the third-party notices.

## Rollback / fallback
Hide `profile: "cmyk"` and the CMYK picker in the UI; the CMYK code paths are separate from the RGB
ones (RGB exports are byte-identical with or without them). The transform backend can be swapped
behind `CmykTransform` if a broader CMM is needed.

## Review
ORCH 2026-10-12 (owner request: CMYK color system for print export). Provisional until QAP attaches
Photoshop / Acrobat / Illustrator evidence and the CI interop run.
