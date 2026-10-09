//! Headless artwork scenarios S1–S8 (MASTER_PLAN §2.4) driven through the engine's JSON command
//! boundary and the packed pointer-event path — the same API Android uses over JNI.
//!
//! Each scenario writes artifacts and `result.json` to `target/qa/<scenario>/` (see
//! `veskra_qa::report`). Hardware / Adobe sub-checks are recorded as `NOT_RUN` with their
//! `docs/BLOCKERS.md` id and are never reported as passed. Independent oracles (psd-tools,
//! Pillow, librsvg, poppler, qpdf) run when installed; otherwise the sub-check is `NOT_RUN`
//! with `TOOL:<name>`.
//!
//! Run: `cargo test -p veskra-qa --test scenarios -- --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use std::path::Path;
use veskra_core::export::{ExportFormat, ExportOptions};
use veskra_core::{ErrorCode, LayerId, SampleFlags, ToolType};
use veskra_doc::Document;
use veskra_input::PointerAction;
use veskra_qa::blockers::{NO_ADOBE, NO_DEVICE, NO_DISPLAY_INSTRUMENT};
use veskra_qa::drive::{Driver, ellipse, line, wave};
use veskra_qa::report::Report;
use veskra_qa::{RenderDiff, de00, events, int02, premul8, px_at, repo_root, tools};

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::write(path, veskra_testkit::encode_png_rgba8(w, h, rgba)).unwrap();
}

fn rgb(px: [u8; 4]) -> [u8; 3] {
    [px[0], px[1], px[2]]
}

fn temp_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".tmp-"))
                .collect()
        })
        .unwrap_or_default()
}

// =============================================================================================
// S1 Sketch
// =============================================================================================

#[test]
fn s1_sketch_pencil_erase_palm_two_finger() {
    let mut r = Report::new("S1", "Sketch: pencil rough, erase/recover, palm on glass, two-finger view change");
    let mut d = Driver::new(101);
    let (w, h) = (1024u32, 768u32);
    d.ok(json!({"cmd": "document.create", "width": w, "height": h, "background": "#FFFFFF"}));
    d.ok(json!({"cmd": "view.size", "width": w, "height": h}));
    d.ok(json!({"cmd": "view.set", "zoom": 1.0, "rotation": 0.0, "panX": 0.0, "panY": 0.0, "mirror": false}));
    d.ok(json!({"cmd": "brush.select", "preset": "pencil.rough"}));
    d.ok(json!({"cmd": "color.set", "hex": "#303040"}));
    let layer1 = d.info()["activeLayer"].as_str().unwrap().to_string();
    let blank = d.composite();

    // --- three stylus strokes through the packed JNI layout (historical + predicted events) ---
    let paths = [
        wave(100.0, 900.0, 200.0, 30.0, 120),
        wave(100.0, 900.0, 400.0, 30.0, 120),
        line((150.0, 600.0), (850.0, 650.0), 100, 0.2, 1.0),
    ];
    let mut t = 1_000_000_000u64;
    let mut trace = Vec::new();
    let (mut commits, mut predicted_in) = (0, 0);
    for pts in &paths {
        let ev = events::stylus_stroke(pts, t, 0, ToolType::Stylus, true, PointerAction::Down);
        predicted_in += ev.iter().filter(|e| e.predicted).count();
        t = ev.last().unwrap().t_ns + 50_000_000;
        let (fl, _) = d.input(&ev, 7);
        if fl & veskra_engine::flags::COMMITTED != 0 {
            commits += 1;
        }
        trace.extend(ev);
    }
    let info = d.info();
    r.check(
        "S1.stylus_strokes_committed",
        &["STY-001", "PNT-001"],
        commits == 3 && info["strokeCount"] == 3 && info["undoLabel"] == "Stroke",
        json!({"commitFlags": commits, "strokeCount": info["strokeCount"], "undoLabel": info["undoLabel"]}),
    );
    let comp = d.composite();
    let changed = comp.chunks_exact(4).zip(blank.chunks_exact(4)).filter(|(a, b)| a != b).count();
    r.check("S1.pencil_painted", &["PNT-002"], changed > 5_000, json!({"changedPixels": changed}));

    // STY-001/002/007 on the stored strokes.
    let confirmed: Vec<(f32, f32)> =
        trace.iter().filter(|e| !e.predicted && e.tool == ToolType::Stylus).map(|e| (e.x, e.y)).collect();
    let doc = d.doc();
    let (mut stored, mut not_confirmed_input, mut flagged_predicted, mut bad_axes, mut non_monotonic) = (0, 0, 0, 0, 0);
    for s in doc.iter_strokes() {
        let mut last_t = 0u64;
        for smp in s.samples() {
            stored += 1;
            if !smp.is_confirmed() || smp.flags.contains(SampleFlags::PREDICTED) {
                flagged_predicted += 1;
            }
            if !confirmed.iter().any(|(x, y)| (x - smp.x).abs() < 1e-3 && (y - smp.y).abs() < 1e-3) {
                not_confirmed_input += 1;
            }
            if !(0.0..=1.0).contains(&smp.pressure)
                || !smp.flags.contains(SampleFlags::PRESSURE_REPORTED)
                || smp.tilt.is_none()
            {
                bad_axes += 1;
            }
            if smp.t_ns < last_t {
                non_monotonic += 1;
            }
            last_t = smp.t_ns;
        }
    }
    r.check(
        "S1.predicted_events_never_persisted",
        &["STY-007"],
        stored > 0 && flagged_predicted == 0 && not_confirmed_input == 0,
        json!({"predictedEventsIn": predicted_in, "storedSamples": stored, "storedFlaggedPredicted": flagged_predicted,
               "storedNotAtAConfirmedInputPosition": not_confirmed_input}),
    );
    r.check(
        "S1.pressure_tilt_order",
        &["STY-001", "STY-002"],
        bad_axes == 0 && non_monotonic == 0,
        json!({"samplesWithBadPressureOrMissingReportedTilt": bad_axes, "nonMonotonicTimestamps": non_monotonic}),
    );

    // --- ACTION_CANCEL discards a pending stroke (STY-004) ---
    let ver = d.s.version();
    let mut ev = events::stylus_stroke(
        &line((300.0, 700.0), (700.0, 700.0), 30, 0.5, 0.5),
        t,
        0,
        ToolType::Stylus,
        false,
        PointerAction::Down,
    );
    if let Some(last) = ev.last_mut() {
        last.action = PointerAction::Cancel;
    }
    t = ev.last().unwrap().t_ns + 50_000_000;
    let (fl, _) = d.input(&ev, 8);
    r.check(
        "S1.action_cancel_discards_stroke",
        &["STY-004"],
        fl & veskra_engine::flags::COMMITTED == 0
            && d.s.version() == ver
            && d.composite() == comp
            && !d.s.has_live_stroke(),
        json!({"flags": fl, "versionUnchanged": d.s.version() == ver}),
    );
    trace.extend(ev);

    // --- erase / recover ---
    let before_erase = d.composite();
    // darkest pixel of stroke 2 near x = 500
    let mut probe = (500i32, 400i32);
    let mut darkest = 255u8;
    let mid = paths[1][60];
    for y in (mid.1 as i32 - 6)..=(mid.1 as i32 + 6) {
        for x in (mid.0 as i32 - 2)..=(mid.0 as i32 + 2) {
            let v = px_at(&before_erase, w, x as u32, y as u32)[0];
            if v < darkest {
                darkest = v;
                probe = (x, y);
            }
        }
    }
    let er = events::stylus_stroke(
        &line((probe.0 as f32 - 40.0, probe.1 as f32), (probe.0 as f32 + 40.0, probe.1 as f32), 30, 1.0, 1.0),
        t,
        0,
        ToolType::StylusEraser,
        false,
        PointerAction::Down,
    );
    t = er.last().unwrap().t_ns + 50_000_000;
    let (fl, _) = d.input(&er, 16);
    trace.extend(er);
    let after_erase = d.composite();
    let erased_px = px_at(&after_erase, w, probe.0 as u32, probe.1 as u32);
    let layer_alpha = d.layer_px(&layer1, probe.0, probe.1)[3];
    r.check(
        "S1.eraser_removes_pencil",
        &["PNT-004", "STY-001"],
        fl & veskra_engine::flags::COMMITTED != 0
            && darkest < 200
            && erased_px == [255, 255, 255, 255]
            && layer_alpha == 0,
        json!({"probe": [probe.0, probe.1], "beforeErase": px_at(&before_erase, w, probe.0 as u32, probe.1 as u32),
               "afterErase": erased_px, "layerAlphaAfterErase": layer_alpha,
               "eraserIsStylusEraserTool": true}),
    );
    d.ok(json!({"cmd": "history.undo"}));
    let undo1 = d.composite() == before_erase;
    d.ok(json!({"cmd": "history.redo"}));
    let redo1 = d.composite() == after_erase;
    d.ok(json!({"cmd": "history.undo"}));
    let recovered = d.composite() == before_erase;
    r.check(
        "S1.erase_undo_redo_recover",
        &["PNT-009"],
        undo1 && redo1 && recovered,
        json!({"undoRestoresExact": undo1, "redoReappliesExact": redo1, "finalRecovered": recovered}),
    );

    // --- two-finger pinch: view changes, nothing is painted (STY-005 logic) ---
    let v0 = d.s.view();
    let ver = d.s.version();
    let strokes_before = d.info()["strokeCount"].clone();
    let comp_before = d.composite();
    let pe = events::pinch((500.0, 300.0), 200.0, 400.0, 12, t);
    t = pe.last().unwrap().t_ns + 50_000_000;
    let (fl, _) = d.input(&pe, 4);
    trace.extend(pe);
    let v1 = d.s.view();
    let anchor = v1.view_to_doc().apply(veskra_core::PointF::new(500.0, 300.0));
    let a0 = v0.view_to_doc().apply(veskra_core::PointF::new(500.0, 300.0));
    let anchor_err = ((anchor.x - a0.x).powi(2) + (anchor.y - a0.y).powi(2)).sqrt();
    let zoom_ratio = v1.zoom / v0.zoom;
    r.check(
        "S1.pinch_changes_view_paints_nothing",
        &["STY-005"],
        fl & veskra_engine::flags::VIEW != 0
            && fl & veskra_engine::flags::COMMITTED == 0
            && d.s.version() == ver
            && d.info()["strokeCount"] == strokes_before
            && d.composite() == comp_before
            && (zoom_ratio - 2.0).abs() < 0.02
            && anchor_err < 0.5,
        json!({"flags": fl, "zoomRatio": zoom_ratio, "rotation": v1.rotation, "anchorDriftDocPx": anchor_err,
               "documentVersionUnchanged": d.s.version() == ver}),
    );

    // --- palm-sized finger alone: no paint, no navigation ---
    let ver = d.s.version();
    let v_before = d.s.view();
    let pe = events::palm(300.0, 500.0, 120.0, 180.0, t);
    t = pe.last().unwrap().t_ns + 50_000_000;
    let (fl, _) = d.input(&pe, 3);
    trace.extend(pe);
    r.check(
        "S1.palm_finger_ignored",
        &["STY-003"],
        fl & (veskra_engine::flags::COMMITTED | veskra_engine::flags::VIEW) == 0
            && d.s.version() == ver
            && d.s.view() == v_before,
        json!({"flags": fl, "touchMajorPx": 180, "palmThresholdPx": 100}),
    );

    // --- stylus stroke while a palm rests on the glass (zoomed view: doc mapping) ---
    let n_before = d.doc().stroke_count();
    let view_pts = line((420.0, 250.0), (580.0, 260.0), 40, 0.5, 0.9);
    let ev = events::stylus_with_resting_palm(&view_pts, (700.0, 600.0), t);
    let (fl, _) = d.input(&ev, 5);
    trace.extend(ev);
    let doc = d.doc();
    let expect0 = d.s.view().view_to_doc().apply(veskra_core::PointF::new(420.0, 250.0));
    let new_strokes: Vec<_> = doc
        .iter_strokes()
        .filter(|s| {
            s.samples()
                .first()
                .is_some_and(|f| (f.x as f64 - expect0.x).abs() < 1e-2 && (f.y as f64 - expect0.y).abs() < 1e-2)
        })
        .collect();
    r.check(
        "S1.stylus_with_resting_palm_commits_only_stylus",
        &["STY-003", "STY-005"],
        fl & veskra_engine::flags::COMMITTED != 0 && doc.stroke_count() == n_before + 1 && new_strokes.len() == 1,
        json!({"strokesAdded": doc.stroke_count() - n_before, "firstSampleDoc": [expect0.x, expect0.y],
               "matchedStrokes": new_strokes.len()}),
    );

    // --- save / reopen identical ---
    let path = r.artifact("s1.veskra");
    d.ok(json!({"cmd": "document.save", "path": p(&path)}));
    let mut d2 = Driver::new(101);
    d2.ok(json!({"cmd": "document.open", "path": p(&path)}));
    let reopened_doc_eq = *d2.doc() == *d.doc();
    let reopened_comp_eq = d2.composite() == d.composite();
    let no_pred = d2.doc().iter_strokes().all(|s| s.samples().iter().all(|x| x.is_confirmed()));
    r.check(
        "S1.save_reopen_identical",
        &["SAV-001", "STY-007", "G1"],
        reopened_doc_eq && reopened_comp_eq && no_pred,
        json!({"documentEqual": reopened_doc_eq, "compositeBitExact": reopened_comp_eq, "allSamplesConfirmed": no_pred}),
    );
    write_png(&r.artifact("s1_composite.png"), w, h, &d.composite());
    std::fs::write(r.artifact("s1_input_trace.jsonl"), veskra_input::trace::to_jsonl(&trace).unwrap()).unwrap();
    d.write_log(&r.artifact("commands.jsonl"));
    r.metric("inputEvents", json!(trace.len()));

    r.not_run(
        "S1.hw_palm_rejection",
        &["STY-003"],
        "HW",
        NO_DEVICE,
        "real palm contact shapes/timing need a pen tablet",
    );
    r.not_run("S1.hw_two_finger_gesture", &["STY-005"], "HW", NO_DEVICE, "real multi-touch digitizer behaviour");
    r.not_run(
        "S1.hw_latency",
        &["STY-008", "PERF-002"],
        "HW",
        NO_DEVICE,
        "event-to-photon latency needs device instrumentation",
    );
    r.not_run("S1.30min_sketch", &["G2", "QAP-01"], "HW", NO_DEVICE, "30-minute real drawing session");
    r.finish();
}

