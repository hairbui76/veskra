//! `veskra` — headless test runner using the same engine as Android (ARCHITECTURE §I).
//!
//! ```text
//! veskra run <script.jsonl> [--seed N]        execute one JSON command per line, print receipts
//! veskra replay-input <trace.jsonl> <out.veskra> [--size WxH]
//!                                              replay raw pointer events through the input arbiter
//! veskra render <in.veskra> <out.png>          composite a project to an sRGB PNG
//! veskra inspect-project <in.veskra>           manifest/entries/layers summary as JSON
//! veskra inspect-export <file.(png|psd|svg|pdf)>  structural validation of an export
//! veskra export <in.veskra> <out.ext> [--no-vector] [--no-raster] [--no-icc]
//!               [--profile document|srgb|display_p3|adobe_rgb] [--depth 8|16]
//! veskra benchmark-stroke [--size N] [--samples N] [--brush ID] [--input] [--predict]
//! veskra compare-color <a.png> <b.png>         per-channel diff + CIEDE2000 stats (sRGB, opaque)
//! ```
//! Exit code 0 on success, 1 on any error (errors printed as JSON on stderr).

use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;
use veskra_core::ToolType;
use veskra_core::export::{ExportFormat, ExportOptions};
use veskra_engine::veskra_input::{PointerAction, RawPointerEvent};
use veskra_engine::{Command, EngineConfig, Session};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(v) => {
            if !v.is_null() {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{}", json!({"error": e}));
            ExitCode::FAILURE
        }
    }
}

type R<T> = Result<T, String>;

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn session(seed: Option<u64>) -> Session {
    Session::new(EngineConfig { deterministic_seed: seed, ..Default::default() })
}

fn exec(s: &mut Session, cmd: Command) -> R<Value> {
    s.execute(cmd).map(|r| r.result).map_err(|e| e.to_string())
}

fn format_for(path: &Path) -> R<ExportFormat> {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => Ok(ExportFormat::Png),
        Some("psd") => Ok(ExportFormat::Psd),
        Some("svg") => Ok(ExportFormat::Svg),
        Some("pdf") => Ok(ExportFormat::Pdf),
        Some("ai") => Err("native .ai export is not supported; export SVG or PDF and use Illustrator Save As".into()),
        _ => Err("unknown export extension (png, psd, svg, pdf)".into()),
    }
}

