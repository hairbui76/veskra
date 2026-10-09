//! QAP-03 fault-injection matrix (engine level, host filesystem).
//!
//! 100 scripted cycles of *edit → `document.save` to the same path*. Every 10 cycles four cycles
//! inject a failure through the public API / real filesystem:
//! * `read_only_dir` — the target's directory is made unwritable (`chmod 555`; as root, where
//!   permission bits are not enforced, `chattr +i`); skipped as NOT_RUN if neither is enforced;
//! * `missing_dir` — path into a directory that does not exist;
//! * `target_is_dir` — the save path is an existing directory (rename over it must fail);
//! * `parent_is_file` — the save path's parent is the user's regular file.
//!
//! After every failure: typed error, previous file byte-identical and loadable (and equal to the
//! last saved snapshot), `dirty` stays true, `savedVersion` unchanged, no `*.tmp-*` leftovers.
//! After every success: `dirty` false, file reloads to the session document, no leftovers.
//!
//! Real ENOSPC (opt-in, needs root + tmpfs mount): `VESKRA_QA_ENOSPC=1`.
//! Results: `target/qa/QAP-03/result.json`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use veskra_qa::drive::{Driver, line};
use veskra_qa::report::Report;

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains(".tmp-"))
                .collect()
        })
        .unwrap_or_default()
}

/// Makes a directory unwritable for the lifetime of the guard.
struct ReadOnly {
    dir: PathBuf,
    chattr: bool,
}

impl ReadOnly {
    fn probe_blocked(dir: &Path) -> bool {
        let probe = dir.join(".qa-probe");
        match std::fs::write(&probe, b"x") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                false
            }
            Err(_) => true,
        }
    }

    fn new(dir: &Path) -> Option<ReadOnly> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).ok()?;
        if Self::probe_blocked(dir) {
            return Some(ReadOnly { dir: dir.to_path_buf(), chattr: false });
        }
        // Root ignores permission bits: try the immutable attribute instead.
        let ok = Command::new("chattr").arg("+i").arg(dir).output().map(|o| o.status.success()).unwrap_or(false);
        let guard = ReadOnly { dir: dir.to_path_buf(), chattr: ok };
        if ok && Self::probe_blocked(dir) { Some(guard) } else { None }
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        if self.chattr {
            let _ = Command::new("chattr").arg("-i").arg(&self.dir).output();
        }
        let _ = std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o755));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Fault {
    None,
    ReadOnlyDir,
    MissingDir,
    TargetIsDir,
    ParentIsFile,
}

impl Fault {
    fn name(self) -> &'static str {
        match self {
            Fault::None => "none",
            Fault::ReadOnlyDir => "read_only_dir",
            Fault::MissingDir => "missing_dir",
            Fault::TargetIsDir => "target_is_dir",
            Fault::ParentIsFile => "parent_is_file",
        }
    }
}

