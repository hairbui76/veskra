# ADR-004: Brush core
Status: Accepted
Date: 2026-10-09
Owner: BRH
Related tickets: SPK-04, BRH-01..07
Amended by: ADR-019 (schema-3 presets: image tips, textures, jitter, dynamics, wet edges; paint mixing still deferred)

## Context
Hokusai (MyPaint-inspired, fix15 linear tiles) is a candidate. Adopting it requires an Android
build, a fix15→ADR-005 pixel adapter (R-02), determinism audit and license review. The spike could
not be executed in this environment (no device for feel/latency; GitHub source access through
the egress proxy was not established), and MASTER_PLAN §4 explicitly allows "implement a smaller
custom brush core".

## Decision
Veskra v1 uses an **independent, small Rust brush engine** (`veskra-brush`):
* dab-based stamping along arc-length-spaced confirmed samples, pressure → size/opacity/flow via
  editable curves, hardness falloff, optional seeded procedural grain (charcoal/textured pencil);
* per-stroke coverage accumulation buffer composited onto a base snapshot of the layer, which gives
  stroke-level opacity without self-overlap darkening, exact undo bases and alpha-lock/selection
  handling;
* fully deterministic for identical (samples, preset, seed);
* each preset declares `vector_eligible` honestly: solid round pens/inks/pencils without texture are
  vector-eligible; airbrush/textured/charcoal/eraser are raster-only.
Hokusai remains a candidate for P8 NEXT-04 (wet/smudge brushes) behind a new ADR.

## Alternatives considered
1. Hokusai now — richer MyPaint behaviour, but unverified build/adapter cost and fix15 semantics.
2. AndroidX Ink brushes — would split stroke authority across Kotlin and Rust (R-17).

## Acceptance experiment
PNT-001 determinism goldens (AUTO), PNT-002 qualitative goldens (AUTO part), physical pen feel review
(HW, BLOCKED here).

## Consequences
No smudge/wet media in v1 (already deferred by MASTER_PLAN §1.2).

## Rollback / fallback
Replace the dab renderer behind the `StrokeRenderer` API if device feel reviews fail.

## Review
ORCH 2026-10-09.
