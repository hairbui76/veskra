# Blockers and evidence gaps (execution session 2026-10-09)

Per AGENTS.md: "If not runnable, mark `Not run` with blocker and do not tick DoD." Every item
below is reflected in `progress/tickets.json` (status `Blocked` or `In Verification` with a
`blockers` entry). None of these were substituted with mocked or invented results.

| # | Blocker | Root cause | Affected tickets / tests | What was done instead | How to unblock |
|---|---|---|---|---|---|
| B-1 | **No Android SDK/NDK** | Network policy denies `dl.google.com` (SDK, NDK, platform jars, build-tools). Not routed around. | SPK-01, AND-01, `:app` compile/lint, APK, REL-02 | Rust engine + FFI type-check for `aarch64-linux-android` / `x86_64-linux-android`; pure-JVM `:kit` with real JNI tests on host; `:app` written but **never compiled**; CI job `android-apk` defined for GitHub runners (unverified) | Allow `dl.google.com` in the environment network policy, or run CI on GitHub-hosted runners |
| B-2 | **No emulator / device** | No KVM in container; no physical tablet | SPK-02/03/08/09/12, STY-003/005/006/008/009 HW, PERF-001..005, SAV-004/008, SEC-003, COL-011 | Synthetic event traces (labelled SYNTHETIC), headless arbitration tests, host CPU timings (labelled HOST) | Physical pen tablets from ≥2 GPU vendors (MASTER_PLAN §1.3) |
| B-3 | **No Adobe Photoshop / Illustrator** | No licence/installation (R-14) | PSD-002/004 ADOBE, COL-009/010, ILL-001/002/003/005/007/009, ADR-012 acceptance | Independent open-source oracles: psd-tools, Krita 5.2.2, ImageMagick (PSD); rsvg, Inkscape 1.2.2, poppler, qpdf, pypdf (SVG/PDF); manual Illustrator checklist in `scripts/interop/` | Human reviewer with current Photoshop/Illustrator completes the checklists |
| B-4 | **GitHub push denied (403)** | Claude GitHub App lacks access to `hairbui76/veskra` | Publishing commits/CI runs | All work committed locally on `claude/vibrant-cori-0mnhk5` | Reconnect GitHub / install the Claude GitHub App on the repository |
| B-5 | **Hokusai / psd-great / PhotoCraft not evaluated** | Git-hosted sources not fetched; spikes need device | SPK-04, SPK-06 (candidate comparison) | ADR-004/007 chose independent implementations (documented fallbacks in DECISIONS_AND_RISKS) | Run the spikes with source access + device; new ADR if adopting |
| B-6 | **No calibrated display instrumentation** | Hardware | COL-011, device visual review (MASTER_PLAN §6.3) | Numeric ICC suite only (`docs/reports/color-baseline.md`) | Colorimeter + ≥2 physical displays |
| B-7 | **No beta users / legal review** | Human escalation (AGENT_ASSIGNMENTS) | REL-04/05/10/11, trademark of "Veskra", namespace `org.veskra.dev` | Provisional namespace documented in ADR-002 | Human owners |

Gate consequence: **no quality gate G0–G7 can be signed in this session**; ORCH + QAP sign-off
requires the hardware and Adobe evidence above. Implementation-complete tickets are recorded as
`In Verification` with `impl: complete`.
