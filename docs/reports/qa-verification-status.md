# VERIFICATION_MATRIX status (QAP, 2026-10-09)

Determined by running tests in this session, not by assumption: `cargo test --workspace
--no-fail-fast` (53 test binaries + doc-tests, 452 tests passed, 0 failed, host), `cargo test -p veskra-qa`, `bash
scripts/interop/run_all.sh` (17/17 steps PASS, overall `FAIL_KNOWN_DEFECTS`), the 20 000-iteration
fuzz run, `cargo deny check licenses bans sources advisories` (all ok) and `bash scripts/qa/perf_host.sh`.
Kotlin `:kit` JVM tests and any Android build were **not run** in this session.

**Status meanings.** PASS = every evidence type the matrix requires was produced here (AUTO/COLOR
only). PARTIAL = the automated part passes but a required HW/ADOBE/UI/SEC part is missing (blocker
named). NOT_RUN = no required evidence produced (blocker named). FAIL = a check fails.
No gate is signed by this table (AGENTS.md §7). Blockers: `docs/BLOCKERS.md`.
Abbreviations: `qa:S1` = `target/qa/S1/result.json` from `crates/veskra-qa/tests/scenarios.rs`.

## Stylus and gestures

| ID | Status | Evidence / blocker |
|---|---|---|
| STY-001 | PARTIAL | AUTO: `veskra-input` `sty_001_thousand_events_monotonic_pressure_in_range`, `sty_001_out_of_order_historical_dropped_and_counted`; qa:S1 `pressure_tilt_order`. HW: B-2 |
| STY-002 | PARTIAL | AUTO: `sty_002_tilt_azimuth_preserved_or_none`; qa:S1. HW: B-2 |
| STY-003 | NOT_RUN (B-2) | HW video required. Logic only: `palm_never_draws_or_navigates`, `sty_004_cancel_and_palm_trace`, qa:S1 `palm_finger_ignored`, `stylus_with_resting_palm_commits_only_stylus` (SYNTHETIC) |
| STY-004 | PARTIAL | AUTO: `sty_004_action_cancel_rolls_back_and_flag_canceled`, qa:S1 `action_cancel_discards_stroke`. HW: B-2 |
| STY-005 | NOT_RUN (B-2) | HW only. Logic: `sty_005_pinch_anchor_stable_no_strokes`, qa:S1 pinch (zoom ×2.000, anchor drift 0, no paint) |
| STY-006 | NOT_RUN (B-2) | HW only. Logic: `sty_006_stylus_buttons_and_fallback` |
| STY-007 | PASS | `sty_007_predictions_never_confirmed`, `predicted_never_reaches_confirmed_even_when_interleaved`, `sty007_predicted_never_stored_and_preview_is_pure`, engine `predicted_samples_never_reach_document`, qa:S1 (164 predicted in, 0 stored, also after reopen) |
| STY-008 | NOT_RUN (B-2) | latency instrumentation |
| STY-009 | PARTIAL | AUTO: `lifecycle_reset_commits_pending_and_clears` (`input.reset`). HW surface/GPU: B-2 |

## Brush, raster, layers, history

