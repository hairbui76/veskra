//! QAP-08 / SEC-001 deterministic mutation fuzz smoke test.
//!
//! Seeds: `tests/fuzz-corpus/<target>/` (provenance in its README). Mutations (testkit `Rng`,
//! fixed seed): bit flips, interesting-byte overwrites, truncation, splices from other seeds,
//! chunk duplication and length-field inflation (u16/u32 big- and little-endian), stacked 1–4×.
//!
//! Targets: `IccProfile::parse` (+ transform construction), `veskra_format::{decode, inspect,
//! read_preview}` on raw mutations and on structure-aware mutations (entries re-zipped with a
//! recomputed `integrity.json`, so mutated JSON/tiles/strokes/ICC reach the decoder; decoded
//! documents are then composited and re-encoded), `inspect_psd(keep_pixels)` + PSD/PNG/SVG/PDF exporter `validate`,
//! `veskra_input::unpack_events` (+ `Session::input_batch` on the decoded events) and
//! `Session::execute_json` (byte- and JSON-structure-level command mutations; file paths are
//! rewritten into a sandbox directory and `document.create` is clamped to 1024² by the harness).
//!
//! Asserted per input: no panic (caught and recorded with the input saved under
//! `target/qa/fuzz/crashes/`), wall time ≤ `VESKRA_FUZZ_MAX_MS` (default 5000 ms) and peak heap
//! growth ≤ `VESKRA_FUZZ_MAX_HEAP_MB` (default 512 MiB), measured by a counting global
//! allocator (this binary only). Iterations per target: `VESKRA_FUZZ_ITERS` (default 1000).
//! Results: `target/qa/fuzz/result.json`.
//!
//! Regenerate the seed corpus (deterministic): `VESKRA_REGEN_FUZZ_CORPUS=1 cargo test -p veskra-qa --test fuzz_smoke corpus`.
#![allow(clippy::unwrap_used, clippy::expect_used)]
// The counting allocator below is the only unsafe code in this test binary (reviewer: QAP + ORCH).
#![allow(unsafe_code)]

use serde_json::{Value, json};
use std::alloc::{GlobalAlloc, Layout, System};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use veskra_core::ToolType;
use veskra_core::export::ExportFormat;
use veskra_engine::{EngineConfig, Session};
use veskra_input::PointerAction;
use veskra_qa::drive::{Driver, ellipse, line, wave};
use veskra_qa::{events, qa_root, repo_root};
use veskra_testkit::Rng;

// ---------------------------------------------------------------------------- counting allocator

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);

fn grew(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed).saturating_add(size);
    PEAK.fetch_max(live, Relaxed);
    LARGEST.fetch_max(size, Relaxed);
}