fn run(args: &[String]) -> R<Value> {
    let Some(cmd) = args.first() else {
        return Err("usage: veskra <run|replay-input|render|inspect-project|inspect-export|export|benchmark-stroke|compare-color> ...".into());
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "run" => {
            let path = rest.first().ok_or("run <script.jsonl>")?;
            let seed = opt(rest, "--seed").map(|s| s.parse::<u64>().map_err(|e| e.to_string())).transpose()?;
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let mut s = session(seed.or(Some(1)));
            let mut out = Vec::new();
            for (i, line) in text.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let r: Value = serde_json::from_str(&s.execute_json(line)).map_err(|e| e.to_string())?;
                if r.get("error").is_some() && !flag(rest, "--keep-going") {
                    return Err(format!("line {}: {}", i + 1, r["error"]));
                }
                out.push(r);
            }
            Ok(json!({"results": out}))
        }
        "replay-input" => {
            let (trace, outp) = (rest.first().ok_or("replay-input <trace> <out>")?, rest.get(1).ok_or("missing out")?);
            let (w, h) = opt(rest, "--size")
                .and_then(|s| s.split_once('x'))
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                .unwrap_or((1024u32, 1024u32));
            let text = std::fs::read_to_string(trace).map_err(|e| e.to_string())?;
            let events = veskra_engine::veskra_input::trace::parse_jsonl(&text).map_err(|e| e.to_string())?;
            let mut s = session(Some(1));
            exec(
                &mut s,
                Command::DocumentCreate { width: w, height: h, dpi: 300.0, background: Some("#FFFFFF".into()) },
            )?;
            exec(&mut s, Command::ViewSize { width: w as f64, height: h as f64 })?;
            exec(
                &mut s,
                Command::ViewSet {
                    zoom: Some(1.0),
                    rotation: Some(0.0),
                    pan_x: Some(0.0),
                    pan_y: Some(0.0),
                    mirror: Some(false),
                },
            )?;
            let start = Instant::now();
            let mut commits = 0;
            for chunk in events.chunks(32) {
                let o = s.input_batch(chunk);
                if o.flags & veskra_engine::flags::COMMITTED != 0 {
                    commits += 1;
                }
            }
            let el = start.elapsed();
            // A trace may end mid-stroke (app paused/killed): apply the lifecycle policy
            // (commit confirmed samples) instead of failing the save on a live stroke.
            let reset = exec(&mut s, Command::InputReset)?;
            if reset["flags"].as_i64().unwrap_or(0) & i64::from(veskra_engine::flags::COMMITTED) != 0 {
                commits += 1;
            }
            exec(&mut s, Command::DocumentSave { path: outp.clone() })?;
            let info = exec(&mut s, Command::DocumentInfo)?;
            Ok(
                json!({"events": events.len(), "commitBatches": commits, "elapsedMs": el.as_secs_f64() * 1e3, "info": info}),
            )
        }
        "render" => {
            let (inp, outp) = (rest.first().ok_or("render <in> <out.png>")?, rest.get(1).ok_or("missing out")?);
            let mut s = session(None);
            exec(&mut s, Command::DocumentOpen { path: inp.clone() })?;
            exec(
                &mut s,
                Command::ExportRun { format: ExportFormat::Png, path: outp.clone(), options: ExportOptions::default() },
            )
        }
        "export" => {
            let (inp, outp) = (rest.first().ok_or("export <in> <out>")?, rest.get(1).ok_or("missing out")?);
            let format = format_for(Path::new(outp))?;
            let mut options = ExportOptions {
                include_vector_paths: !flag(rest, "--no-vector"),
                include_raster_appearance: !flag(rest, "--no-raster"),
                embed_color_profile: !flag(rest, "--no-icc"),
                ..Default::default()
            };
            if let Some(p) = opt(rest, "--profile") {
                options.profile = serde_json::from_value(json!(p))
                    .map_err(|_| format!("unknown --profile {p} (document|srgb|display_p3|adobe_rgb)"))?;
            }
            if let Some(d) = opt(rest, "--depth") {
                options.bit_depth = d.parse().map_err(|_| format!("bad --depth {d} (8|16)"))?;
            }
            let mut s = session(None);
            exec(&mut s, Command::DocumentOpen { path: inp.clone() })?;
            let pre = exec(&mut s, Command::ExportPreflight { format, options: options.clone() })?;
            let rep = exec(&mut s, Command::ExportRun { format, path: outp.clone(), options })?;
            Ok(json!({"preflight": pre, "report": rep}))
        }
        "inspect-project" => {
            let inp = rest.first().ok_or("inspect-project <in.veskra>")?;
            let bytes = std::fs::read(inp).map_err(|e| e.to_string())?;
            let info = veskra_format::inspect(&bytes).map_err(|e| veskra_core::VeskraError::from(e).to_string())?;
            let mut s = session(None);
            // A bundle can be structurally readable yet not openable (future schema, corrupt
            // document data): say why instead of silently reporting `layers: null`.
            match exec(&mut s, Command::DocumentOpen { path: inp.clone() })
                .and_then(|_| exec(&mut s, Command::LayerList))
            {
                Ok(layers) => Ok(json!({"bundle": format!("{info:?}"), "layers": layers})),
                Err(e) => Ok(json!({"bundle": format!("{info:?}"), "layers": null, "openError": e})),
            }
        }
        "inspect-export" => {
            let inp = rest.first().ok_or("inspect-export <file>")?;
            let format = format_for(Path::new(inp))?;
            let bytes = std::fs::read(inp).map_err(|e| e.to_string())?;
            veskra_io::exporter_for(format).validate(&bytes).map_err(|e| e.to_string())?;
            Ok(json!({"format": format, "bytes": bytes.len(), "structurallyValid": true}))
        }
        "benchmark-stroke" => {
            let size: u32 = opt(rest, "--size").and_then(|s| s.parse().ok()).unwrap_or(2048);
            let samples: usize = opt(rest, "--samples").and_then(|s| s.parse().ok()).unwrap_or(2000);
            let brush = opt(rest, "--brush").unwrap_or("pen.brush").to_string();
            if samples == 0 {
                return Err("--samples must be at least 1".into());
            }
            let mut s = session(Some(7));
            exec(
                &mut s,
                Command::DocumentCreate { width: size, height: size, dpi: 300.0, background: Some("#FFFFFF".into()) },
            )?;
            exec(&mut s, Command::BrushSelect { preset: brush.clone() })?;
            // `--input` drives the Android hot path (raw events → arbiter → live stroke);
            // `--predict` adds 3 predicted (display-only) events per batch, as Android does.
            let via_input = flag(rest, "--input") || flag(rest, "--predict");
            let predict = flag(rest, "--predict");
            let pt = |k: usize| {
                let t = k as f32 / samples as f32;
                let (x, y) = (size as f32 * (0.1 + 0.8 * t), size as f32 * (0.5 + 0.3 * (t * 12.0).sin()));
                (x, y, 0.3 + 0.7 * (t * 3.0).sin().abs(), k as u64 * 4_000_000)
            };
            let raw = |a: PointerAction, k: usize, predicted: bool| {
                let (x, y, p, t) = pt(k);
                let mut e = RawPointerEvent::new(a, 0, ToolType::Stylus, x, y, 1_000_000_000 + t);
                e.pressure = Some(p);
                e.predicted = predicted;
                e
            };
            if via_input {
                exec(&mut s, Command::ViewSize { width: size as f64, height: size as f64 })?;
                exec(
                    &mut s,
                    Command::ViewSet {
                        zoom: Some(1.0),
                        rotation: Some(0.0),
                        pan_x: Some(0.0),
                        pan_y: Some(0.0),
                        mirror: Some(false),
                    },
                )?;
            } else {
                exec(&mut s, Command::StrokeBegin { eraser: false, tool: None })?;
            }
            let mut lat = Vec::with_capacity(samples / 4 + 1);
            let t0 = Instant::now();
            let batch = 4;
            let mut i = 0;
            while i < samples {
                let end = (i + batch).min(samples);
                let tb = Instant::now();
                if via_input {
                    let mut evs: Vec<RawPointerEvent> = (i..end)
                        .map(|k| raw(if k == 0 { PointerAction::Down } else { PointerAction::Move }, k, false))
                        .collect();
                    if predict {
                        evs.extend((end..end + 3).map(|k| raw(PointerAction::Move, k, true)));
                    }
                    let _ = s.input_batch(&evs);
                } else {
                    let pts: Vec<veskra_engine::SampleSpec> = (i..end)
                        .map(|k| {
                            let (x, y, p, t) = pt(k);
                            veskra_engine::SampleSpec { x, y, t, p: Some(p), tilt: None, azimuth: None }
                        })
                        .collect();
                    exec(&mut s, Command::StrokeSamples { samples: pts })?;
                }
                lat.push(tb.elapsed().as_secs_f64() * 1e3);
                i = end;
            }
            let tc = Instant::now();
            if via_input {
                let o = s.input_batch(&[raw(PointerAction::Up, samples - 1, false)]);
                if o.flags & veskra_engine::flags::COMMITTED == 0 {
                    return Err(format!("stroke was not committed: {:?}", o.warnings));
                }
            } else {
                exec(&mut s, Command::StrokeCommit)?;
            }
            let commit_ms = tc.elapsed().as_secs_f64() * 1e3;
            let total = t0.elapsed().as_secs_f64() * 1e3;
            lat.sort_by(|a, b| a.total_cmp(b));
            let pct = |p: f64| lat[((lat.len() as f64 - 1.0) * p) as usize];
            Ok(json!({"environment": "HOST CPU (not a device measurement)", "brush": brush, "canvas": size,
                      "path": if predict { "input+prediction" } else if via_input { "input" } else { "stroke.samples" },
                      "samples": samples, "batchSize": batch, "totalMs": total, "commitMs": commit_ms,
                      "batchLatencyMs": {"p50": pct(0.5), "p95": pct(0.95), "max": pct(1.0)}}))
        }
        "compare-color" => {
            let (a, b) = (rest.first().ok_or("compare-color <a.png> <b.png>")?, rest.get(1).ok_or("missing b")?);
            let (wa, ha, pa) = veskra_testkit::decode_png_rgba8(&std::fs::read(a).map_err(|e| e.to_string())?)?;
            let (wb, hb, pb) = veskra_testkit::decode_png_rgba8(&std::fs::read(b).map_err(|e| e.to_string())?)?;
            if (wa, ha) != (wb, hb) {
                return Err(format!("size mismatch {wa}x{ha} vs {wb}x{hb}"));
            }
            let d = veskra_testkit::ImageDiff::rgba8(&pa, &pb, 1);
            let mut de: Vec<f64> = pa
                .chunks_exact(4)
                .zip(pb.chunks_exact(4))
                .filter(|(x, y)| x[3] == 255 && y[3] == 255)
                .map(|(x, y)| {
                    veskra_color::lab::ciede2000(
                        veskra_color::lab::srgb8_to_lab_d50([x[0], x[1], x[2]]),
                        veskra_color::lab::srgb8_to_lab_d50([y[0], y[1], y[2]]),
                    )
                })
                .collect();
            de.sort_by(|x, y| x.total_cmp(y));
            let q = |p: f64| if de.is_empty() { f64::NAN } else { de[((de.len() as f64 - 1.0) * p) as usize] };
            Ok(json!({"maxAbs": d.max_abs, "meanAbs": d.mean_abs, "pixelsOver1": d.pixels_over, "pixels": d.pixels,
                      "opaquePixelsCompared": de.len(), "deltaE00": {"median": q(0.5), "p95": q(0.95), "max": q(1.0)},
                      "note": "values interpreted as sRGB; ICC tags of the inputs are not applied"}))
        }
        other => Err(format!("unknown command {other}")),
    }
}