// =============================================================================================
// S2 Ink + color (INT-02 style 12-layer document)
// =============================================================================================

#[test]
fn s2_ink_color_12_layer_save_reopen_undo() {
    let mut r = Report::new(
        "S2",
        "Ink + color: lines/flats/shadow, selection fill, alpha lock, clipping, Multiply; INT-02 12 layers",
    );
    let seed = 202;
    let mut d = Driver::new(seed);
    let (w, h) = (1024u32, 768u32);
    let pr = int02::build(&mut d, w, h);
    let steps = d.undo_steps;
    let info = d.info();
    let layers = d.layers();
    let names: Vec<&str> = layers.iter().map(|l| l["name"].as_str().unwrap()).collect();
    r.check(
        "S2.twelve_layer_stack",
        &["INT-02", "PNT-006"],
        info["layerCount"] == 12 && names == pr.names_top_first && info["historyTrimmed"] == 0,
        json!({"layerCount": info["layerCount"], "names": names, "strokeCount": info["strokeCount"],
               "historyBytes": info["historyBytes"], "historyTrimmed": info["historyTrimmed"]}),
    );
    let get = |n: &str| layers.iter().find(|l| l["name"] == n).cloned().unwrap_or(Value::Null);
    let props_ok = get("Shadow")["clip"] == true
        && get("Shadow")["blend"] == "multiply"
        && get("Flats")["alphaLocked"] == true
        && get("Sketch")["visible"] == false
        && get("Hair shadow")["clip"] == true
        && get("Character")["kind"] == "group"
        && get("Highlights")["blend"] == "screen"
        && get("Texture")["blend"] == "overlay";
    r.check("S2.layer_properties", &["PNT-006", "PNT-007"], props_ok, json!({"layers": layers}));
    let fid_ok = get("Lines")["vectorFidelity"] == "exact"
        && get("Eyes")["vectorFidelity"] == "exact"
        && get("Flats")["vectorFidelity"] == "raster";
    r.check(
        "S2.hybrid_classification",
        &["G3", "PNT-011"],
        fid_ok,
        json!({"Lines": get("Lines")["vectorFidelity"], "Eyes": get("Eyes")["vectorFidelity"],
               "Flats": get("Flats")["vectorFidelity"], "Ink accents": get("Ink accents")["vectorFidelity"],
               "Texture": get("Texture")["vectorFidelity"]}),
    );

    let flats = d.layer_id("Flats");
    let shadow = d.layer_id("Shadow");
    let comp = d.composite();
    let at = |pt: (i32, i32)| px_at(&comp, w, pt.0 as u32, pt.1 as u32);
    let f_in = d.layer_px(&flats, pr.box_right_on_shadow.0, pr.box_right_on_shadow.1);
    let f_out = d.layer_px(&flats, pr.box_outside.0, pr.box_outside.1);
    let f_ell = d.layer_px(&flats, pr.ellipse_inside.0, pr.ellipse_inside.1);
    r.check(
        "S2.bucket_fill_bounded_by_lines",
        &["PNT-008"],
        rgb(f_in) == pr.skin && f_in[3] == 255 && f_out[3] == 0 && rgb(f_ell) == pr.blue && f_ell[3] == 255,
        json!({"insideBox": f_in, "outsideBox": f_out, "insideEllipse": f_ell}),
    );
    let lock_in = d.layer_px(&flats, pr.box_inside.0, pr.box_inside.1);
    let lock_out = d.layer_px(&flats, pr.outside_flats_on_shadow.0, pr.outside_flats_on_shadow.1);
    r.check(
        "S2.alpha_lock_fill",
        &["PNT-006"],
        rgb(lock_in) == pr.shade && lock_in[3] == 255 && lock_out[3] == 0 && rgb(at(pr.box_inside)) == pr.shade,
        json!({"lockedFillInsideFlat": lock_in, "lockedFillOutsideFlat(alpha must stay 0)": lock_out,
               "compositeInside": at(pr.box_inside)}),
    );
    let sh_out = d.layer_px(&shadow, pr.outside_flats_on_shadow.0, pr.outside_flats_on_shadow.1);
    let sh_in = d.layer_px(&shadow, pr.box_right_on_shadow.0, pr.box_right_on_shadow.1);
    let c_out = at(pr.outside_flats_on_shadow);
    let c_in = at(pr.box_right_on_shadow);
    r.check(
        "S2.clipped_multiply_shadow",
        &["PNT-006", "PNT-007"],
        sh_out[3] > 0 && rgb(c_out) == pr.paper && sh_in[3] > 0 && (0..3).all(|k| c_in[k] + 10 < pr.skin[k]),
        json!({"shadowPixelOutsideFlats": sh_out, "compositeThere(must be paper)": c_out,
               "shadowPixelOnFlat": sh_in, "compositeOnFlat(multiply darker than skin)": c_in, "skin": pr.skin}),
    );

    // --- undo everything, redo everything (atomic steps) ---
    let built = d.doc();
    let mut fresh = Driver::new(seed);
    fresh.ok(json!({"cmd": "document.create", "width": w, "height": h, "dpi": 300, "background": "#F4F1EA"}));
    let mut undone = 0;
    let last_err = loop {
        match d.try_cmd(json!({"cmd": "history.undo"})) {
            Ok(_) => undone += 1,
            Err(e) => break e,
        }
        if undone > steps + 5 {
            break json!("runaway");
        }
    };
    let at_start = *d.doc() == *fresh.doc() && d.composite() == fresh.composite();
    let mut redone = 0;
    while d.try_cmd(json!({"cmd": "history.redo"})).is_ok() {
        redone += 1;
        if redone > steps + 5 {
            break;
        }
    }
    let back = *d.doc() == *built && d.composite() == comp;
    r.check(
        "S2.undo_all_redo_all_atomic",
        &["PNT-009", "INT-02"],
        undone == steps && last_err["code"] == "NothingToUndo" && at_start && redone == steps && back,
        json!({"commandsRecordedAsSteps": steps, "undone": undone, "redone": redone,
               "afterUndoAllEqualsFreshDocument": at_start, "afterRedoAllEqualsBuilt": back, "lastUndoError": last_err["code"]}),
    );

    // --- save / reopen ---
    let path = r.artifact("int02.veskra");
    let saved = d.ok(json!({"cmd": "document.save", "path": p(&path)}));
    let dirty_after_save = d.info()["dirty"].clone();
    let mut d2 = Driver::new(seed);
    let opened = d2.ok(json!({"cmd": "document.open", "path": p(&path)}));
    let same_doc = *d2.doc() == *built;
    let same_comp = d2.composite() == comp;
    let l2 = d2.layers();
    let strip = |v: &[Value]| -> Vec<Value> {
        v.iter()
            .map(|l| {
                let mut l = l.clone();
                l.as_object_mut().map(|m| m.remove("active"));
                l
            })
            .collect()
    };
    let same_layers = strip(&l2) == strip(&layers);
    r.check(
        "S2.save_reopen_identical",
        &["SAV-001", "INT-02", "SAV-009"],
        same_doc && same_comp && same_layers && dirty_after_save == false && opened["dirty"] == false,
        json!({"bytes": saved["bytes"], "sha256": saved["sha256"], "documentEqual": same_doc,
               "compositeBitExact": same_comp, "layerListEqual": same_layers,
               "reopenedCanUndo": opened["canUndo"]}),
    );
    r.check(
        "S2.history_is_session_scoped",
        &["INT-02"],
        opened["canUndo"] == false && opened["canRedo"] == false,
        json!({"note": "undo history is not persisted in .veskra (by design); after reopen it starts empty",
               "canUndo": opened["canUndo"]}),
    );

    // --- edit after reopen: 3 undoable edits, exact undo back to the reopened state ---
    d2.select(&shadow);
    let reopened = d2.doc(); // after the (non-undoable) layer selection
    d2.ok(json!({"cmd": "brush.select", "preset": "round.soft"}));
    d2.ok(json!({"cmd": "color.set", "hex": "#5B4A7A"}));
    d2.stroke(&line((260.0, 300.0), (560.0, 320.0), 40, 0.8, 0.8));
    let ink = d2.layer_id("Ink accents");
    d2.ok(json!({"cmd": "layer.rename", "id": ink, "name": "Ink accents v2"}));
    let hi = d2.layer_id("Highlights");
    d2.ok(json!({"cmd": "layer.set", "id": hi, "opacity": 0.5}));
    let edited = d2.doc();
    let edited_comp = d2.composite();
    let labels: Vec<Value> = (0..3).map(|_| d2.ok(json!({"cmd": "history.undo"}))["undone"].clone()).collect();
    let back_to_reopened = *d2.doc() == *reopened && d2.composite() == comp;
    let extra = d2.err(json!({"cmd": "history.undo"}));
    let dirty_after_undo = d2.info()["dirty"].clone();
    for _ in 0..3 {
        d2.ok(json!({"cmd": "history.redo"}));
    }
    let redo_ok = *d2.doc() == *edited && d2.composite() == edited_comp;
    r.check(
        "S2.edit_after_reopen_undo_history",
        &["INT-02", "PNT-009"],
        back_to_reopened && extra["code"] == "NothingToUndo" && redo_ok && labels[2] == "Stroke",
        json!({"undoLabels": labels, "undoReturnsToReopenedExactly": back_to_reopened,
               "fourthUndo": extra["code"], "redoReappliesExactly": redo_ok,
               "dirtyAfterUndoingBackToSavedState": dirty_after_undo}),
    );
    let path2 = r.artifact("int02_edited.veskra");
    d2.ok(json!({"cmd": "document.save", "path": p(&path2)}));
    let mut d3 = Driver::new(seed);
    d3.ok(json!({"cmd": "document.open", "path": p(&path2)}));
    let ok3 = *d3.doc() == *edited && d3.composite() == edited_comp;
    r.check("S2.edited_save_reopen_identical", &["SAV-001", "INT-02"], ok3, json!({"equal": ok3}));

    write_png(&r.artifact("int02_composite.png"), w, h, &comp);
    std::fs::write(r.artifact("layers.json"), serde_json::to_vec_pretty(&layers).unwrap()).unwrap();
    d.write_log(&r.artifact("commands.jsonl"));
    r.not_run("S2.hw_coloring_session", &["G3"], "HW", NO_DEVICE, "real stylus coloring session on device");
    r.finish();
}

