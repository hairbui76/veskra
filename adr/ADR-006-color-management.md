# ADR-006: Color management implementation
Status: Accepted
Date: 2026-10-09
Owner: CLR
Related tickets: SPK-05, CLR-02..05, COL-001..012

## Context
v1 working space is sRGB (MASTER_PLAN §6). LittleCMS (lcms2) is mature but is a C library: Android
static builds need the NDK, which is unavailable here, and it adds an unsafe FFI surface.

## Decision
* `veskra-color` implements, in safe Rust, a bounded ICC parser for **RGB matrix/TRC and gray TRC
  profiles (v2 and v4)**, matrix-shaper transforms via XYZ D50 (relative and absolute colorimetric;
  perceptual/saturation map to relative colorimetric for matrix profiles, as LittleCMS does), CIELAB
  and CIEDE2000.
* Veskra generates its **own** sRGB IEC 61966-2.1 and Display P3 v4 profiles in code (deterministic
  bytes; no third-party profile redistribution).
* LUT-based profiles (CMYK, A2B-only) are **rejected with a typed error** in v1 rather than
  approximated (COL-004/COL-012 behaviour: warn/refuse, never mislabel).
* **Independent oracle (COL-008):** LittleCMS via Pillow's `ImageCms` and colour-science generate the
  fixture expectations in `tests/fixtures/color/`; they are test-time tools only, not shipped.
* Missing source profile → explicit sRGB assumption with `assumed = true` metadata.

## Alternatives considered
1. lcms2 crate in production — most complete; C build on Android unverified; larger attack surface.
2. qcms/moxcms — pure Rust, but would still need audit; our scope (matrix/TRC) is small enough to own.

## Acceptance experiment
Rust transforms vs LittleCMS on ≥128 swatches: median ΔE00 ≤ 1.0, p95 ≤ 2.0 (COL-003); sRGB 8-bit
identity bit-exact (COL-002); generated ICC files open in LittleCMS. Results: `docs/reports/color-baseline.md`.

## Consequences
CMYK/LUT profile import is a P8 item (NEXT-03) needing either a LUT evaluator or lcms2.

## Amendment
2026-10-12: ADR-018 adds LUT-based CMYK **output** profiles (`mft1`/`mft2`/`mAB`/`mBA`) for
export and color conversion, with a bundled FOGRA39 profile. Working/document profiles remain
matrix/TRC RGB or Gray; LUT-only profiles are still rejected as working profiles.

## Rollback / fallback
Swap the transform backend for lcms2 behind `RgbTransform` if broader profile support is required.

## Review
ORCH 2026-10-09; numeric evidence attached by CLR report.
