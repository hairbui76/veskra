# ADR-007: PSD writer selection
Status: Accepted (provisional)
Date: 2026-10-09
Owner: FMT
Related tickets: SPK-06, FMT-08..11

## Context
Candidates psd-great and PhotoCraft's PSD code are git-hosted, unaudited and could not be evaluated
end-to-end here; DECISIONS_AND_RISKS lists "minimal custom supported PSD writer from Adobe public
spec" as the fallback. v1 needs 8-bit RGB export only (import is P8).

## Decision
Implement an **independent minimal PSD writer** in `veskra-io` from the public Adobe Photoshop File
Formats Specification: header, color mode data, image resources (1039 ICC profile, 1005 resolution
info), layer and mask information (layer records, channel image data with RLE/PackBits or raw,
group section dividers via `lsct`, Unicode names via `luni`, clipping flag, user masks), and the
merged composite image data. 8-bit RGB, ≤ 30 000 px edges (PSD limit), no PSB.

**Independent oracles:** psd-tools (Python) parses the layer tree and recomposites; Krita 5.2.2
(headless `--export`) opens the file and renders it; ImageMagick reads the merged composite.
Photoshop verification is a human escalation (BLOCKED here).

## Alternatives considered
1. psd-great — broad structures, but git dependency, provenance/license review pending.
2. PhotoCraft PSD — tied to a larger desktop architecture (Q5).

## Acceptance experiment
PSD-001 psd-tools parse + layer names/order/visibility; PSD-003 ICC bytes equal; PSD-004 composite
vs Veskra CPU composite within threshold; Krita open/export without error. Photoshop: BLOCKED.

## Consequences
We own PSD bugs; corpus tests are mandatory.

## Rollback / fallback
Switch to an audited crate if it passes the same corpus with less code.

## Review
ORCH 2026-10-09 (provisional pending Photoshop evidence, R-14).