// SAFETY (all four methods): every call is forwarded unchanged to `System`, which implements the
// `GlobalAlloc` contract; the wrapper only updates atomic counters and never touches the memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        // SAFETY: same layout the caller passed, which satisfies `alloc`'s preconditions.
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grew(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        // SAFETY: as above.
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            grew(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: `p`/`l` come from a previous `System` allocation made through this wrapper.
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `p`/`l` come from this wrapper; `new_size` is forwarded unchanged.
        let q = unsafe { System.realloc(p, l, new_size) };
        if !q.is_null() {
            LIVE.fetch_sub(l.size(), Relaxed);
            grew(new_size);
        }
        q
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

// ---------------------------------------------------------------------------- configuration

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn corpus_dir() -> PathBuf {
    repo_root().join("tests/fuzz-corpus")
}

fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(corpus_dir().join(target))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap_or_default()))
                .filter(|(n, _)| !n.ends_with(".md"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

// ---------------------------------------------------------------------------- mutators

const INTERESTING_U32: [u32; 10] =
    [0, 1, 0x7F, 0xFF, 0xFFFF, 0x0001_0000, 0x00FF_FFFF, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF];

fn mutate(rng: &mut Rng, data: &mut Vec<u8>, others: &[(String, Vec<u8>)]) -> Vec<String> {
    let mut ops = Vec::new();
    for _ in 0..(1 + rng.below(4)) {
        let len = data.len();
        match rng.below(7) {
            0 if len > 0 => {
                for _ in 0..(1 + rng.below(8)) {
                    let i = rng.below(len as u64) as usize;
                    data[i] ^= 1 << rng.below(8);
                }
                ops.push("bitflip".into());
            }
            1 if len > 0 => {
                let i = rng.below(len as u64) as usize;
                data[i] = [0u8, 0xFF, 0x7F, 0x80, 0x01, b'{', b'"', b'0'][rng.below(8) as usize];
                ops.push("byte".into());
            }
            2 if len > 0 => {
                let n = rng.below(len as u64) as usize;
                data.truncate(n);
                ops.push(format!("truncate@{n}"));
            }
            3 if !others.is_empty() => {
                let src = &others[rng.below(others.len() as u64) as usize].1;
                if !src.is_empty() {
                    let a = rng.below(src.len() as u64) as usize;
                    let b = (a + 1 + rng.below(256) as usize).min(src.len());
                    let at = rng.below(len as u64 + 1) as usize;
                    if rng.below(2) == 0 {
                        data.splice(at..at, src[a..b].iter().copied());
                    } else {
                        let end = (at + (b - a)).min(len);
                        data.splice(at..end, src[a..b].iter().copied());
                    }
                    ops.push("splice".into());
                }
            }
            4 if len > 1 => {
                let a = rng.below(len as u64) as usize;
                let b = (a + 1 + rng.below(64) as usize).min(len);
                let chunk = data[a..b].to_vec();
                data.splice(a..a, chunk);
                ops.push("dup".into());
            }
            5 | 6 if len >= 4 => {
                let i = rng.below(len as u64 - 3) as usize;
                let v = INTERESTING_U32[rng.below(INTERESTING_U32.len() as u64) as usize];
                let v =
                    if rng.below(4) == 0 { (len as u32).wrapping_mul(2).wrapping_add(rng.below(16) as u32) } else { v };
                match rng.below(4) {
                    0 => data[i..i + 4].copy_from_slice(&v.to_be_bytes()),
                    1 => data[i..i + 4].copy_from_slice(&v.to_le_bytes()),
                    2 => data[i..i + 2].copy_from_slice(&(v as u16).to_be_bytes()),
                    _ => data[i..i + 2].copy_from_slice(&(v as u16).to_le_bytes()),
                }
                ops.push(format!("inflate@{i}={v:#x}"));
            }
            _ => {}
        }
    }
    ops
}

// ---------------------------------------------------------------------------- structure-aware .veskra

/// Mutate any JSON value in place: replace a random node with an interesting value, delete an
/// object key, or grow/shrink an array.
fn mutate_json(rng: &mut Rng, v: &mut Value) {
    fn count(v: &Value) -> usize {
        1 + match v {
            Value::Array(a) => a.iter().map(count).sum(),
            Value::Object(o) => o.values().map(count).sum(),
            _ => 0,
        }
    }
    fn nth<'a>(v: &'a mut Value, n: &mut usize) -> Option<&'a mut Value> {
        if *n == 0 {
            return Some(v);
        }
        *n -= 1;
        match v {
            Value::Array(a) => a.iter_mut().find_map(|x| nth(x, n)),
            Value::Object(o) => o.values_mut().find_map(|x| nth(x, n)),
            _ => None,
        }
    }
    let total = count(v);
    let mut k = rng.below(total as u64) as usize;
    let Some(node) = nth(v, &mut k) else { return };
    match (rng.below(4), &mut *node) {
        (0, Value::Object(o)) if !o.is_empty() => {
            let key = o.keys().nth(rng.below(o.len() as u64) as usize).cloned().unwrap_or_default();
            o.remove(&key);
        }
        (0, Value::Array(a)) if !a.is_empty() => {
            let i = rng.below(a.len() as u64) as usize;
            let dup = a[i].clone();
            for _ in 0..(1 + rng.below(64)) {
                a.push(dup.clone());
            }
        }
        (1, Value::Array(a)) if !a.is_empty() => {
            a.truncate(rng.below(a.len() as u64) as usize);
        }
        _ => {
            *node = match rng.below(14) {
                0 => json!(0),
                1 => json!(-1),
                2 => json!(65),
                3 => json!(16384),
                4 => json!(4_294_967_296u64),
                5 => json!(1e308),
                6 => json!(-0.5),
                7 => json!(""),
                8 => json!("x".repeat(300)),
                9 => json!(null),
                10 => json!(true),
                11 => json!([]),
                12 => json!({}),
                _ => json!(rng.range(-70000.0, 70000.0).round()),
            };
        }
    }
}

/// Unzip a `.veskra`, mutate one entry (JSON structurally, binary entries bytewise), re-zip with
/// a recomputed `integrity.json` so the mutation reaches the decoder past the hash check.
fn mutate_bundle(rng: &mut Rng, seed: &[u8], others: &[(String, Vec<u8>)]) -> Option<(Vec<u8>, Vec<String>)> {
    use sha2::{Digest, Sha256};
    use veskra_format::zip::{Method, ZipReader, ZipWriter};
    let zr = ZipReader::parse(seed).ok()?;
    let mut entries: Vec<(String, Vec<u8>, Method)> = Vec::new();
    for e in zr.entries() {
        if e.name == "integrity.json" {
            continue;
        }
        entries.push((e.name.clone(), zr.read(e).ok()?, e.method));
    }
    if entries.is_empty() {
        return None;
    }
    let mut ops = Vec::new();
    for _ in 0..(1 + rng.below(2)) {
        let i = rng.below(entries.len() as u64) as usize;
        let (name, data, _) = &mut entries[i];
        if name.ends_with(".json") {
            if let Ok(mut v) = serde_json::from_slice::<Value>(data) {
                mutate_json(rng, &mut v);
                *data = serde_json::to_vec(&v).ok()?;
                ops.push(format!("json:{name}"));
            }
        } else {
            let o = mutate(rng, data, others);
            ops.push(format!("bytes:{name}:{}", o.join("+")));
        }
    }
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let mut integ: Vec<Value> = entries
        .iter()
        .map(|(n, d, _)| json!({"name": n, "size": d.len(), "sha256": hex(&Sha256::digest(d))}))
        .collect();
    integ.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    let mut zw = ZipWriter::new(Vec::new());
    for (n, d, m) in &entries {
        zw.add(n, d, *m).ok()?;
    }
    zw.add(
        "integrity.json",
        &serde_json::to_vec(&json!({"algorithm": "sha256", "entries": integ})).ok()?,
        Method::Deflated,
    )
    .ok()?;
    let (out, _) = zw.finish().ok()?;
    Some((out, ops))
}

