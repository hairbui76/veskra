# ADR-016: Frozen core contracts (ARC-01)
Status: Accepted
Date: 2026-10-09
Owner: ORCH
Related tickets: ARC-01, CORE-01..03

## Decision
`crates/veskra-core` defines the shared contracts: stable 128-bit IDs (UUID strings), geometry
(`RectI` half-open, `Affine` SVG/PDF matrix layout), typed `VeskraError`/`ErrorCode`, `StrokeSample`
with `CONFIRMED`/`PREDICTED`/`PRESSURE_REPORTED` flags (absent axes = `None`), `PixelDescriptor`
(ADR-005), `ColorRgba` (straight, encoded), `BlendMode`, safety limits, and the export
options/preflight/report types. Per-crate public surfaces are listed in `docs/CONTRACTS.md`.
Crate dependency direction is enforced by `scripts/check_deps.py` in CI:
core ← {color, raster, input, vector} ← {brush, doc} ← compose ← {format, io} ← engine ← {ffi, cli}.

## Consequences
Changing a core type requires a new ADR or an amendment approved by ORCH; additive enum variants are
allowed because public enums are `#[non_exhaustive]`.

## Review
ORCH 2026-10-09.