| ID | Status | Evidence / blocker |
|---|---|---|
| PNT-001 | PASS | `pnt001_bit_identical_across_runs_and_batching` |
| PNT-002 | PARTIAL | AUTO goldens `pnt002_builtin_preset_goldens`; HW qualitative review: B-2 |
| PNT-003 | PASS | `pnt003_v1_fixture_migrates_preserving_id_and_defaults` |
| PNT-004 | PASS | `pnt004_eraser_leaves_no_dark_or_colored_fringe`; qa:S1 eraser |
| PNT-005 | PASS | `pnt005_partial_write_does_not_touch_unrelated_tiles` |
| PNT-006 | PASS | `veskra-compose` tests (groups, clipping, masks, alpha lock); qa:S2 clipping/alpha-lock checks |
| PNT-007 | PASS | `blend_modes_match_rational_reference_table`, `blend_modes_random_against_reference`; qa:S2 Multiply |
| PNT-008 | PASS | `pnt008_bounded_memory_and_caps` (raster unit test), qa:S2 `bucket_fill_bounded_by_lines` |
| PNT-009 | PASS | `history_exact_over_1000_operations`; qa:S2 undo ×46 / redo ×46 exact |
| PNT-010 | PARTIAL | `fit_error_bound_holds_on_fixtures`, `vector_outline_matches_raster_coverage`; qa:S4 silhouette Hausdorff 1.0 px. Bound not yet **signed** (VEC/ADR) |
| PNT-011 | PARTIAL | AUTO: `plan_matrix_all_combinations`, qa:S2 classification, qa:S5. ADOBE: B-3 |
| PNT-012 | NOT_RUN (B-2) | no GPU renderer implemented; CPU oracle exists (`veskra-compose`) |

## Color

| ID | Status | Evidence / blocker |
|---|---|---|
| COL-001 | PASS | `col001_iccp_carries_exact_document_profile`; qa:S3 ICC 1039; qa:S7 iCCP == sRGB bytes |
| COL-002 | PASS | `col002_srgb_8bit_identity_exhaustive`, `col002_opaque_swatches_exact`; qa:S7 0 LSB (png crate + Pillow) |
| COL-003 | PASS | `col003_swatches_in_common_lab` (median 0.0878, p95 0.3004); qa:S7 ΔE00 0 |
| COL-004 | PASS | `col004_untagged_assumes_srgb_or_rejects_by_policy`, `col004_corrupt_iccp_rejected_never_silently_ignored` |
| COL-005 | PASS | `col005_p3_convert_matches_littlecms_and_assign_differs`, engine `convert_and_assign_are_visibly_different_for_p3` (Android UI not verified, B-1) |
| COL-006 | PASS | `no_halos_on_semitransparent_edges`; qa:S7 alpha 0/3/64/128/255 exact |
| COL-007 | PARTIAL | only `sixteen_bit_gradient_keeps_precision` (import); no banding suite for compose/export gradients |
| COL-008 | PASS | `transforms_match_littlecms` (LittleCMS 2.19); run_all `color_oracle_regen` byte-identical |
| COL-009 | PARTIAL | AUTO proxy: psd-tools ICC + merged composite exact (qa:S3). Photoshop/Krita-color: B-3 |
| COL-010 | PARTIAL | AUTO: `ill005_profile_semantics_truthful`, pypdf ICCBased. Illustrator: B-3 |
| COL-011 | NOT_RUN (B-2, B-6) | device colorspace |
| COL-012 | PASS | `p3_document_embedded_or_converted` (converted + `ProfileNotEmbedded` warning) |

## Persistence and recovery

| ID | Status | Evidence / blocker |
|---|---|---|
| SAV-001 | PASS | `sav001_rich_document_roundtrip_is_exact_and_byte_identical`, `int01_stroke_save_reopen_identical`; qa:S1/S2 |
| SAV-002 | PARTIAL | AUTO: `saved_version_semantics`; HW (save while drawing on device): B-2; `:kit SaveStateMachineTest` not run here |
| SAV-003 | PARTIAL | AUTO: `sav003_sav005_failures_at_every_stage_keep_original_intact`, recovery tests, qa:S8, QAP-03. **Known defect QA-D1** (recover response says saved). HW force-stop: B-2 |
| SAV-004 | NOT_RUN (B-1, B-2) | SAF provider path is Kotlin/device |
| SAV-005 | PASS | format seam ENOSPC tests; qa:S8 `StorageFull`; QAP-03 real tmpfs ENOSPC (opt-in run). Known QA-D2 (permission errors typed `Io`) |
| SAV-006 | PASS | `malformed.rs` (truncation, corrupt manifest, oversized ICC, giant tiles); fuzz raw + structure-aware `.veskra`, 40 000 inputs, 0 panics |
| SAV-007 | PASS | `sav007_future_schema_refused_without_modification` |
| SAV-008 | NOT_RUN (B-2) | process eviction on device |
| SAV-009 | PASS | `sav009_repeated_cycles_do_not_drift` |