// =============================================================================================
// S3 PSD handoff
// =============================================================================================

fn has_base(doc: &Document, siblings: &[LayerId], pos: usize) -> bool {
    siblings[..pos].iter().any(|s| doc.layer(*s).is_some_and(|x| x.is_group() || !x.props.clip_to_below))
}

/// Expected PSD layer tree (bottom → top) in psd_check.py's schema.
fn expected_tree(doc: &Document, ids: &[LayerId]) -> Vec<Value> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            let l = doc.layer(*id).unwrap();
            let g = l.group();
            let blend = match g {
                Some(g) if g.pass_through => "PASS_THROUGH".to_string(),
                _ => format!("{:?}", l.props.blend).to_uppercase(),
            };
            json!({
                "name": l.props.name, "group": g.is_some(), "visible": l.props.visible,
                "opacity": (l.props.opacity * 255.0).round() as i64, "blend": blend,
                "clipping": l.props.clip_to_below && g.is_none() && has_base(doc, ids, i),
                "children": g.map(|g| expected_tree(doc, &g.children)).unwrap_or_default(),
            })
        })
        .collect()
}

/// Minimal independent read of the PSD header and image-resource section.
/// (version, height, width, depth, color mode, ICC resource 1039 bytes)
type PsdHeader = (u16, u32, u32, u16, u16, Option<Vec<u8>>);