#[test]
fn qap03_save_fault_matrix_100_cycles() {
    let mut r =
        Report::new("QAP-03", "Fault-injection matrix: 100 edit -> save cycles with injected failures (host FS)");
    let rw = r.dir().join("rw");
    std::fs::create_dir_all(&rw).unwrap();
    let main = rw.join("main.veskra");
    let as_dir = r.dir().join("isdir.veskra");
    std::fs::create_dir_all(&as_dir).unwrap();
    std::fs::write(as_dir.join("keep.txt"), b"must survive").unwrap();

    let mut d = Driver::new(3003);
    d.ok(json!({"cmd": "document.create", "width": 256, "height": 256, "background": "#FFFFFF"}));
    d.ok(json!({"cmd": "brush.select", "preset": "pen.brush"}));
    d.ok(json!({"cmd": "document.save", "path": p(&main)}));
    let mut good_bytes = std::fs::read(&main).unwrap();
    let mut good_doc = d.doc();

    let mut tally: BTreeMap<&str, (usize, usize, BTreeMap<String, usize>)> = BTreeMap::new();
    let mut violations: Vec<Value> = Vec::new();
    let mut ro_not_enforced = 0;
    let mut permission_codes = Vec::new();
    for i in 0..100u32 {
        // ---- edit (always creates at least one undo step) ----
        let y = 20.0 + (i % 11) as f32 * 20.0;
        match i % 3 {
            0 | 1 => {
                let hex = format!("#{:02X}30{:02X}", (i * 37 % 256) as u8, (i * 91 % 256) as u8);
                d.ok(json!({"cmd": "color.set", "hex": hex}));
                d.stroke(&line((20.0, y), (236.0, y + 10.0), 24, 0.3, 0.9));
            }
            _ => {
                let id = d.info()["activeLayer"].as_str().unwrap().to_string();
                d.ok(json!({"cmd": "layer.set", "id": id, "opacity": 0.5 + (i % 10) as f32 / 20.0}));
            }
        }
        if i % 25 == 24 {
            d.add_layer("raster", &format!("Layer {}", i + 2), json!({"at": "top"}));
        }
        let info_before = d.info();
        assert_eq!(info_before["dirty"], true);

        let fault = match i % 10 {
            3 => Fault::ReadOnlyDir,
            5 => Fault::MissingDir,
            7 => Fault::TargetIsDir,
            9 => Fault::ParentIsFile,
            _ => Fault::None,
        };
        let mut guard = None;
        let target = match fault {
            Fault::None => main.clone(),
            Fault::ReadOnlyDir => {
                guard = ReadOnly::new(&rw);
                if guard.is_none() {
                    ro_not_enforced += 1;
                }
                main.clone()
            }
            Fault::MissingDir => r.dir().join(format!("missing-{i}")).join("sub").join("main.veskra"),
            Fault::TargetIsDir => as_dir.clone(),
            Fault::ParentIsFile => main.join("child.veskra"),
        };
        if fault == Fault::ReadOnlyDir && guard.is_none() {
            // Not enforceable here: do a normal save instead so the cycle still progresses.
            d.ok(json!({"cmd": "document.save", "path": p(&main)}));
            good_bytes = std::fs::read(&main).unwrap();
            good_doc = d.doc();
            continue;
        }
        let res = d.try_cmd(json!({"cmd": "document.save", "path": p(&target)}));
        drop(guard);
        let info = d.info();
        let entry = tally.entry(fault.name()).or_default();
        entry.0 += 1;
        match (fault, res) {
            (Fault::None, Ok(_)) => {
                let bytes = std::fs::read(&main).unwrap();
                let loaded = veskra_format::decode(&bytes).map(|l| l.doc == *d.doc()).unwrap_or(false);
                if info["dirty"] != false
                    || info["savedVersion"] != info["version"]
                    || !loaded
                    || !leftovers(&rw).is_empty()
                {
                    violations.push(json!({"cycle": i, "fault": "none", "info": info, "reloadsEqual": loaded}));
                } else {
                    entry.1 += 1;
                }
                good_bytes = bytes;
                good_doc = d.doc();
            }
            (Fault::None, Err(e)) => violations.push(json!({"cycle": i, "fault": "none", "unexpectedError": e})),
            (f, Ok(v)) => violations.push(json!({"cycle": i, "fault": f.name(), "unexpectedSuccess": v})),
            (f, Err(e)) => {
                let code = e["code"].as_str().unwrap_or("?").to_string();
                *entry.2.entry(code.clone()).or_default() += 1;
                if f == Fault::ReadOnlyDir {
                    permission_codes.push(code.clone());
                }
                let now = std::fs::read(&main).unwrap_or_default();
                let reload_ok = veskra_format::decode(&now).map(|l| l.doc == *good_doc).unwrap_or(false);
                let validate_ok = d.try_cmd(json!({"cmd": "document.validate", "path": p(&main)})).is_ok();
                let dir_intact = as_dir.is_dir()
                    && std::fs::read(as_dir.join("keep.txt")).ok().as_deref() == Some(&b"must survive"[..]);
                let mut left = leftovers(&rw);
                left.extend(leftovers(r.dir()));
                let typed = matches!(
                    code.as_str(),
                    "Io" | "FilePermissionDenied" | "NotFound" | "InvalidArgument" | "StorageFull"
                );
                let ok = typed
                    && e["recoverable"] == true
                    && now == good_bytes
                    && reload_ok
                    && validate_ok
                    && dir_intact
                    && left.is_empty()
                    && info["dirty"] == true
                    && info["savedVersion"] == info_before["savedVersion"]
                    && info["version"] == info_before["version"];
                if ok {
                    entry.1 += 1;
                } else {
                    violations.push(
                        json!({"cycle": i, "fault": f.name(), "error": e, "previousBytesIntact": now == good_bytes,
                        "previousReloadsToLastSaved": reload_ok, "validates": validate_ok, "dirIntact": dir_intact,
                        "tempLeftovers": left, "info": info}),
                    );
                }
            }
        }
    }
    // Final state: last successful save reloads to the session's last saved snapshot.
    let final_ok = veskra_format::load(&main).map(|l| l.doc == *good_doc).unwrap_or(false);
    let tally_json: Value = tally
        .iter()
        .map(|(k, (n, ok, codes))| (k.to_string(), json!({"cycles": n, "invariantsHeld": ok, "errorCodes": codes})))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    r.metric("cycles", json!(100));
    r.metric("byFault", tally_json.clone());
    r.check(
        "QAP-03.failed_save_preserves_previous_and_dirty",
        &["SAV-005", "SAV-003", "QAP-03"],
        violations.is_empty() && final_ok,
        json!({"violations": violations, "finalFileEqualsLastSaved": final_ok, "byFault": tally_json}),
    );
    if ro_not_enforced > 0 {
        r.not_run(
            "QAP-03.read_only_dir",
            &["SAV-005"],
            "AUTO",
            "ENV:permissions-not-enforced",
            "neither chmod 555 nor chattr +i blocked writes in this environment",
        );
    }
    // Contract consistency: `export_to_path` reports EACCES as FilePermissionDenied; save should too.
    if !permission_codes.is_empty() {
        let all = permission_codes.iter().all(|c| c == "FilePermissionDenied");
        r.check_known(
            "QAP-03.permission_error_code",
            &["SAV-005"],
            "QA-D2",
            all,
            json!({"observedCodes": permission_codes, "expected": "FilePermissionDenied (as veskra_io::export_to_path maps it)",
                   "route": "veskra-format FormatError::from(io::Error) maps PermissionDenied to Io"}),
        );
    }
    enospc(&mut r);
    r.finish();
}