// ---------------------------------------------------------------------------- input blobs

/// `VQIN` | count u32 | n_ints u32 | n_floats u32 | n_longs u32 | ints i32 LE | floats f32 LE | longs i64 LE.
/// Decoding is lenient: arrays hold only the bytes actually present (no allocation from header values).
fn encode_input_blob(events: &[veskra_input::RawPointerEvent]) -> Vec<u8> {
    let (i, f, l) = veskra_input::pack_events(events);
    let mut out = b"VQIN".to_vec();
    for n in [events.len(), i.len(), f.len(), l.len()] {
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    i.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    f.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    l.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
    out
}

fn decode_input_blob(b: &[u8]) -> (Vec<i32>, Vec<f32>, Vec<i64>, usize) {
    let u = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize).unwrap_or(0);
    let (count, ni, nf, nl) = (u(4), u(8), u(12), u(16));
    let mut o = 20usize.min(b.len());
    let mut take = |n: usize, sz: usize| -> &[u8] {
        let avail = (b.len() - o) / sz;
        let n = n.min(avail);
        let s = &b[o..o + n * sz];
        o += n * sz;
        s
    };
    let ints = take(ni, 4).chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let floats = take(nf, 4).chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let longs = take(nl, 8).chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap_or([0; 8]))).collect();
    (ints, floats, longs, count)
}

// ---------------------------------------------------------------------------- command sanitizer

fn sandbox() -> PathBuf {
    let d = qa_root().join("fuzz/sandbox");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Keep the harness bounded and inside its sandbox: rewrite `path`/`dir`, clamp document size.
fn sanitize(text: &str, sb: &Path) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(text) else { return text.to_string() };
    if let Some(m) = v.as_object_mut() {
        if let Some(pv) = m.get("path").cloned() {
            let ext = pv
                .as_str()
                .and_then(|s| s.rsplit('.').next())
                .filter(|e| ["veskra", "png", "psd", "svg", "pdf", "ai"].contains(e))
                .unwrap_or("veskra")
                .to_string();
            m.insert("path".into(), json!(sb.join(format!("f.{ext}")).to_string_lossy()));
        }
        if m.contains_key("dir") {
            m.insert("dir".into(), json!(sb.join("autosave").to_string_lossy()));
        }
        if m.get("cmd").and_then(|c| c.as_str()) == Some("document.create") {
            for k in ["width", "height"] {
                if let Some(n) = m.get(k).and_then(|x| x.as_f64())
                    && n > 1024.0
                {
                    m.insert(k.into(), json!(1024));
                }
            }
        }
    }
    v.to_string()
}

const COMMANDS: &[&str] = &[
    "document.create",
    "document.info",
    "document.save",
    "document.open",
    "document.validate",
    "document.autosave",
    "document.recover",
    "recovery.list",
    "recovery.open",
    "layer.add",
    "layer.remove",
    "layer.duplicate",
    "layer.move",
    "layer.set",
    "layer.rename",
    "layer.select",
    "layer.clear",
    "layer.list",
    "brush.list",
    "brush.select",
    "brush.set",
    "color.set",
    "color.get",
    "color.pick",
    "stroke.begin",
    "stroke.samples",
    "stroke.commit",
    "stroke.cancel",
    "history.undo",
    "history.redo",
    "selection.rect",
    "selection.ellipse",
    "selection.lasso",
    "selection.all",
    "selection.clear",
    "selection.invert",
    "edit.fill",
    "edit.fill_selection",
    "edit.delete_selection",
    "edit.transform",
    "view.size",
    "view.fit",
    "view.set",
    "view.mirror",
    "input.config",
    "input.reset",
    "export.preflight",
    "export.run",
];

fn interesting_value(rng: &mut Rng, layer_ids: &[String]) -> Value {
    match rng.below(16) {
        0 => json!(0),
        1 => json!(-1),
        2 => json!(1e308),
        3 => json!(-1e308),
        4 => json!(4_294_967_297u64),
        5 => json!(""),
        6 => json!("x".repeat(4096)),
        7 => json!(null),
        8 => json!(true),
        9 => Value::Array((0..rng.below(2000)).map(|i| json!([i as f64 * 0.37, (i * 7 % 300) as f64])).collect()),
        10 => {
            let at = ["top", "aboveActive", "into", "above", "below", "nowhere"][rng.below(6) as usize];
            json!({"at": at, "id": layer_ids.first().cloned().unwrap_or_default(),
                   "group": layer_ids.last().cloned().unwrap_or_default()})
        }
        11 => json!(layer_ids.get(rng.below(layer_ids.len().max(1) as u64) as usize).cloned().unwrap_or_default()),
        12 => json!(rng.range(-5000.0, 5000.0)),
        13 => json!("00000000-0000-0000-0000-000000000000"),
        14 => Value::Array(
            (0..rng.below(3000))
                .map(|i| json!({"x": rng.range(-100.0, 300.0), "y": rng.range(-100.0, 300.0), "t": i * 1000, "p": rng.range(-1.0, 2.0)}))
                .collect(),
        ),
        _ => json!(rng.f64()),
    }
}

