# ADR-010: Rendering strategy
Status: Accepted (provisional)
Date: 2026-10-09
Owner: REN
Related tickets: SPK-03, REN-01..04, PNT-012

## Decision
v1 ships the **deterministic CPU compositor** (`veskra-compose`) as both the reference oracle and the
presentation path: composite dirty regions → RGBA8 premultiplied sRGB → platform bitmaps (ADR-003).
GPU acceleration of the *view transform* is delegated to Android HWUI (bitmap draw with matrix).
A wgpu/Vulkan compositor (REN-03) is deferred until SPK-03 can run on physical GPUs; GPU loss can
then fall back to this path, which never owns authoritative data.

## Alternatives considered
1. wgpu now — unverifiable here (no device); highest lifecycle risk (R-01).
2. OpenGL ES via GLSurfaceView — same verification gap.

## Acceptance experiment
Device frame-time traces for 2048² sketching ≥ 60 FPS (PERF-001) — BLOCKED. Host benchmarks of
composite throughput are recorded in `docs/reports/performance-host.md` (host CPU, not device).

## Review
ORCH 2026-10-09 (provisional).
