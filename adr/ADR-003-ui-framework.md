# ADR-003: UI framework and canvas presentation
Status: Accepted (provisional)
Date: 2026-10-09
Owner: AND + DSN
Related tickets: AND-01, REN-01, SPK-03, SPK-09

## Context
MASTER_PLAN requires a Kotlin/Compose tablet shell, and forbids per-point painting through Compose
recomposition. Rust must own document, painting and rendering logic.

## Decision
* Kotlin + Jetpack Compose (Material 3) for all chrome: gallery, rails, sheets, dialogs.
* The canvas is a dedicated `android.view.View` (`CanvasView`) hosted via `AndroidView`. It forwards
  every `MotionEvent` (current + historical + predicted, flagged) to Rust in packed batches on a single
  engine thread. **All input arbitration (stylus vs finger vs palm, gestures, stroke state machine)
  runs in Rust (`veskra-input`)** so it is unit-testable headlessly.
* Presentation v1: Rust renders the document composite (including the live stroke) into RGBA8
  premultiplied sRGB buffers for dirty regions; Kotlin keeps a grid of 512×512 `ARGB_8888` bitmaps,
  updates only dirty ones (`copyPixelsFromBuffer`) and draws them with the Rust-provided view matrix
  (HWUI handles pan/zoom/rotate on the GPU). See ADR-010.
* AndroidX Ink is **not** used (SPK-09 not executed); it would introduce a second stroke model (R-17).

## Alternatives considered
1. SurfaceView + wgpu/Vulkan swapchain — lowest latency potential, highest lifecycle risk (R-01);
   deferred to REN-03/SPK-03 with hardware.
2. AndroidX Ink front-buffer overlay — possible latency overlay later; must not become authoritative.

## Acceptance experiment
On a physical tablet: 30-minute sketch, rotation/pause/resume, measured ink latency (QAP-05).
**BLOCKED** here (no SDK/device). Kotlin logic shared with the app lives in the pure-JVM `:kit`
module and is tested on the host JVM, including real JNI calls into the Rust library.

## Consequences
Bitmap upload cost scales with dirty area; 512 px display tiles bound the per-update upload.

## Rollback / fallback
If latency budgets fail on devices, add a front-buffered low-latency overlay (Ink or GL) for the live
stroke only, keeping Rust tiles authoritative.

## Review
ORCH 2026-10-09 (provisional pending device evidence).
