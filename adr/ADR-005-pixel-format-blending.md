# ADR-005: Pixel format, premultiplication and blend policy
Status: Accepted
Date: 2026-10-09
Owner: CLR + REN
Related tickets: CLR-01, CORE-04, DOC-02, CLR-03

## Context
Low-flow brush accumulation and premultiplied storage at low alpha lose precision in 8 bits
(stuck values, halos on unpremultiply; COL-006). Photoshop/Krita composite 8/16-bit RGB documents
on transfer-encoded values by default, so PSD composites only match if Veskra does the same (R-11).

## Decision
* Authoritative layer tiles: **RGBA u16, premultiplied, transfer-encoded in the document working
  profile** (`PixelDescriptor::WORKING_V1`; sRGB in v1). Invariant `r,g,b ≤ a`.
* Masks/selections: u8 coverage.
* `ColorRgba` at API boundaries: straight alpha, encoded, f32.
* Document-level `BlendSpace`: **`GammaEncoded` (default)** — Normal/Multiply/Screen/Overlay use the
  W3C Compositing and Blending Level 1 formulas on encoded values — or `Linear` (experimental; exporters
  warn `BlendSpaceMismatch`).
* All depth/alpha conversions are implemented once in `veskra-color::convert` with documented rounding.
* Memory: 8 B/px → a fully painted 4096² layer is 128 MiB; sparse tiles bound real use. Budget
  review at G6 (R-09); fallback is U8 storage for display caches only, never for authoritative tiles.

## Alternatives considered
1. RGBA8 premultiplied — half the memory; fails low-alpha precision and accumulation (rejected).
2. f16/f32 linear — best physical blending; doubles/quadruples memory, mismatches Photoshop composites.

## Acceptance experiment
COL-006 alpha fixtures (0, 1/255, 0.5, 1) round-trip without halos; PNT-007 formula tests;
PSD composite vs psd-tools/Krita recomposite comparison (FMT-08/11).

## Consequences
Exporters convert u16 premultiplied → 8-bit straight once via `veskra-color::convert`.

## Rollback / fallback
If device memory tests fail, introduce tile compression of cold tiles before reducing precision.

## Review
ORCH 2026-10-09.
