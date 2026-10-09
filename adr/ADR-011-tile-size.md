# ADR-011: Tile size
Status: Accepted (provisional)
Date: 2026-10-09
Owner: REN
Related tickets: CORE-04, REN-02, REN-04

## Decision
`TILE_SIZE = 64` (32 KiB per RGBA16 tile). Small tiles keep copy-on-write undo deltas and per-dab
dirty regions small, matching MyPaint-style engines; display uploads use separate 512 px bitmaps.

## Alternatives considered
128/256 px tiles — fewer map entries and better streaming for large fills, but 4–16× larger undo
copies per touched tile.

## Acceptance experiment
Device benchmark of brush dab density, undo memory and upload cost for 64/128/256 — BLOCKED (no
device). Host measurements may inform but do not decide this ADR.

## Rollback / fallback
`TILE_SIZE` is a single constant in `veskra-core`; changing it requires a native-format migration
(tile size is recorded in `document.json`).

## Review
ORCH 2026-10-09 (provisional).