fn psd_header_and_icc(b: &[u8]) -> Option<PsdHeader> {
    let be16 = |o: usize| b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
    let be32 = |o: usize| b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    if b.get(0..4)? != b"8BPS" {
        return None;
    }
    let (ver, ch, hgt, wid, depth, mode) = (be16(4)?, be16(12)?, be32(14)?, be32(18)?, be16(22)?, be16(24)?);
    let _ = ch;
    let mut o = 26usize;
    o = o.checked_add(4)?.checked_add(be32(o)? as usize)?;
    let res_len = be32(o)? as usize;
    let mut q = o + 4;
    let end = q.checked_add(res_len)?;
    let mut icc = None;
    while q + 12 <= end {
        if b.get(q..q + 4)? != b"8BIM" {
            return None;
        }
        let id = be16(q + 4)?;
        let nlen = *b.get(q + 6)? as usize;
        let mut r = q + 6 + 1 + nlen;
        if (1 + nlen) % 2 == 1 {
            r += 1;
        }
        let size = be32(r)? as usize;
        let data = b.get(r + 4..r + 4 + size)?;
        if id == 1039 {
            icc = Some(data.to_vec());
        }
        q = r + 4 + size + (size % 2);
    }
    Some((ver, hgt, wid, depth, mode, icc))
}

#[test]
fn s3_psd_handoff() {
    let mut r = Report::new("S3", "PSD handoff: 12-layer RGB/sRGB document with transparency");
    let mut d = Driver::new(303);
    let (w, h) = (1024u32, 768u32);
    int02::build(&mut d, w, h);
    let doc = d.doc();
    let pre = d.ok(json!({"cmd": "export.preflight", "format": "psd"}));
    let psd = r.artifact("int02.psd");
    let rep = d.ok(json!({"cmd": "export.run", "format": "psd", "path": p(&psd)}));
    r.check(
        "S3.export_report",
        &["PSD-001", "PSD-006"],
        rep["success"] == true
            && rep["validation"] == "structurally_validated"
            && rep["widthPx"] == w
            && rep["heightPx"] == h
            && rep["profileEmbedded"] == true
            && pre["blockedReason"].is_null()
            && pre["warnings"] == rep["warnings"],
        json!({"preflightWarnings": pre["warnings"], "reportWarnings": rep["warnings"], "bytes": rep["fileBytes"],
               "flattened": rep["flattenedFeatureList"], "durationMs": rep["durationMs"]}),
    );
    let bytes = std::fs::read(&psd).unwrap();
    let valid = veskra_io::exporter_for(ExportFormat::Psd).validate(&bytes).is_ok();
    let hdr = psd_header_and_icc(&bytes);
    let icc_ok = hdr.as_ref().and_then(|x| x.5.as_deref()) == Some(&doc.color.icc_bytes[..]);
    r.check(
        "S3.psd_header_and_icc_resource",
        &["PSD-001", "PSD-003", "COL-001"],
        valid
            && hdr.as_ref().is_some_and(|x| x.0 == 1 && x.1 == h && x.2 == w && x.3 == 8 && x.4 == 3)
            && icc_ok,
        json!({"veskraValidator": valid, "version/height/width/depth/mode": hdr.as_ref().map(|x| [x.0 as u32, x.1, x.2, x.3 as u32, x.4 as u32]),
               "icc1039EqualsDocumentProfile": icc_ok, "iccBytes": doc.color.icc_bytes.len()}),
    );

    // Expected files for psd_check.py / psd_krita_check.sh.
    let comp = veskra_qa::composite8(&doc);
    let raw = r.artifact("int02.composite.rgba");
    std::fs::write(&raw, &comp).unwrap();
    write_png(&r.artifact("int02.composite.png"), w, h, &comp);
    let exp = json!({
        "name": "int02", "width": w, "height": h, "composite_rgba": raw,
        "icc_hex": doc.color.icc_bytes.iter().map(|v| format!("{v:02x}")).collect::<String>(),
        "layers": expected_tree(&doc, doc.root()),
    });
    let jp = r.artifact("int02.expected.json");
    std::fs::write(&jp, serde_json::to_vec_pretty(&exp).unwrap()).unwrap();

    if tools::python_has("import psd_tools, numpy, PIL") {
        let script = repo_root().join("scripts/interop/psd_check.py");
        let out = tools::run("python3", &[&p(&script), &p(&psd), &p(&jp)]).unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        std::fs::write(r.artifact("psd_check.result.json"), &stdout).unwrap();
        let v: Value = serde_json::from_str(stdout.trim()).unwrap_or(json!({"parseError": stdout}));
        r.check_ev(
            "S3.psd_tools_layer_tree_icc",
            &["PSD-001", "PSD-003", "PSD-005"],
            "AUTO (psd-tools)",
            out.status.success() && v["layer_tree_ok"] == true && v["icc_ok"] == true,
            json!({"psdTools": v["psd_tools"], "errors": v["errors"], "size": v["size"]}),
        );
        let m = &v["merged_psd_tools"];
        let merged_ok =
            m["alpha_max"] == 0 && m["opaque_color_max"] == 0 && m["premul_max"].as_f64().is_some_and(|x| x <= 2.0);
        let im = &v["merged_imagemagick"];
        let im_ok = im.is_null()
            || (im["alpha_max"] == 0
                && im["opaque_color_max"] == 0
                && im["premul_max"].as_f64().is_some_and(|x| x <= 2.0));
        r.check_ev(
            "S3.merged_composite_matches",
            &["PSD-004"],
            "AUTO (psd-tools, ImageMagick)",
            merged_ok && im_ok,
            json!({"thresholds": "alpha 0, opaque color 0, premultiplied <= 2.0 (8-bit un-matting residual)",
                   "psdTools": m, "imageMagick": im}),
        );
        let rc = &v["recomposite_psd_tools"];
        // psd-tools recomposites the 8-bit layer channels itself (independent compositor) through
        // 12 layers incl. Multiply/Screen/Overlay, clipping and 90 % group opacity; Veskra composites
        // in 16 bit. QAP limit for this stack: alpha <= 1, opaque color <= 2, premultiplied <= 3.
        let rc_ok = rc["alpha_max"].as_i64().is_some_and(|x| x <= 1)
            && rc["opaque_color_max"].as_i64().is_some_and(|x| x <= 2)
            && rc["premul_max"].as_f64().is_some_and(|x| x <= 3.0);
        r.check_ev(
            "S3.psd_tools_recomposite_of_layers",
            &["PSD-004", "PSD-005"],
            "AUTO (psd-tools)",
            rc_ok,
            json!({"thresholds": "alpha <= 1, opaque color <= 2, premultiplied <= 3.0 (8-bit independent recomposite)", "metrics": rc}),
        );
    } else {
        r.not_run(
            "S3.psd_tools",
            &["PSD-001", "PSD-004"],
            "AUTO",
            "TOOL:psd_tools",
            "python3 + psd_tools not installed",
        );
    }

    // PSD-007: cancelled export leaves no file and no temp.
    let cancel = veskra_engine::CancelToken::new();
    cancel.cancel();
    let cpath = r.dir().join("cancelled.psd");
    let res = veskra_engine::export_snapshot(&doc, ExportFormat::Psd, &ExportOptions::default(), &cpath, &cancel);
    r.check(
        "S3.cancelled_export_leaves_nothing",
        &["PSD-007"],
        res.as_ref().is_err_and(|e| e.code == ErrorCode::Cancelled) && !cpath.exists() && temp_leftovers(r.dir()).is_empty(),
        json!({"error": res.err().map(|e| e.code.key()), "fileExists": cpath.exists(), "tempLeftovers": temp_leftovers(r.dir())}),
    );
    r.not_run(
        "S3.krita_render",
        &["PSD-002"],
        "manual Krita (automated)",
        "run_all.sh",
        "executed by scripts/interop/run_all.sh step krita_qa_s3 (see target/interop/summary.json)",
    );
    r.not_run("S3.photoshop_open", &["PSD-002", "PSD-004", "COL-009"], "ADOBE", NO_ADOBE, "no Photoshop available");
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}

// =============================================================================================
// S4 / S5 / S6 Illustrator handoff (SVG/PDF) - open-source renderers only
// =============================================================================================

fn svg_paths(svg: &str) -> (usize, usize) {
    match svg.find("display=\"none\"") {
        Some(i) => (svg[..i].matches("<path ").count(), svg[i..].matches("<path ").count()),
        None => (svg.matches("<path ").count(), 0),
    }
}

fn pdf_media_box(b: &[u8]) -> Option<Vec<f64>> {
    let s = String::from_utf8_lossy(b);
    let i = s.find("/MediaBox")?;
    let rest = &s[i..];
    let a = rest.find('[')?;
    let z = rest.find(']')?;
    rest[a + 1..z].split_whitespace().map(|t| t.parse().ok()).collect()
}

struct Exported {
    report: Value,
    preflight: Value,
    path: std::path::PathBuf,
}

fn export(d: &mut Driver, r: &mut Report, fmt: &str, name: &str, opts: Value) -> Exported {
    let path = r.artifact(name);
    let preflight = d.ok(json!({"cmd": "export.preflight", "format": fmt, "options": opts}));
    let report = d.ok(json!({"cmd": "export.run", "format": fmt, "path": p(&path), "options": opts}));
    Exported { report, preflight, path }
}