fn mutate_command(rng: &mut Rng, seed_line: &str, layer_ids: &[String]) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(seed_line) else { return seed_line.to_string() };
    if let Some(m) = v.as_object_mut() {
        for _ in 0..(1 + rng.below(3)) {
            match rng.below(4) {
                0 => {
                    m.insert("cmd".into(), json!(COMMANDS[rng.below(COMMANDS.len() as u64) as usize]));
                }
                1 => {
                    let keys: Vec<String> = m.keys().filter(|k| *k != "cmd").cloned().collect();
                    if let Some(k) = keys.get(rng.below(keys.len().max(1) as u64) as usize) {
                        let nv = interesting_value(rng, layer_ids);
                        m.insert(k.clone(), nv);
                    }
                }
                2 => {
                    const KEYS: [&str; 18] = [
                        "x",
                        "y",
                        "id",
                        "width",
                        "height",
                        "samples",
                        "matrix",
                        "points",
                        "options",
                        "preset",
                        "hex",
                        "kind",
                        "placement",
                        "opacity",
                        "blend",
                        "zoom",
                        "format",
                        "eraser",
                    ];
                    let k = KEYS[rng.below(18) as usize];
                    let nv = interesting_value(rng, layer_ids);
                    m.insert(k.into(), nv);
                }
                _ => {
                    let keys: Vec<String> = m.keys().cloned().collect();
                    if let Some(k) = keys.get(rng.below(keys.len().max(1) as u64) as usize) {
                        m.remove(k);
                    }
                }
            }
        }
    }
    v.to_string()
}

// ---------------------------------------------------------------------------- the run

static PANIC_MSG: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Default)]
struct TargetStats {
    iterations: usize,
    ok: usize,
    rejected: usize,
    panics: Vec<Value>,
    max_ms: f64,
    max_peak_heap: usize,
    max_single_alloc: usize,
    slow: Vec<Value>,
    heavy: Vec<Value>,
    /// (iteration, ms) of the slowest input and (iteration, bytes) of the heaviest.
    worst_ms: (usize, f64),
    worst_heap: (usize, usize),
}

struct Ctx {
    max_ms: f64,
    max_heap: usize,
    crash_dir: PathBuf,
}

/// Run one input; `f` returns `true` if the input was accepted (Ok), `false` if rejected (typed error).
fn run_one(
    ctx: &Ctx,
    st: &mut TargetStats,
    target: &str,
    i: usize,
    input: &[u8],
    ops: &[String],
    f: impl FnOnce() -> bool,
) {
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    LARGEST.store(0, Relaxed);
    let t0 = Instant::now();
    let res = catch_unwind(AssertUnwindSafe(f));
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let peak = PEAK.load(Relaxed).saturating_sub(base);
    let largest = LARGEST.load(Relaxed);
    st.iterations += 1;
    if ms > st.worst_ms.1 {
        st.worst_ms = (i, ms);
        let _ = std::fs::write(ctx.crash_dir.join(format!("{target}-slowest.bin")), input);
    }
    if peak > st.worst_heap.1 {
        st.worst_heap = (i, peak);
        let _ = std::fs::write(ctx.crash_dir.join(format!("{target}-heaviest.bin")), input);
    }
    st.max_ms = st.max_ms.max(ms);
    st.max_peak_heap = st.max_peak_heap.max(peak);
    st.max_single_alloc = st.max_single_alloc.max(largest);
    match res {
        Ok(true) => st.ok += 1,
        Ok(false) => st.rejected += 1,
        Err(_) => {
            let msg = PANIC_MSG.lock().ok().and_then(|mut g| g.take()).unwrap_or_default();
            let file = ctx.crash_dir.join(format!("{target}-{i}.bin"));
            let _ = std::fs::write(&file, input);
            st.panics.push(json!({"iteration": i, "message": msg, "ops": ops, "input": file.to_string_lossy(), "bytes": input.len()}));
        }
    }
    if ms > ctx.max_ms && st.slow.len() < 10 {
        let file = ctx.crash_dir.join(format!("{target}-{i}.slow.bin"));
        let _ = std::fs::write(&file, input);
        st.slow.push(json!({"iteration": i, "ms": ms, "ops": ops, "input": file.to_string_lossy()}));
    }
    if peak > ctx.max_heap && st.heavy.len() < 10 {
        let file = ctx.crash_dir.join(format!("{target}-{i}.heap.bin"));
        let _ = std::fs::write(&file, input);
        st.heavy.push(json!({"iteration": i, "peakHeapBytes": peak, "largestAlloc": largest, "ops": ops, "input": file.to_string_lossy()}));
    }
}

fn byte_target(ctx: &Ctx, name: &str, corpus: &str, iters: usize, seed: u64, f: &dyn Fn(&[u8]) -> bool) -> TargetStats {
    let seeds = seeds(corpus);
    assert!(!seeds.is_empty(), "no seeds in tests/fuzz-corpus/{corpus}");
    let mut rng = Rng::new(seed);
    let mut st = TargetStats::default();
    for i in 0..iters {
        let (_, base) = &seeds[i % seeds.len()];
        let mut input = base.clone();
        let ops = if i < seeds.len() { vec!["seed".to_string()] } else { mutate(&mut rng, &mut input, &seeds) };
        run_one(ctx, &mut st, name, i, &input, &ops, || f(&input));
    }
    st
}