/// Optional real ENOSPC: small tmpfs, fill it, save must fail with StorageFull and keep the
/// previous file. Needs root (mount) and `VESKRA_QA_ENOSPC=1`.
fn enospc(r: &mut Report) {
    if std::env::var("VESKRA_QA_ENOSPC").as_deref() != Ok("1") {
        r.not_run(
            "QAP-03.real_enospc",
            &["SAV-005"],
            "AUTO",
            "OPT-IN:VESKRA_QA_ENOSPC=1",
            "real tmpfs ENOSPC needs root/mount; format-seam ENOSPC is covered in S8",
        );
        return;
    }
    struct Mount(PathBuf);
    impl Drop for Mount {
        fn drop(&mut self) {
            let _ = Command::new("umount").arg(&self.0).output();
        }
    }
    let mnt = r.dir().join("tmpfs");
    std::fs::create_dir_all(&mnt).unwrap();
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "-o", "size=1m", "tmpfs"])
        .arg(&mnt)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !mounted {
        r.not_run("QAP-03.real_enospc", &["SAV-005"], "AUTO", "ENV:mount-denied", "could not mount a tmpfs");
        return;
    }
    let _m = Mount(mnt.clone());
    let target = mnt.join("doc.veskra");
    let mut d = Driver::new(3004);
    d.ok(json!({"cmd": "document.create", "width": 256, "height": 256, "background": "#FFFFFF"}));
    d.stroke(&line((10.0, 10.0), (240.0, 200.0), 30, 0.5, 1.0));
    d.ok(json!({"cmd": "document.save", "path": p(&target)}));
    let good = std::fs::read(&target).unwrap();
    // Fill the filesystem.
    let filler = mnt.join("filler.bin");
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&filler).unwrap();
        let chunk = vec![0xA5u8; 4096];
        while f.write_all(&chunk).is_ok() {}
        let _ = f.sync_all();
    }
    d.ok(json!({"cmd": "color.set", "hex": "#FF0000"}));
    d.stroke(&line((10.0, 200.0), (240.0, 20.0), 30, 0.5, 1.0));
    let res = d.try_cmd(json!({"cmd": "document.save", "path": p(&target)}));
    let info = d.info();
    let intact = std::fs::read(&target).unwrap() == good;
    let left = leftovers(&mnt);
    let code = res.as_ref().err().map(|e| e["code"].clone()).unwrap_or(Value::Null);
    let _ = std::fs::remove_file(&filler);
    let retry = d.try_cmd(json!({"cmd": "document.save", "path": p(&target)})).is_ok();
    let reload = veskra_format::load(&target).map(|l| l.doc == *d.doc()).unwrap_or(false);
    r.check(
        "QAP-03.real_enospc",
        &["SAV-005"],
        code == "StorageFull" && intact && left.is_empty() && info["dirty"] == true && retry && reload,
        json!({"errorCode": code, "previousIntact": intact, "tempLeftovers": left, "dirtyAfterFailure": info["dirty"],
               "saveAfterFreeingSpace": retry, "reloadEqual": reload, "fs": "tmpfs size=1m (host)"}),
    );
}