/// Render the SVG and PDF of one export pair and compare with the reference composite.
/// Silhouette agreement of two premultiplied images: IoU of `alpha >= 128` masks and the largest
/// distance (px, searched up to 8) from a pixel in one mask to the nearest pixel of the other.
fn silhouette(a: &[u8], b: &[u8], w: u32, h: u32) -> (f64, f64) {
    let ma: Vec<bool> = a.chunks_exact(4).map(|p| p[3] >= 128).collect();
    let mb: Vec<bool> = b.chunks_exact(4).map(|p| p[3] >= 128).collect();
    let (mut inter, mut uni, mut worst) = (0usize, 0usize, 0f64);
    let (wi, hi) = (w as i32, h as i32);
    for y in 0..hi {
        for x in 0..wi {
            let i = (y * wi + x) as usize;
            inter += (ma[i] && mb[i]) as usize;
            uni += (ma[i] || mb[i]) as usize;
            if ma[i] == mb[i] {
                continue;
            }
            let other = if ma[i] { &mb } else { &ma };
            let mut best = 9.0f64;
            for dy in -8i32..=8 {
                for dx in -8i32..=8 {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < wi && yy < hi && other[(yy * wi + xx) as usize] {
                        best = best.min(((dx * dx + dy * dy) as f64).sqrt());
                    }
                }
            }
            worst = worst.max(best);
        }
    }
    (if uni == 0 { 1.0 } else { inter as f64 / uni as f64 }, worst)
}

/// Limits for a render comparison.
#[derive(Clone, Copy)]
enum Limit {
    /// Bit-exact (embedded composite is what is visible).
    Exact,
    /// Vector outlines visible: silhouette IoU >= min, Hausdorff <= px, mean <= m, % > 16 <= pct.
    Silhouette { iou: f64, hausdorff_px: f64, mean: f64, pct_over_16: f64 },
}

#[allow(clippy::too_many_arguments)]
fn render_both(
    r: &mut Report,
    id: &str,
    matrix: &[&str],
    svg: &Path,
    pdf: &Path,
    doc: &Document,
    limit: Limit,
    probes: &[(u32, u32)],
) {
    let (w, h) = (doc.width(), doc.height());
    let comp = veskra_qa::composite8(doc);
    // `<fixture>-reference.png` is the naming scripts/interop/svg_pdf_inkscape_check.py expects.
    let fixture =
        svg.file_name().map(|n| n.to_string_lossy().split('-').next().unwrap_or("x").to_string()).unwrap_or_default();
    write_png(&r.artifact(&format!("{fixture}-reference.png")), w, h, &comp);
    let reference = premul8(&comp);
    let ref_probe: Vec<u8> = probes.iter().map(|&(x, y)| px_at(&reference, w, x, y)[3]).collect();
    for (kind, tool, res) in [
        ("svg", "rsvg-convert", tools::have("rsvg-convert").then(|| tools::rsvg(svg, w, h))),
        ("pdf", "pdftocairo", tools::have("pdftocairo").then(|| tools::pdftocairo(pdf, doc.dpi, w, h))),
    ] {
        let cid = format!("{id}.{kind}_render_vs_composite");
        match res {
            None => r.not_run(&cid, matrix, "AUTO", &format!("TOOL:{tool}"), "renderer not installed"),
            Some(Err(e)) => {
                r.check(&cid, matrix, false, json!({"error": e}));
            }
            Some(Ok(px)) => {
                let got = premul8(&px);
                let df = RenderDiff::of(&got, &reference).unwrap_or_default();
                let probe: Vec<u8> = probes.iter().map(|&(x, y)| px_at(&got, w, x, y)[3]).collect();
                let probe_ok = probe.iter().zip(&ref_probe).all(|(a, b)| a.abs_diff(*b) <= 8);
                let (iou, haus) = silhouette(&got, &reference, w, h);
                let (ok, lim) = match limit {
                    Limit::Exact => (df.max == 0, json!("bit-exact")),
                    Limit::Silhouette { iou: mi, hausdorff_px, mean, pct_over_16 } => (
                        iou >= mi && haus <= hausdorff_px && df.mean <= mean && df.pct_over_16 <= pct_over_16,
                        json!({"minIoU": mi, "maxHausdorffPx": hausdorff_px, "maxMean": mean, "maxPctOver16": pct_over_16}),
                    ),
                };
                r.check_ev(
                    &cid,
                    matrix,
                    &format!("AUTO ({tool})"),
                    ok && probe_ok,
                    json!({"diff": df.json(), "silhouetteIoU": (iou * 1e4).round() / 1e4, "silhouetteHausdorffPx": haus,
                           "limits": lim, "probeAlpha": probe, "referenceProbeAlpha": ref_probe,
                           "note": "probe = alpha inside a 50% stroke; a doubled stroke reads ~192 instead of ~128"}),
                );
            }
        }
    }
    if tools::have("qpdf") {
        let ok = tools::qpdf_check(pdf).unwrap_or(false);
        r.check_ev(&format!("{id}.qpdf_check"), &["ILL-008"], "AUTO (qpdf)", ok, json!({"qpdfNoErrors": ok}));
    } else {
        r.not_run(&format!("{id}.qpdf_check"), &["ILL-008"], "AUTO", "TOOL:qpdf", "qpdf not installed");
    }
    let mb = pdf_media_box(&std::fs::read(pdf).unwrap());
    let want = [w as f64 * 72.0 / doc.dpi as f64, h as f64 * 72.0 / doc.dpi as f64];
    let mb_ok =
        mb.as_ref().is_some_and(|m| m.len() == 4 && (m[2] - want[0]).abs() < 0.01 && (m[3] - want[1]).abs() < 0.01);
    r.check(&format!("{id}.pdf_mediabox"), &["ILL-007"], mb_ok, json!({"mediaBoxPt": mb, "expectedPt": want}));
}

/// QAP limit for renders where vector outlines are visible: the outline is a fitted silhouette
/// of the brush raster, so pixels differ along edges; what must hold is the silhouette (≤ 2 px)
/// and no doubling (probe). PNT-010's signed geometric bound is still pending (VEC).
const VECTOR_LIMIT: Limit = Limit::Silhouette { iou: 0.9, hausdorff_px: 2.0, mean: 0.2, pct_over_16: 1.0 };

