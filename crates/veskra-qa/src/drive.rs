//! JSON-command driver around [`Session`] (the FFI boundary Android uses).

use serde_json::{Value, json};
use std::path::Path;
use veskra_core::{LayerId, RectI};
use veskra_engine::{EngineConfig, InputOutcome, Session};
use veskra_input::RawPointerEvent;

/// Drives a deterministic session through `execute_json` and logs every command
/// (`commands.jsonl` artifacts can be replayed with `veskra run <file> --seed <seed>`).
#[derive(Debug)]
pub struct Driver {
    pub s: Session,
    pub seed: u64,
    log: Vec<String>,
    /// Successful commands that reported `historyChanged` (excluding create/open/undo/redo/recover).
    pub undo_steps: usize,
}

const NON_STEP: &[&str] =
    &["document.create", "document.open", "document.recover", "recovery.open", "history.undo", "history.redo"];

impl Driver {
    pub fn new(seed: u64) -> Self {
        Self {
            s: Session::new(EngineConfig { deterministic_seed: Some(seed), ..Default::default() }),
            seed,
            log: Vec::new(),
            undo_steps: 0,
        }
    }

    /// Execute; `Ok(full response)` or `Err(error object)`.
    pub fn try_cmd(&mut self, cmd: Value) -> Result<Value, Value> {
        let text = cmd.to_string();
        self.log.push(text.clone());
        let out: Value = serde_json::from_str(&self.s.execute_json(&text))
            .unwrap_or_else(|e| panic!("engine returned non-JSON for {text}: {e}"));
        if let Some(e) = out.get("error") {
            return Err(e.clone());
        }
        let name = cmd.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        if out["historyChanged"] == true && !NON_STEP.contains(&name) {
            self.undo_steps += 1;
        }
        Ok(out)
    }

    /// Execute and panic with the engine's error if it fails. Returns the `ok` payload.
    pub fn ok(&mut self, cmd: Value) -> Value {
        match self.try_cmd(cmd.clone()) {
            Ok(v) => v["ok"].clone(),
            Err(e) => panic!("command {cmd} failed: {e}"),
        }
    }

    /// Execute a command that must fail; returns the error object.
    pub fn err(&mut self, cmd: Value) -> Value {
        match self.try_cmd(cmd.clone()) {
            Ok(v) => panic!("command {cmd} unexpectedly succeeded: {v}"),
            Err(e) => e,
        }
    }

    pub fn info(&mut self) -> Value {
        self.ok(json!({"cmd": "document.info"}))
    }

    pub fn layers(&mut self) -> Vec<Value> {
        self.ok(json!({"cmd": "layer.list"}))["layers"].as_array().cloned().unwrap_or_default()
    }

    /// Id of the layer named `name` (first match, top-most first).
    pub fn layer_id(&mut self, name: &str) -> String {
        self.layers()
            .iter()
            .find(|l| l["name"] == name)
            .and_then(|l| l["id"].as_str().map(str::to_owned))
            .unwrap_or_else(|| panic!("no layer named {name}"))
    }

    pub fn add_layer(&mut self, kind: &str, name: &str, placement: Value) -> String {
        let v = self.ok(json!({"cmd": "layer.add", "kind": kind, "name": name, "placement": placement}));
        v["layerId"].as_str().map(str::to_owned).unwrap_or_else(|| panic!("layer.add returned {v}"))
    }

    pub fn select(&mut self, id: &str) {
        self.ok(json!({"cmd": "layer.select", "id": id}));
    }

    /// Headless stroke (`stroke.begin/samples/commit`), samples `(x, y, pressure)` 4 ms apart.
    pub fn stroke(&mut self, pts: &[(f32, f32, f32)]) {
        self.stroke_ex(pts, false);
    }