## PSD

| ID | Status | Evidence / blocker |
|---|---|---|
| PSD-001 | PASS | psd-tools on 5 fixtures (`psd_tools_and_imagemagick_oracle`) and qa:S3 |
| PSD-002 | PARTIAL | Krita 5.2.2 opens all 6 PSDs at correct size/alpha (run_all); layer semantics differ in Krita (known import limits). Photoshop: B-3 |
| PSD-003 | PASS | `six_layer_tree_flags_and_resources`, psd-tools `icc_ok`, qa:S3 own 1039 parse |
| PSD-004 | PARTIAL | AUTO merged composite exact (psd-tools/ImageMagick); ADOBE: B-3 |
| PSD-005 | PARTIAL | AUTO psd-tools recomposite (qa:S3 ≤ 2 LSB opaque) + warnings; ADOBE: B-3 |
| PSD-006 | PASS | `psd006_unrepresentable_semantics_are_warned_not_dropped`; qa:S3 preflight == report warnings |
| PSD-007 | PARTIAL | AUTO `psd007_cancellation_mid_export_and_no_file`, qa:S3 cancel; HW SAF: B-2 |

## Illustrator (SVG/PDF)

| ID | Status | Evidence / blocker |
|---|---|---|
| ILL-001 | NOT_RUN (B-3) | open-source proxy: qa:S4 (4/4 paths, Inkscape imports 4/4) |
| ILL-002 | NOT_RUN (B-3) | proxy: qa:S5 renders bit-exact |
| ILL-003 | NOT_RUN (B-3) | proxy: qa:S6 + FMT-B negative control; PDF OCG import risk (illustrator-export.md §4) |
| ILL-004 | PARTIAL | AUTO `ill004_both_unchecked_blocked_everywhere`; Android UI: B-1 |
| ILL-005 | PARTIAL | AUTO `ill005_profile_semantics_truthful`, qa:S7 iCCP; ADOBE: B-3 |
| ILL-006 | PARTIAL | AUTO `ill006_preflight_counts_before_export_match_report`; UI: B-1 |
| ILL-007 | NOT_RUN (B-3) | AUTO dims/viewBox/MediaBox pass (qa:S4–S6, svg_pdf_interop) |
| ILL-008 | PARTIAL | AUTO `ill008_no_ai_extension_or_mime`, `export_all_formats_and_reject_ai`; UI: B-1 |
| ILL-009 | NOT_RUN (B-3) | Illustrator Save As `.ai` |

## Performance, security, release

| ID | Status | Evidence / blocker |
|---|---|---|
| PERF-001…005 | NOT_RUN (B-2) | host context only: `docs/reports/performance-host.md` |
| SEC-001 | PARTIAL | fuzz smoke 180 000 inputs, 0 panics (`qa-fuzz.md`); `sec001_mutation_loop_no_panic` (import). Open: QA-D3 cost bound, SEC review, not coverage-guided |
| SEC-002 | PARTIAL | `cargo deny check licenses bans sources advisories`: ok (Rust crates). Android/Gradle deps and art assets not audited |
| SEC-003 | NOT_RUN (B-2) | manifest declares no INTERNET permission (static only) |
| SEC-004 | PARTIAL | manifest declares no storage permission (static inspection); SAF flow untested (B-1, B-2) |
| REL-001 | NOT_RUN (B-1, B-7) | Play policy check |
| REL-002 | PARTIAL | Rust workspace tests pass from this tree; APK build/sign: B-1 |
| REL-003 | NOT_RUN (B-2) | install/upgrade rehearsal |
| REL-004 | NOT_RUN (B-7) | beta sketcher |

**FAIL rows:** none at matrix level. Failing checks inside rows: QA-D1 (SAV-003), QA-D2 (SAV-005
error code) — see `docs/reports/qa-scenarios.md` §5.