/// Ink-only artwork on a transparent canvas: vector layer with pressure-tapered pen strokes
/// (one at 50 % opacity for the doubling probe).
fn ink_doc(d: &mut Driver) -> (u32, u32, (u32, u32), usize) {
    let (w, h) = (800u32, 600u32);
    d.ok(json!({"cmd": "document.create", "width": w, "height": h, "dpi": 150}));
    d.add_layer("vector", "Ink", json!({"at": "top"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.brush"}));
    d.ok(json!({"cmd": "color.set", "hex": "#1B2A6B"}));
    d.stroke(&wave(80.0, 720.0, 150.0, 40.0, 160));
    d.stroke(&ellipse((250.0, 380.0), (110.0, 80.0), 120, 0.05, 0.7));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.technical"}));
    d.ok(json!({"cmd": "brush.set", "size": 18.0, "opacity": 0.5}));
    d.ok(json!({"cmd": "color.set", "hex": "#B03020"}));
    d.stroke(&line((480.0, 480.0), (740.0, 480.0), 40, 1.0, 1.0));
    d.ok(json!({"cmd": "brush.set", "opacity": 1.0, "size": 6.0}));
    d.stroke(&line((480.0, 300.0), (700.0, 380.0), 40, 0.2, 1.0));
    (w, h, (610, 480), 4)
}

#[test]
fn s4_illustrator_vector_handoff() {
    let mut r = Report::new("S4", "Illustrator vector handoff (SVG/PDF, editable vector only) - open-source renderers");
    let mut d = Driver::new(404);
    let (_w, _h, probe, strokes) = ink_doc(&mut d);
    let doc = d.doc();
    let opts = json!({"includeVectorPaths": true, "includeRasterAppearance": false, "embedColorProfile": true});
    let svg = export(&mut d, &mut r, "svg", "ink-vector-only.svg", opts.clone());
    let pdf = export(&mut d, &mut r, "pdf", "ink-vector-only.pdf", opts);
    let text = std::fs::read_to_string(&svg.path).unwrap();
    let (vis, hid) = svg_paths(&text);
    r.check(
        "S4.svg_editable_paths",
        &["ILL-001", "ILL-006", "PNT-010"],
        vis == strokes
            && hid == 0
            && !text.contains("<image")
            && svg.preflight["vectorPathCount"] == strokes
            && svg.report["vectorObjectCount"] == strokes
            && svg.report["rasterizedObjectCount"] == 0,
        json!({"visiblePaths": vis, "hiddenPaths": hid, "images": text.matches("<image").count(),
               "preflight": {"vectorPathCount": svg.preflight["vectorPathCount"], "rasterOnly": svg.preflight["rasterOnlyStrokeCount"]},
               "report": {"vectorObjectCount": svg.report["vectorObjectCount"], "rasterized": svg.report["rasterizedObjectCount"],
                          "warnings": svg.report["warnings"]}}),
    );
    r.check(
        "S4.pdf_report",
        &["ILL-001", "ILL-008"],
        pdf.report["success"] == true
            && pdf.report["vectorObjectCount"] == strokes
            && pdf.report["rasterizedObjectCount"] == 0,
        json!({"report": pdf.report}),
    );
    // FMT-B measured 27 / 0.0167 / 0.0142 % on comparable ink fixtures (AA edge differences).
    let fit: f64 = doc.iter_strokes().filter_map(|s| s.geometry.as_ref().map(|g| g.fit_error)).fold(0.0, f64::max);
    r.metric("maxStrokeFitErrorPx", json!(fit));
    render_both(&mut r, "S4", &["ILL-001", "PNT-010"], &svg.path, &pdf.path, &doc, VECTOR_LIMIT, &[probe]);
    r.not_run(
        "S4.illustrator_select_recolor",
        &["ILL-001", "ILL-007", "ILL-009"],
        "ADOBE",
        NO_ADOBE,
        "Illustrator selection/recolor/Save As .ai needs a human with Illustrator",
    );
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}

/// Textured painting on a transparent canvas (+ one vector stroke that must be omitted).
fn paint_doc(d: &mut Driver) -> (u32, u32) {
    let (w, h) = (800u32, 600u32);
    d.ok(json!({"cmd": "document.create", "width": w, "height": h, "dpi": 300}));
    d.ok(json!({"cmd": "brush.select", "preset": "charcoal"}));
    d.ok(json!({"cmd": "color.set", "hex": "#5A3E2B"}));
    d.stroke(&wave(60.0, 740.0, 140.0, 50.0, 200));
    d.ok(json!({"cmd": "brush.select", "preset": "round.soft"}));
    d.ok(json!({"cmd": "brush.set", "size": 70.0}));
    d.ok(json!({"cmd": "color.set", "hex": "#3C8DBC"}));
    d.stroke(&wave(100.0, 700.0, 330.0, 60.0, 120));
    let wash = d.add_layer("raster", "Wash", json!({"at": "top"}));
    d.ok(json!({"cmd": "layer.set", "id": wash, "opacity": 0.6}));
    d.ok(json!({"cmd": "selection.ellipse", "x0": 420.0, "y0": 360.0, "x1": 760.0, "y1": 580.0}));
    d.ok(json!({"cmd": "color.set", "hex": "#E0A02080"}));
    d.ok(json!({"cmd": "edit.fill_selection"}));
    d.ok(json!({"cmd": "selection.clear"}));
    d.add_layer("vector", "Ink", json!({"at": "top"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.technical"}));
    d.ok(json!({"cmd": "color.set", "hex": "#101010"}));
    d.stroke(&line((80.0, 520.0), (380.0, 540.0), 30, 0.5, 0.9));
    (w, h)
}

#[test]
fn s5_illustrator_appearance_handoff() {
    let mut r =
        Report::new("S5", "Illustrator appearance handoff (SVG/PDF, preserve appearance only) - open-source renderers");
    let mut d = Driver::new(505);
    let (w, h) = paint_doc(&mut d);
    let doc = d.doc();
    let opts = json!({"includeVectorPaths": false, "includeRasterAppearance": true, "embedColorProfile": true});
    let svg = export(&mut d, &mut r, "svg", "paint-appearance-only.svg", opts.clone());
    let pdf = export(&mut d, &mut r, "pdf", "paint-appearance-only.pdf", opts);
    let text = std::fs::read_to_string(&svg.path).unwrap();
    let kinds: Vec<Value> =
        svg.report["warnings"].as_array().cloned().unwrap_or_default().iter().map(|x| x["kind"].clone()).collect();
    r.check(
        "S5.svg_structure",
        &["ILL-002", "ILL-006", "ILL-007"],
        text.contains("<image")
            && text.matches("<path ").count() == 0
            && text.contains(&format!("viewBox=\"0 0 {w} {h}\""))
            && kinds.contains(&json!("vector_omitted"))
            && svg.report["profileEmbedded"] == true,
        json!({"images": text.matches("<image").count(), "paths": text.matches("<path ").count(),
               "warningKinds": kinds, "profile": svg.report["profile"]}),
    );
    r.check(
        "S5.pdf_report",
        &["ILL-002", "ILL-005"],
        pdf.report["success"] == true && pdf.report["profileEmbedded"] == true,
        json!({"report": pdf.report, "preflightWarnings": pdf.preflight["warnings"]}),
    );
    // Appearance-only: renders must be bit-exact (FMT-B measured 0/0/0); corners transparent.
    render_both(&mut r, "S5", &["ILL-002", "PNT-011"], &svg.path, &pdf.path, &doc, Limit::Exact, &[(5, 5), (790, 590)]);
    r.not_run(
        "S5.illustrator_appearance",
        &["ILL-002", "COL-010"],
        "ADOBE",
        NO_ADOBE,
        "Illustrator position/scale/alpha/color check",
    );
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}

#[test]
fn s6_hybrid_no_double_rendering() {
    let mut r = Report::new("S6", "Hybrid SVG/PDF (vector + appearance): no doubled artwork - open-source renderers");
    let mut d = Driver::new(606);
    let (_w, _h, probe, strokes) = ink_doc(&mut d);
    // add a textured raster layer under the ink
    let ink = d.layer_id("Ink");
    d.add_layer("raster", "Texture", json!({"at": "below", "id": ink}));
    d.ok(json!({"cmd": "brush.select", "preset": "charcoal"}));
    d.ok(json!({"cmd": "color.set", "hex": "#7A9A5A"}));
    d.stroke(&wave(60.0, 420.0, 520.0, 30.0, 120));
    let doc = d.doc();
    let opts = json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true});
    let svg = export(&mut d, &mut r, "svg", "hybrid-both.svg", opts.clone());
    let pdf = export(&mut d, &mut r, "pdf", "hybrid-both.pdf", opts);
    let text = std::fs::read_to_string(&svg.path).unwrap();
    let (vis, hid) = svg_paths(&text);
    r.check(
        "S6.svg_default_strategy_hidden_editable_vectors",
        &["ILL-003", "ILL-006"],
        vis == 0 && hid == strokes && text.contains("<image") && svg.report["vectorObjectCount"] == strokes,
        json!({"visiblePaths": vis, "hiddenPaths(display=none)": hid, "vectorObjectCount": svg.report["vectorObjectCount"],
               "rasterOnlyStrokes(preflight)": svg.preflight["rasterOnlyStrokeCount"], "warnings": svg.report["warnings"]}),
    );
    r.check(
        "S6.pdf_report",
        &["ILL-003"],
        pdf.report["success"] == true && pdf.report["vectorObjectCount"] == strokes,
        json!({"report": pdf.report}),
    );
    // SVG default = exact composite visible; PDF default (ADR-012 provisional) = visible vectors
    // + raster residual: AA-edge differences only. One limit covering both.
    render_both(&mut r, "S6", &["ILL-003"], &svg.path, &pdf.path, &doc, VECTOR_LIMIT, &[probe]);
    r.not_run(
        "S6.illustrator_hide_raster_edit_vectors",
        &["ILL-003", "ILL-009"],
        "ADOBE",
        NO_ADOBE,
        "Illustrator: initial display without doubling, hide raster, edit vectors",
    );
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}

// =============================================================================================
// S7 Color sanity
// =============================================================================================

struct Swatch {
    id: String,
    rgb: [u8; 3],
    lab: [f64; 3],
}

fn swatches() -> Vec<Swatch> {
    let text = std::fs::read_to_string(repo_root().join("tests/fixtures/color/srgb_swatches.csv")).unwrap();
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split(',').collect();
            Swatch {
                id: c[0].to_string(),
                rgb: [c[2].parse().unwrap(), c[3].parse().unwrap(), c[4].parse().unwrap()],
                lab: [c[5].parse().unwrap(), c[6].parse().unwrap(), c[7].parse().unwrap()],
            }
        })
        .collect()
}

fn decode_with_meta(bytes: &[u8]) -> (u32, u32, Vec<u8>, Option<Vec<u8>>, bool) {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut rd = dec.read_info().unwrap();
    let icc = rd.info().icc_profile.as_ref().map(|c| c.to_vec());
    let srgb = rd.info().srgb.is_some();
    let mut buf = vec![0u8; rd.output_buffer_size().unwrap()];
    let fi = rd.next_frame(&mut buf).unwrap();
    buf.truncate(fi.buffer_size());
    assert_eq!(fi.color_type, png::ColorType::Rgba, "Veskra PNG export is RGBA8");
    (fi.width, fi.height, buf, icc, srgb)
}

fn stats(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.total_cmp(b));
    let q = |p: f64| if v.is_empty() { f64::NAN } else { v[((v.len() as f64 - 1.0) * p).round() as usize] };
    json!({"n": v.len(), "median": q(0.5), "p95": q(0.95), "max": q(1.0)})
}

#[test]
fn s7_color_sanity_known_swatches() {
    let mut r = Report::new("S7", "Color sanity: known sRGB swatches -> fill -> PNG -> independent decoders");
    let sw = swatches();
    let cell = 16u32;
    let cols = 16u32;
    let rows = (sw.len() as u32).div_ceil(cols);
    let (w, h) = (cols * cell, (rows + 1) * cell);
    let mut d = Driver::new(707);
    d.ok(json!({"cmd": "document.create", "width": w, "height": h, "dpi": 72}));
    let mut centers = Vec::new();
    for (i, s) in sw.iter().enumerate() {
        let (cx, cy) = ((i as u32 % cols) * cell, (i as u32 / cols) * cell);
        d.ok(json!({"cmd": "selection.rect", "x0": cx, "y0": cy, "x1": cx + cell, "y1": cy + cell}));
        d.ok(json!({"cmd": "color.set", "hex": format!("#{:02X}{:02X}{:02X}", s.rgb[0], s.rgb[1], s.rgb[2])}));
        d.ok(json!({"cmd": "edit.fill_selection"}));
        centers.push((cx + cell / 2, cy + cell / 2));
    }
    // Alpha row on its own layer over transparency (COL-006): 0, ~0.01, 0.25, 0.5, 1.
    d.ok(json!({"cmd": "selection.clear"}));
    let alpha_layer = d.add_layer("raster", "Alpha", json!({"at": "top"}));
    let alphas: [(u8, &str); 5] =
        [(0, "#C0402000"), (3, "#C0402003"), (64, "#2060C040"), (128, "#2060C080"), (255, "#2060C0FF")];
    let mut a_centers = Vec::new();
    for (k, (_, hex)) in alphas.iter().enumerate() {
        let (cx, cy) = (k as u32 * cell, rows * cell);
        d.ok(json!({"cmd": "selection.rect", "x0": cx, "y0": cy, "x1": cx + cell, "y1": cy + cell}));
        d.ok(json!({"cmd": "color.set", "hex": hex}));
        if !hex.ends_with("00") {
            d.ok(json!({"cmd": "edit.fill_selection"}));
        }
        a_centers.push((cx + cell / 2, cy + cell / 2));
    }
    d.ok(json!({"cmd": "selection.clear"}));
    let _ = alpha_layer;
    let info = d.info();
    r.check(
        "S7.document_profile",
        &["COL-001"],
        info["profile"].as_str().is_some_and(|s| s.contains("sRGB")) && info["profileAssumed"] == false,
        json!({"profile": info["profile"], "assumed": info["profileAssumed"], "blendSpace": info["blendSpace"]}),
    );

    // Engine-side pick (composite sampling).
    let mut pick_bad = 0;
    for (s, &(x, y)) in sw.iter().zip(&centers) {
        let want = format!("#{:02X}{:02X}{:02X}", s.rgb[0], s.rgb[1], s.rgb[2]);
        if d.ok(json!({"cmd": "color.pick", "x": x as f32 + 0.5, "y": y as f32 + 0.5}))["picked"] != want.as_str() {
            pick_bad += 1;
        }
    }
    r.check(
        "S7.engine_pick_matches",
        &["COL-002"],
        pick_bad == 0,
        json!({"mismatches": pick_bad, "swatches": sw.len()}),
    );

    let srgb_icc = veskra_color::IccProfile::srgb();
    let mut pts: Vec<(u32, u32)> = centers.clone();
    pts.extend(a_centers.iter().copied());
    std::fs::write(
        r.artifact("probe_points.json"),
        serde_json::to_vec(&pts.iter().map(|p| [p.0, p.1]).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    for (variant, embed) in [("icc", true), ("noicc", false)] {
        let path = r.artifact(&format!("swatches-{variant}.png"));
        let rep = d.ok(json!({"cmd": "export.run", "format": "png", "path": p(&path),
                              "options": {"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": embed}}));
        let bytes = std::fs::read(&path).unwrap();
        let (pw, ph, px, icc, srgb_chunk) = decode_with_meta(&bytes);
        let icc_ok =
            if embed { icc.as_deref() == Some(srgb_icc.bytes()) && !srgb_chunk } else { icc.is_none() && srgb_chunk };
        r.check_ev(
            &format!("S7.{variant}.png_profile_tagging"),
            &["COL-001", "ILL-005"],
            "AUTO (png crate)",
            icc_ok && (pw, ph) == (w, h) && rep["profileEmbedded"] == embed,
            json!({"iccBytes": icc.as_ref().map(|v| v.len()), "iccEqualsVeskraSrgb": icc.as_deref() == Some(srgb_icc.bytes()),
                   "srgbChunk": srgb_chunk, "reportProfile": rep["profile"]}),
        );
        let mut max_lsb = 0u8;
        let mut de_src = Vec::new();
        let mut de_lab = Vec::new();
        let mut worst = (String::new(), 0.0f64);
        for (s, &(x, y)) in sw.iter().zip(&centers) {
            let got = px_at(&px, w, x, y);
            max_lsb = max_lsb.max((0..3).map(|k| got[k].abs_diff(s.rgb[k])).max().unwrap_or(0)).max(255 - got[3]);
            de_src.push(de00(rgb(got), s.rgb));
            let e = veskra_color::lab::ciede2000(veskra_color::lab::srgb8_to_lab_d50(rgb(got)), s.lab);
            if e >= worst.1 {
                worst = (s.id.clone(), e);
            }
            de_lab.push(e);
        }
        let (de_src_s, de_lab_s) = (stats(de_src), stats(de_lab));
        r.check_ev(
            &format!("S7.{variant}.png_crate_swatches"),
            &["COL-002", "COL-003"],
            "AUTO+COLOR (png crate; Lab reference = colour-science values in srgb_swatches.csv)",
            max_lsb == 0 && de_lab_s["p95"].as_f64().is_some_and(|x| x <= 2.0) && de_lab_s["median"].as_f64().is_some_and(|x| x <= 1.0),
            json!({"swatches": sw.len(), "maxChannelLsb": max_lsb, "deltaE00_vs_source_rgb": de_src_s,
                   "deltaE00_vs_colour_science_lab": de_lab_s, "worstSwatch": worst.0, "thresholds": "COL-002 <= 1 LSB (expect 0); COL-003 median <= 1.0, p95 <= 2.0"}),
        );
        let a_got: Vec<[u8; 4]> = a_centers.iter().map(|&(x, y)| px_at(&px, w, x, y)).collect();
        let a_ok = a_got[0] == [0, 0, 0, 0]
            && a_got[1] == [0xC0, 0x40, 0x20, 3]
            && a_got[2] == [0x20, 0x60, 0xC0, 64]
            && a_got[3] == [0x20, 0x60, 0xC0, 128]
            && a_got[4] == [0x20, 0x60, 0xC0, 255];
        r.check_ev(
            &format!("S7.{variant}.alpha_swatches"),
            &["COL-006"],
            "AUTO (png crate)",
            a_ok,
            json!({"decoded": a_got}),
        );

        if tools::python_has("import PIL") {
            let script = repo_root().join("scripts/qa/png_probe.py");
            let out =
                tools::run("python3", &["-I", &p(&script), &p(&path), &p(&r.dir().join("probe_points.json"))]).unwrap();
            let v: Value =
                serde_json::from_slice(&out.stdout).unwrap_or(json!({"error": String::from_utf8_lossy(&out.stderr)}));
            std::fs::write(r.artifact(&format!("pillow-{variant}.json")), serde_json::to_vec_pretty(&v).unwrap())
                .unwrap();
            let pix = v["pixels"].as_array().cloned().unwrap_or_default();
            let mut max_lsb = 0u8;
            let mut des = Vec::new();
            for (s, pv) in sw.iter().zip(&pix) {
                let g: Vec<u8> = pv
                    .as_array()
                    .map(|a| a.iter().map(|x| x.as_u64().unwrap_or(999).min(255) as u8).collect())
                    .unwrap_or_default();
                if g.len() == 4 {
                    max_lsb = max_lsb.max((0..3).map(|k| g[k].abs_diff(s.rgb[k])).max().unwrap_or(0)).max(255 - g[3]);
                    des.push(de00([g[0], g[1], g[2]], s.rgb));
                } else {
                    max_lsb = 255;
                }
            }
            let icc_len_ok = if embed { v["icc_len"] == srgb_icc.bytes().len() } else { v["icc_len"] == 0 };
            let ds = stats(des);
            r.check_ev(
                &format!("S7.{variant}.pillow_swatches"),
                &["COL-002", "COL-003", "COL-001"],
                "AUTO+COLOR (Pillow)",
                out.status.success() && pix.len() == pts.len() && max_lsb == 0 && icc_len_ok,
                json!({"pillow": v["pillow"], "mode": v["mode"], "iccLen": v["icc_len"], "srgbIntent": v["srgb_intent"],
                       "maxChannelLsb": max_lsb, "deltaE00": ds}),
            );
        } else {
            r.not_run(
                &format!("S7.{variant}.pillow_swatches"),
                &["COL-002"],
                "AUTO",
                "TOOL:Pillow",
                "python3 Pillow not installed",
            );
        }
    }
    r.not_run(
        "S7.photoshop_illustrator_view",
        &["COL-009", "COL-010"],
        "ADOBE+COLOR",
        NO_ADOBE,
        "Photoshop/Illustrator under controlled conditions",
    );
    r.not_run("S7.physical_display", &["COL-011"], "HW+COLOR", NO_DISPLAY_INSTRUMENT, "colorimeter + device display");
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}

// =============================================================================================
// S8 Recovery
// =============================================================================================

mod faulty {
    use std::fs::File;
    use std::io::{self, Write};
    use std::path::Path;
    use veskra_format::SaveSink;

    /// Temp sink that accepts `limit` bytes, then fails every write with ENOSPC.
    pub struct Enospc {
        f: File,
        written: u64,
        limit: u64,
    }
    impl Write for Enospc {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let room = self.limit.saturating_sub(self.written);
            if room == 0 {
                return Err(io::Error::from_raw_os_error(28));
            }
            let n = (buf.len() as u64).min(room) as usize;
            self.f.write_all(&buf[..n])?;
            self.written += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.f.flush()
        }
    }
    impl SaveSink for Enospc {
        fn sync_all(&mut self) -> io::Result<()> {
            self.f.sync_all()
        }
    }
    pub fn opener(limit: u64) -> impl FnOnce(&Path) -> io::Result<Box<dyn SaveSink>> {
        move |p: &Path| {
            let f = std::fs::OpenOptions::new().write(true).create_new(true).open(p)?;
            Ok(Box::new(Enospc { f, written: 0, limit }) as Box<dyn SaveSink>)
        }
    }
}

#[test]
fn s8_recovery_checkpoints_crash_storage_full() {
    let mut r = Report::new("S8", "Recovery: autosave checkpoints, simulated crash, storage full");
    let mut d = Driver::new(808);
    let (w, h) = (512u32, 384u32);
    d.ok(json!({"cmd": "document.create", "width": w, "height": h, "background": "#FFFFFF"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.brush"}));
    let auto = r.dir().join("autosave");
    let original = r.artifact("original.veskra");
    let mut comps = Vec::new();
    for i in 0..4 {
        let hex = ["#AA2020", "#20AA20", "#2020AA", "#202020"][i];
        d.ok(json!({"cmd": "color.set", "hex": hex}));
        d.stroke(&wave(40.0, 470.0, 60.0 + 80.0 * i as f32, 20.0, 80));
        if i == 1 {
            d.ok(json!({"cmd": "document.save", "path": p(&original)}));
        }
        let cp = d.ok(json!({"cmd": "document.autosave", "dir": p(&auto)}));
        comps.push((
            cp["seq"].as_u64().unwrap_or(0),
            cp["path"].as_str().unwrap_or("").to_string(),
            d.composite(),
            d.doc(),
        ));
    }
    let orig_bytes = std::fs::read(&original).unwrap();
    let list =
        d.ok(json!({"cmd": "recovery.list", "dir": p(&auto)}))["checkpoints"].as_array().cloned().unwrap_or_default();
    let seqs: Vec<u64> = list.iter().filter(|c| c["valid"] == true).filter_map(|c| c["seq"].as_u64()).collect();
    r.check(
        "S8.checkpoints_bounded_keep_3",
        &["SAV-003"],
        seqs == vec![2, 3, 4],
        json!({"validSeqs": seqs, "listed": list.len()}),
    );

    // Simulated crash while the newest checkpoint was being replaced: truncate it.
    let newest = comps[3].1.clone();
    let nb = std::fs::read(&newest).unwrap();
    std::fs::write(&newest, &nb[..nb.len() / 2]).unwrap();
    // ... and an interrupted temp write left behind by a killed process.
    std::fs::write(
        auto.join(format!("{}.tmp-deadbeef", Path::new(&newest).file_name().unwrap().to_string_lossy())),
        &nb[..100],
    )
    .unwrap();
    let list =
        d.ok(json!({"cmd": "recovery.list", "dir": p(&auto)}))["checkpoints"].as_array().cloned().unwrap_or_default();
    let newest_entry = list.iter().find(|c| c["seq"] == 4).cloned().unwrap_or(Value::Null);
    r.check(
        "S8.truncated_checkpoint_detected",
        &["SAV-003", "SAV-006"],
        newest_entry["valid"] == false && newest_entry["error"].is_string() && list.len() == 3,
        json!({"newest": newest_entry, "listedIgnoringTemp": list.len()}),
    );

    let mut rec = Driver::new(809);
    let opened = rec.ok(json!({"cmd": "document.recover", "dir": p(&auto)}));
    let after = rec.info();
    let ok_prev = rec.composite() == comps[2].2 && *rec.doc() == *comps[2].3;
    r.check(
        "S8.recover_picks_previous_valid",
        &["SAV-003", "G4"],
        ok_prev && after["dirty"] == true && after["savedVersion"].is_null(),
        json!({"recoveredEqualsSeq3": ok_prev, "dirty(document.info)": after["dirty"],
               "savedVersion(document.info)": after["savedVersion"],
               "note": "a recovered checkpoint is never marked as the user's saved file"}),
    );
    r.check_known(
        "S8.recover_receipt_reports_unsaved",
        &["SAV-003"],
        "QA-D1",
        opened["dirty"] == after["dirty"] && opened["savedVersion"] == after["savedVersion"],
        json!({"receiptDirty": opened["dirty"], "receiptSavedVersion": opened["savedVersion"],
               "infoDirty": after["dirty"], "infoSavedVersion": after["savedVersion"],
               "note": "the document.recover response must describe the state the session is actually in"}),
    );
    let orig_now = std::fs::read(&original).unwrap();
    let orig_valid = d.try_cmd(json!({"cmd": "document.validate", "path": p(&original)})).is_ok();
    r.check(
        "S8.original_untouched",
        &["SAV-003", "SAV-005"],
        orig_now == orig_bytes && orig_valid,
        json!({"bytesUnchanged": orig_now == orig_bytes, "validates": orig_valid}),
    );
    // Continue autosaving from the recovered session: sequence never reuses a name.
    let cp = rec.ok(json!({"cmd": "document.autosave", "dir": p(&auto)}));
    r.check("S8.autosave_after_recovery", &["SAV-003"], cp["seq"] == 5, json!({"seq": cp["seq"]}));

    // Storage full through the format crate's public fault seam (ENOSPC mid-write).
    let store = veskra_format::AutosaveStore::new(auto.clone(), 3).unwrap();
    let before: Vec<(u64, bool)> = store.list().unwrap().iter().map(|c| (c.seq, c.valid)).collect();
    let meta = veskra_format::SaveMeta::new("veskra-qa", "unspecified");
    let snap = rec.doc();
    let res = store.checkpoint_with(&snap, &meta, faulty::opener(4096));
    let after: Vec<(u64, bool)> = store.list().unwrap().iter().map(|c| (c.seq, c.valid)).collect();
    let temps: Vec<String> = temp_leftovers(&auto).into_iter().filter(|n| !n.ends_with("deadbeef")).collect();
    r.check(
        "S8.storage_full_checkpoint",
        &["SAV-005"],
        matches!(res, Err(veskra_format::FormatError::StorageFull)) && before == after && temps.is_empty(),
        json!({"error": res.as_ref().err().map(|e| e.to_string()), "checkpointsBefore": before, "checkpointsAfter": after,
               "newTempLeftovers": temps}),
    );
    let res = veskra_format::save_atomic_with(&snap, &original, &meta, None, faulty::opener(10_000));
    let still = std::fs::read(&original).unwrap() == orig_bytes;
    r.check(
        "S8.storage_full_user_save",
        &["SAV-005"],
        res.as_ref().is_err_and(|e| e.code() == ErrorCode::StorageFull) && still && temp_leftovers(r.dir()).is_empty(),
        json!({"error": res.as_ref().err().map(|e| e.to_string()), "originalUnchanged": still}),
    );
    let removed = veskra_format::remove_stale_temps(&auto.join(Path::new(&newest).file_name().unwrap())).unwrap_or(0);
    r.check("S8.stale_temp_cleanup", &["SAV-003"], removed == 1, json!({"removed": removed}));

    r.not_run("S8.device_force_stop", &["SAV-003", "SAV-008"], "HW", NO_DEVICE, "process kill / OS eviction on device");
    r.not_run(
        "S8.saf_provider_failure",
        &["SAV-004"],
        "HW",
        NO_DEVICE,
        "SAF provider write failure needs Android (also B-1)",
    );
    r.not_run("S8.rotation_sleep_gpu_loss", &["STY-009", "SAV-008"], "HW", NO_DEVICE, "surface lifecycle on device");
    d.write_log(&r.artifact("commands.jsonl"));
    r.finish();
}