    pub fn stroke_ex(&mut self, pts: &[(f32, f32, f32)], eraser: bool) {
        self.ok(json!({"cmd": "stroke.begin", "eraser": eraser}));
        for chunk in pts.chunks(16).enumerate() {
            let samples: Vec<Value> = chunk
                .1
                .iter()
                .enumerate()
                .map(|(i, (x, y, p))| json!({"x": x, "y": y, "t": ((chunk.0 * 16 + i) as u64) * 4_000_000, "p": p}))
                .collect();
            self.ok(json!({"cmd": "stroke.samples", "samples": samples}));
        }
        let r = self.ok(json!({"cmd": "stroke.commit"}));
        assert_eq!(r["committed"], true, "stroke not committed: {r}");
    }

    /// Packed-event path: `pack_events` → `unpack_events` (JNI layout) → `Session::input_batch`,
    /// in batches of `batch` events. Returns the OR of all flags and every outcome.
    pub fn input(&mut self, events: &[RawPointerEvent], batch: usize) -> (i32, Vec<InputOutcome>) {
        let mut flags = 0;
        let mut outs = Vec::new();
        self.log.push(format!("# input_batch: {} packed events (not replayable via `veskra run`)", events.len()));
        for chunk in events.chunks(batch.max(1)) {
            let (i, f, l) = veskra_input::pack_events(chunk);
            let decoded = veskra_input::unpack_events(&i, &f, &l, chunk.len())
                .unwrap_or_else(|e| panic!("packed batch rejected: {e}"));
            assert_eq!(decoded.len(), chunk.len());
            let o = self.s.input_batch(&decoded);
            flags |= o.flags;
            outs.push(o);
        }
        (flags, outs)
    }

    pub fn doc(&self) -> std::sync::Arc<veskra_doc::Document> {
        self.s.snapshot().unwrap_or_else(|e| panic!("no document: {e}"))
    }

    /// Straight RGBA8 composite of the current (committed) document.
    pub fn composite(&self) -> Vec<u8> {
        crate::composite8(&self.doc())
    }

    /// What the display shows (premultiplied RGBA8, includes a live stroke).
    pub fn display(&self) -> Vec<u8> {
        let d = self.doc();
        let mut buf = vec![0u8; (d.width() * d.height() * 4) as usize];
        self.s.render_display(RectI::from_size(d.width(), d.height()), &mut buf).unwrap_or_else(|e| panic!("{e}"));
        buf
    }

    pub fn layer_px(&self, id: &str, x: i32, y: i32) -> [u8; 4] {
        let id = LayerId::parse(id).unwrap_or_else(|e| panic!("bad layer id {id}: {e}"));
        crate::layer_px(&self.doc(), id, x, y)
    }

    pub fn write_log(&self, path: &Path) {
        let mut s = format!("# veskra-qa command log, seed {}\n", self.seed);
        for l in &self.log {
            s.push_str(l);
            s.push('\n');
        }
        std::fs::write(path, s).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    }
}

/// Points on a straight line from `a` to `b` with pressure `p0 → p1`.
pub fn line(a: (f32, f32), b: (f32, f32), n: usize, p0: f32, p1: f32) -> Vec<(f32, f32, f32)> {
    let n = n.max(2);
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, p0 + (p1 - p0) * t)
        })
        .collect()
}

/// Closed ellipse polyline (the end overlaps the start by `overlap` of a turn).
pub fn ellipse(c: (f32, f32), r: (f32, f32), n: usize, overlap: f32, p: f32) -> Vec<(f32, f32, f32)> {
    let n = n.max(8);
    (0..=n)
        .map(|i| {
            let a = (i as f32 / n as f32) * std::f32::consts::TAU * (1.0 + overlap);
            (c.0 + r.0 * a.cos(), c.1 + r.1 * a.sin(), p)
        })
        .collect()
}

/// A wavy stroke across `[x0, x1]` around `y` with varying pressure.
pub fn wave(x0: f32, x1: f32, y: f32, amp: f32, n: usize) -> Vec<(f32, f32, f32)> {
    let n = n.max(2);
    (0..n)
        .map(|i| {
            let t = i as f32 / (n - 1) as f32;
            (x0 + (x1 - x0) * t, y + amp * (t * 9.0).sin(), 0.25 + 0.7 * (t * 3.0).sin().abs())
        })
        .collect()
}
