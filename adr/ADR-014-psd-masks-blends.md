# ADR-014: PSD supported subset and flattening policy
Status: Accepted (provisional)
Date: 2026-10-09
Owner: FMT + CLR
Related tickets: FMT-08, FMT-10, PSD-005, PSD-006

## Decision
Preserved natively in PSD: raster layers, groups (isolated → `norm`/section dividers; pass-through →
`pass`), names (Pascal + `luni`), visibility, opacity, Normal/Multiply/Screen/Overlay, clipping
(`clipping` byte), layer masks (user mask channel −2), alpha lock (`lspf`/protection flags are
editing state only). Vector-stroke layers are written as their raster pixels (Veskra geometry stays
in `.veskra`). Document in `Linear` blend space → layers still written, `BlendSpaceMismatch` warning,
merged composite reflects Veskra's render. Anything else is rasterized per layer with an
`ExportWarning`; never silently dropped.

## Acceptance experiment
psd-tools recomposite and Krita render compared to the Veskra composite; Photoshop BLOCKED.

## Review
ORCH 2026-10-09 (provisional).
