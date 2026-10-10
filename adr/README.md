# Architecture Decision Records

Template: `DECISIONS_AND_RISKS.md` §4. IDs are immutable. Status values: Proposed | Accepted | Accepted (provisional) | Superseded | Rejected.

"Accepted (provisional)" means ORCH accepted the decision so dependent work can proceed, but the
ADR's acceptance experiment needs evidence this environment cannot produce (device, Adobe apps).
Such ADRs stay provisional until QAP attaches that evidence; they must not be cited as verified.

| ADR | Title | Status |
|---|---|---|
| [001](ADR-001-license.md) | License and re-use policy | Accepted |
| [002](ADR-002-android-platform.md) | Android platform floor/target, ABIs, namespace | Accepted (provisional) |
| [003](ADR-003-ui-framework.md) | Kotlin + Compose shell, canvas View with tiled bitmaps | Accepted (provisional) |
| [004](ADR-004-brush-core.md) | Independent Rust brush core (Hokusai not adopted for v1) | Accepted |
| [005](ADR-005-pixel-format-blending.md) | RGBA16 premultiplied encoded tiles, gamma-encoded blend default | Accepted |
| [006](ADR-006-color-management.md) | Pure-Rust matrix/TRC ICC engine; LittleCMS as test oracle | Accepted |
| [007](ADR-007-psd-writer.md) | Independent PSD writer from the public Adobe spec | Accepted (provisional) |
| [008](ADR-008-illustrator-export.md) | SVG/PDF Illustrator handoff, no native `.ai` | Accepted |
| [009](ADR-009-native-container.md) | `.veskra` ZIP container v1 and staged save | Accepted |
| [010](ADR-010-rendering.md) | CPU compositor + platform bitmap presentation; GPU deferred | Accepted (provisional) |
| [011](ADR-011-tile-size.md) | 64 px tiles | Accepted (provisional) |
| [012](ADR-012-hybrid-visibility.md) | Hybrid vector/raster visibility strategy (SVG hidden vectors, PDF residual) | Accepted (provisional) |
| [013](ADR-013-wide-gamut.md) | sRGB-only v1; Display P3 gated (export clause amended by 017) | Accepted |
| [014](ADR-014-psd-masks-blends.md) | PSD supported subset and flattening policy | Accepted (provisional) |
| [015](ADR-015-min-hardware.md) | Minimum hardware | Proposed (blocked) |
| [016](ADR-016-core-contracts.md) | Frozen core contracts (ARC-01) | Accepted |
| [017](ADR-017-tool-set-and-export-color.md) | Painter tool set and export color targets (amends 013 for export) | Accepted (provisional) |