fn new_session() -> Session {
    let mut s = Session::new(EngineConfig { deterministic_seed: Some(77), ..Default::default() });
    s.execute_json(r##"{"cmd":"document.create","width":128,"height":96,"background":"#FFFFFF"}"##);
    s.execute_json(r#"{"cmd":"view.size","width":128,"height":96}"#);
    s.execute_json(r#"{"cmd":"layer.add","kind":"vector","name":"V"}"#);
    s.execute_json(r#"{"cmd":"layer.add","kind":"group","name":"G"}"#);
    s
}

fn layer_ids(s: &mut Session) -> Vec<String> {
    let v: Value = serde_json::from_str(&s.execute_json(r#"{"cmd":"layer.list"}"#)).unwrap_or(Value::Null);
    v["ok"]["layers"]
        .as_array()
        .map(|a| a.iter().filter_map(|l| l["id"].as_str().map(String::from)).collect())
        .unwrap_or_default()
}

#[test]
fn fuzz_smoke_all_targets() {
    let iters = env_usize("VESKRA_FUZZ_ITERS", 1000);
    let ctx = Ctx {
        max_ms: env_usize("VESKRA_FUZZ_MAX_MS", 5000) as f64,
        max_heap: env_usize("VESKRA_FUZZ_MAX_HEAP_MB", 512) << 20,
        crash_dir: qa_root().join(format!("fuzz/crashes-{iters}")),
    };
    let _ = std::fs::remove_dir_all(&ctx.crash_dir);
    std::fs::create_dir_all(&ctx.crash_dir).unwrap();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        if let Ok(mut g) = PANIC_MSG.lock() {
            *g = Some(format!("{payload} at {loc}"));
        }
    }));
    let started = Instant::now();
    let srgb = veskra_color::IccProfile::srgb();
    let mut results: Vec<(&str, TargetStats, f64)> = Vec::new();
    let mut time =
        |name: &'static str, st: TargetStats, t: Instant| results.push((name, st, t.elapsed().as_secs_f64()));

    let t = Instant::now();
    let st = byte_target(&ctx, "icc", "icc", iters, 11, &|b| match veskra_color::IccProfile::parse(b) {
        Ok(p) => {
            let _ = p.is_srgb_equivalent();
            let _ = veskra_color::RgbTransform::new(&p, &srgb, veskra_color::RenderingIntent::Perceptual);
            let _ = veskra_color::RgbTransform::new(&srgb, &p, veskra_color::RenderingIntent::AbsoluteColorimetric);
            true
        }
        Err(_) => false,
    });
    time("icc: IccProfile::parse + RgbTransform::new", st, t);

    let t = Instant::now();
    let st = byte_target(&ctx, "veskra", "veskra", iters, 12, &|b| {
        let _ = veskra_format::inspect(b);
        let _ = veskra_format::read_preview(b);
        veskra_format::decode(b).is_ok()
    });
    time("veskra: veskra_format::{inspect, read_preview, decode}", st, t);

    // Structure-aware: valid ZIP + recomputed integrity, mutated JSON / tiles / strokes / ICC.
    let t = Instant::now();
    let vseeds: Vec<(String, Vec<u8>)> =
        seeds("veskra").into_iter().filter(|(_, b)| veskra_format::decode(b).is_ok()).collect();
    let mut rng = Rng::new(19);
    let mut st = TargetStats::default();
    for i in 0..iters {
        let (_, base) = &vseeds[i % vseeds.len()];
        let Some((input, ops)) = mutate_bundle(&mut rng, base, &vseeds) else { continue };
        run_one(&ctx, &mut st, "veskra-struct", i, &input, &ops, || {
            let _ = veskra_format::inspect(&input);
            match veskra_format::decode(&input) {
                Ok(l) => {
                    // A document that decodes must also composite and re-encode.
                    let _ = veskra_compose::thumbnail_rgba8(&l.doc, 32);
                    let _ = veskra_format::encode(&l.doc, &veskra_format::SaveMeta::new("fuzz", "x"), None);
                    true
                }
                Err(_) => false,
            }
        });
    }
    time("veskra-struct: decode of re-zipped bundles with valid integrity (+ compose, encode)", st, t);

    let t = Instant::now();
    let st = byte_target(&ctx, "psd", "psd", iters, 13, &|b| {
        let _ = veskra_io::psd::inspect_psd(b, true);
        veskra_io::exporter_for(ExportFormat::Psd).validate(b).is_ok()
    });
    time("psd: inspect_psd(keep_pixels) + PsdExporter::validate", st, t);

    for (name, corpus, fmt, seed) in [
        ("png: PngExporter::validate (inspect_png)", "png", ExportFormat::Png, 14u64),
        ("svg: SvgExporter::validate (XML well-formedness)", "svg", ExportFormat::Svg, 15),
        ("pdf: PdfExporter::validate (validate_pdf)", "pdf", ExportFormat::Pdf, 16),
    ] {
        let t = Instant::now();
        let st = byte_target(&ctx, corpus, corpus, iters, seed, &|b| veskra_io::exporter_for(fmt).validate(b).is_ok());
        time(name, st, t);
    }

    // input: unpack + feed the decoded events to a live session
    let t = Instant::now();
    let input_seeds = seeds("input");
    let mut rng = Rng::new(17);
    let mut st = TargetStats::default();
    let mut s = new_session();
    for i in 0..iters {
        if i % 200 == 0 {
            s = new_session();
        }
        let mut blob = input_seeds[i % input_seeds.len()].1.clone();
        let ops = if i < input_seeds.len() { vec!["seed".into()] } else { mutate(&mut rng, &mut blob, &input_seeds) };
        let (ints, floats, longs, count) = decode_input_blob(&blob);
        let sref = &mut s;
        run_one(&ctx, &mut st, "input", i, &blob, &ops, move || {
            match veskra_input::unpack_events(&ints, &floats, &longs, count) {
                Ok(ev) => {
                    let _ = sref.input_batch(&ev);
                    let _ = sref.execute_json(r#"{"cmd":"input.reset"}"#);
                    true
                }
                Err(_) => false,
            }
        });
        if st.panics.len() > 50 {
            break;
        }
    }
    time("input: unpack_events + Session::input_batch", st, t);

    // commands
    let t = Instant::now();
    let cmd_lines: Vec<String> = seeds("commands")
        .iter()
        .flat_map(|(_, b)| {
            String::from_utf8_lossy(b)
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .collect();
    let byte_seeds: Vec<(String, Vec<u8>)> = cmd_lines.iter().map(|l| (String::new(), l.as_bytes().to_vec())).collect();
    let sb = sandbox();
    let mut rng = Rng::new(18);
    let mut st = TargetStats::default();
    let mut s = new_session();
    let mut ids = layer_ids(&mut s);
    for i in 0..iters {
        if i % 150 == 0 {
            s = new_session();
            ids = layer_ids(&mut s);
        }
        let base = &cmd_lines[i % cmd_lines.len()];
        let (text, ops) = if i < cmd_lines.len() {
            (base.clone(), vec!["seed".to_string()])
        } else if rng.below(2) == 0 {
            (mutate_command(&mut rng, base, &ids), vec!["json".to_string()])
        } else {
            let mut b = base.as_bytes().to_vec();
            let ops = mutate(&mut rng, &mut b, &byte_seeds);
            (String::from_utf8_lossy(&b).into_owned(), ops)
        };
        let cmd = sanitize(&text, &sb);
        let sref = &mut s;
        run_one(&ctx, &mut st, "commands", i, cmd.as_bytes(), &ops, || {
            let out = sref.execute_json(&cmd);
            serde_json::from_str::<Value>(&out).map(|v| v.get("ok").is_some()).unwrap_or(false)
        });
        if i % 25 == 0 {
            ids = layer_ids(&mut s);
        }
    }
    time("commands: Session::execute_json", st, t);
    std::panic::set_hook(prev_hook);

    // ---- report ----
    let total_s = started.elapsed().as_secs_f64();
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (name, st, secs) in &results {
        rows.push(json!({"target": name, "iterations": st.iterations, "accepted": st.ok, "rejectedTyped": st.rejected,
                         "panics": st.panics.len(), "maxMsPerInput": (st.max_ms * 100.0).round() / 100.0,
                         "maxPeakHeapBytes": st.max_peak_heap, "maxSingleAllocBytes": st.max_single_alloc,
                         "overTime": st.slow.len(), "overHeap": st.heavy.len(),
                         "slowestIteration": st.worst_ms.0, "heaviestIteration": st.worst_heap.0, "seconds": (secs * 100.0).round() / 100.0,
                         "panicDetails": st.panics.iter().take(10).collect::<Vec<_>>(), "slowDetails": st.slow, "heapDetails": st.heavy}));
        if !st.panics.is_empty() || !st.slow.is_empty() || !st.heavy.is_empty() {
            problems.push(name.to_string());
        }
    }
    let out = json!({
        "environment": "HOST CPU; deterministic mutation smoke fuzzer (not coverage-guided)",
        "profile": if cfg!(debug_assertions) { "test (debug assertions on)" } else { "release" },
        "itersPerTarget": iters, "limits": {"maxMsPerInput": ctx.max_ms, "maxPeakHeapBytes": ctx.max_heap},
        "totalSeconds": total_s, "targets": rows, "problemTargets": problems,
    });
    let dir = qa_root().join("fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    let name = format!("result-{iters}.json");
    std::fs::write(dir.join(&name), serde_json::to_vec_pretty(&out).unwrap()).unwrap();
    std::fs::write(dir.join("result.json"), serde_json::to_vec_pretty(&out).unwrap()).unwrap();
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&out["targets"].as_array().map(|a| {
            a.iter()
                .map(|r| {
                    json!({
        "target": r["target"], "iterations": r["iterations"], "accepted": r["accepted"], "panics": r["panics"],
        "maxMs": r["maxMsPerInput"], "maxPeakHeap": r["maxPeakHeapBytes"], "seconds": r["seconds"]})
                })
                .collect::<Vec<_>>()
        }))
        .unwrap()
    );
    assert!(problems.is_empty(), "fuzz problems in {problems:?}; see {}", dir.join(&name).display());
}

// ---------------------------------------------------------------------------- corpus

/// Total size of the committed corpus stays small; with `VESKRA_REGEN_FUZZ_CORPUS=1` it is
/// regenerated deterministically from Veskra itself.
#[test]
fn corpus_present_and_small() {
    if std::env::var("VESKRA_REGEN_FUZZ_CORPUS").as_deref() == Ok("1") {
        regenerate();
    }
    let mut total = 0u64;
    for t in ["icc", "veskra", "psd", "png", "svg", "pdf", "input", "commands"] {
        let s = seeds(t);
        assert!(!s.is_empty(), "tests/fuzz-corpus/{t} is empty");
        total += s.iter().map(|x| x.1.len() as u64).sum::<u64>();
    }
    assert!(corpus_dir().join("README.md").exists());
    assert!(total < 1_000_000, "fuzz corpus is {total} bytes (limit 1 MB)");
}

fn put(rel: &str, bytes: &[u8]) {
    let p = corpus_dir().join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn regenerate() {
    let c = repo_root().join("tests/fixtures/color/icc");
    put("icc/veskra-srgb-v4.icc", veskra_color::IccProfile::srgb().bytes());
    put("icc/veskra-display-p3-v4.icc", veskra_color::IccProfile::display_p3().bytes());
    put("icc/lcms2-builtin-srgb.icc", &std::fs::read(c.join("lcms2-builtin-srgb.icc")).unwrap());

    // Small documents covering groups, clipping, blend modes, masks/selection, vector strokes, hidden layers.
    let mut d = Driver::new(9001);
    d.ok(json!({"cmd": "document.create", "width": 64, "height": 48, "dpi": 144, "background": "#F0E8D8"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pencil.rough"}));
    d.stroke(&wave(4.0, 60.0, 12.0, 4.0, 24));
    let v = d.add_layer("vector", "Ink", json!({"at": "top"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.technical"}));
    d.ok(json!({"cmd": "brush.set", "size": 3.0}));
    d.stroke(&ellipse((32.0, 24.0), (14.0, 10.0), 24, 0.05, 0.8));
    d.add_layer("raster", "Shade", json!({"at": "below", "id": v}));
    d.ok(json!({"cmd": "selection.ellipse", "x0": 10, "y0": 8, "x1": 40, "y1": 40}));
    d.ok(json!({"cmd": "color.set", "hex": "#8040C080"}));
    d.ok(json!({"cmd": "edit.fill_selection"}));
    let sh = d.layer_id("Shade");
    d.ok(json!({"cmd": "layer.set", "id": sh, "blend": "multiply", "clip": true}));
    let g = d.add_layer("group", "Group", json!({"at": "top"}));
    let inner = d.add_layer("raster", "Inner", json!({"at": "into", "group": g}));
    d.ok(json!({"cmd": "layer.set", "id": g, "opacity": 0.7}));
    d.select(&inner);
    d.ok(json!({"cmd": "brush.select", "preset": "charcoal"}));
    d.ok(json!({"cmd": "color.set", "hex": "#204060"}));
    d.stroke(&line((4.0, 40.0), (60.0, 30.0), 20, 0.4, 0.9));
    let hidden = d.add_layer("raster", "Hidden", json!({"at": "top"}));
    d.ok(json!({"cmd": "layer.set", "id": hidden, "visible": false}));
    let dir = std::env::temp_dir().join("veskra-qa-corpus");
    std::fs::create_dir_all(&dir).unwrap();
    let save = |d: &mut Driver, name: &str| {
        let pth = dir.join(name);
        d.ok(json!({"cmd": "document.save", "path": pth.to_string_lossy()}));
        std::fs::read(&pth).unwrap()
    };
    put("veskra/layers-groups-selection.veskra", &save(&mut d, "a.veskra"));
    let exp = |d: &mut Driver, fmt: &str, name: &str, opts: Value| {
        let pth = dir.join(name);
        d.ok(json!({"cmd": "export.run", "format": fmt, "path": pth.to_string_lossy(), "options": opts}));
        std::fs::read(&pth).unwrap()
    };
    let both = json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": true});
    let vec_only = json!({"includeVectorPaths": true, "includeRasterAppearance": false, "embedColorProfile": false});
    put("psd/layers-groups-clip.psd", &exp(&mut d, "psd", "a.psd", both.clone()));
    put("svg/hybrid.svg", &exp(&mut d, "svg", "a.svg", both.clone()));
    put("svg/vector-only-noicc.svg", &exp(&mut d, "svg", "b.svg", vec_only.clone()));
    put("pdf/hybrid.pdf", &exp(&mut d, "pdf", "a.pdf", both.clone()));
    put("pdf/vector-only-noicc.pdf", &exp(&mut d, "pdf", "b.pdf", vec_only));
    let mut small = Driver::new(9002);
    small.ok(json!({"cmd": "document.create", "width": 24, "height": 16, "background": "#3366CC80"}));
    put("png/rgba-icc.png", &exp(&mut small, "png", "a.png", both));
    put(
        "png/rgba-srgb-chunk.png",
        &exp(
            &mut small,
            "png",
            "b.png",
            json!({"includeVectorPaths": true, "includeRasterAppearance": true, "embedColorProfile": false}),
        ),
    );
    put("veskra/tiny-transparent.veskra", &save(&mut small, "b.veskra"));
    put(
        "veskra/future-schema.veskra",
        &std::fs::read(repo_root().join("tests/fixtures/documents/future-schema.veskra")).unwrap(),
    );

    // Packed input batches.
    let stroke = events::stylus_stroke(
        &wave(5.0, 120.0, 40.0, 10.0, 24),
        1_000_000,
        0,
        ToolType::Stylus,
        true,
        PointerAction::Down,
    );
    put("input/stylus-historical-predicted.bin", &encode_input_blob(&stroke));
    put("input/pinch.bin", &encode_input_blob(&events::pinch((64.0, 48.0), 30.0, 60.0, 6, 5_000_000)));
    put(
        "input/palm-and-stylus.bin",
        &encode_input_blob(&events::stylus_with_resting_palm(
            &line((10.0, 10.0), (90.0, 60.0), 12, 0.3, 0.9),
            (100.0, 80.0),
            9_000_000,
        )),
    );
    let mut eraser = events::stylus_stroke(
        &line((20.0, 20.0), (100.0, 70.0), 10, 1.0, 1.0),
        2_000_000,
        3,
        ToolType::StylusEraser,
        false,
        PointerAction::Down,
    );
    if let Some(l) = eraser.last_mut() {
        l.action = PointerAction::Cancel;
    }
    put("input/eraser-cancel.bin", &encode_input_blob(&eraser));

    // One line per command family (paths are rewritten into the sandbox by the harness).
    let lines = [
        r##"{"cmd":"document.create","width":64,"height":48,"dpi":300,"background":"#FFFFFF"}"##,
        r#"{"cmd":"document.info"}"#,
        r#"{"cmd":"document.save","path":"f.veskra"}"#,
        r#"{"cmd":"document.open","path":"f.veskra"}"#,
        r#"{"cmd":"document.validate","path":"f.veskra"}"#,
        r#"{"cmd":"document.autosave","dir":"autosave"}"#,
        r#"{"cmd":"recovery.list","dir":"autosave"}"#,
        r#"{"cmd":"document.recover","dir":"autosave"}"#,
        r#"{"cmd":"layer.add","kind":"raster","name":"L","placement":{"at":"top"}}"#,
        r#"{"cmd":"layer.add","kind":"group","name":"G"}"#,
        r#"{"cmd":"layer.set","id":"00000000-0000-0000-0000-000000000001","opacity":0.5,"blend":"multiply","clip":true,"alphaLocked":true}"#,
        r#"{"cmd":"layer.move","id":"00000000-0000-0000-0000-000000000001","placement":{"at":"into","group":"00000000-0000-0000-0000-000000000002","index":0}}"#,
        r#"{"cmd":"layer.list"}"#,
        r#"{"cmd":"brush.select","preset":"charcoal"}"#,
        r#"{"cmd":"brush.set","size":12.5,"opacity":0.8,"flow":0.6,"stabilization":0.3}"#,
        r##"{"cmd":"color.set","hex":"#20406080"}"##,
        r#"{"cmd":"color.pick","x":10.5,"y":10.5,"layer":true}"#,
        r#"{"cmd":"stroke.begin","eraser":false}"#,
        r#"{"cmd":"stroke.samples","samples":[{"x":5,"y":5,"t":0,"p":0.4},{"x":40,"y":30,"t":4000000,"p":0.9,"tilt":0.3,"azimuth":1.0}]}"#,
        r#"{"cmd":"stroke.commit"}"#,
        r#"{"cmd":"stroke.cancel"}"#,
        r#"{"cmd":"history.undo"}"#,
        r#"{"cmd":"history.redo"}"#,
        r#"{"cmd":"selection.rect","x0":2,"y0":2,"x1":30,"y1":20,"op":"add"}"#,
        r#"{"cmd":"selection.ellipse","x0":2,"y0":2,"x1":30,"y1":20,"op":"subtract"}"#,
        r#"{"cmd":"selection.lasso","points":[[1,1],[50,4],[30,40]],"op":"intersect"}"#,
        r#"{"cmd":"selection.invert"}"#,
        r#"{"cmd":"edit.fill","x":3,"y":3,"tolerance":0.2,"contiguous":false,"sampleMerged":true}"#,
        r#"{"cmd":"edit.fill_selection"}"#,
        r#"{"cmd":"edit.delete_selection"}"#,
        r#"{"cmd":"edit.transform","matrix":[0.5,0.2,-0.2,1.5,10,-4]}"#,
        r#"{"cmd":"view.set","zoom":3.0,"rotation":0.7,"panX":-20,"panY":15,"mirror":true}"#,
        r#"{"cmd":"view.fit"}"#,
        r#"{"cmd":"input.config","fingerDraws":false,"stylusButton":"eyedropper"}"#,
        r#"{"cmd":"input.reset"}"#,
        r#"{"cmd":"export.preflight","format":"svg","options":{"includeVectorPaths":true,"includeRasterAppearance":false,"embedColorProfile":true}}"#,
        r#"{"cmd":"export.run","format":"psd","path":"f.psd"}"#,
        r#"{"cmd":"export.run","format":"pdf","path":"f.pdf","options":{"includeVectorPaths":false,"includeRasterAppearance":true,"embedColorProfile":false,"rasterScale":2.0}}"#,
    ];
    put("commands/commands.jsonl", (lines.join("\n") + "\n").as_bytes());
}
