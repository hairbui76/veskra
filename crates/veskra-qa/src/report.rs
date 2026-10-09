//! Machine-readable scenario results.
//!
//! Every scenario writes `target/qa/<scenario>/result.json`:
//! ```json
//! {"scenario": "S1", "title": "...", "environment": "HOST ...", "status": "AUTO_PASS",
//!  "counts": {"pass": 12, "fail": 0, "notRun": 3},
//!  "checks": [{"id": "...", "matrix": ["STY-005"], "evidence": "AUTO", "status": "PASS", "detail": {...}},
//!             {"id": "...", "matrix": ["STY-003"], "evidence": "HW", "status": "NOT_RUN", "blocker": "B-2", "detail": "..."}],
//!  "metrics": {...}, "artifacts": [...]}
//! ```
//! `status`: `FAIL` if any automated check failed (including known, routed defects — those are
//! listed with `knownDefect` and do not fail `cargo test`); `AUTO_PASS` if every automated check passed but
//! some sub-checks are `NOT_RUN` (hardware/Adobe/tool missing); `PASS` only if nothing is `NOT_RUN`.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    NotRun,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::NotRun => "NOT_RUN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub matrix: Vec<String>,
    pub evidence: String,
    pub status: Status,
    pub blocker: Option<String>,
    /// Failing check tracked as a known, routed defect (`QA-Dn`, see docs/reports/qa-verification-status.md).
    pub known_defect: Option<String>,
    pub detail: Value,
}

/// Collects checks for one scenario; [`Report::finish`] writes `result.json` and fails the test if
/// any automated check failed (after the artifacts are on disk).
#[derive(Debug)]
pub struct Report {
    scenario: String,
    title: String,
    dir: PathBuf,
    checks: Vec<Check>,
    metrics: Map<String, Value>,
    artifacts: Vec<String>,
}

impl Report {
    /// Start a scenario; (re)creates `target/qa/<scenario>/`.
    pub fn new(scenario: &str, title: &str) -> Self {
        let dir = crate::qa_root().join(scenario);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
        Self {
            scenario: scenario.into(),
            title: title.into(),
            dir,
            checks: Vec::new(),
            metrics: Map::new(),
            artifacts: Vec::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of an artifact inside the scenario directory (recorded in the result).
    pub fn artifact(&mut self, name: &str) -> PathBuf {
        if !self.artifacts.iter().any(|a| a == name) {
            self.artifacts.push(name.to_string());
        }
        self.dir.join(name)
    }

    /// Record an automated (AUTO) check. Returns `ok` so callers can branch.
    pub fn check(&mut self, id: &str, matrix: &[&str], ok: bool, detail: Value) -> bool {
        self.check_ev(id, matrix, "AUTO", ok, detail)
    }

    /// Record an automated check with an explicit evidence label (e.g. `AUTO+COLOR`).
    pub fn check_ev(&mut self, id: &str, matrix: &[&str], evidence: &str, ok: bool, detail: Value) -> bool {
        if !ok {
            eprintln!("[{}] FAIL {id}: {detail}", self.scenario);
        }
        self.checks.push(Check {
            id: id.into(),
            matrix: matrix.iter().map(|s| s.to_string()).collect(),
            evidence: evidence.into(),
            status: if ok { Status::Pass } else { Status::Fail },
            blocker: None,
            known_defect: None,
            detail,
        });
        ok
    }

    /// A check that currently fails because of a known defect routed to the owning crate.
    /// A failure is recorded as `FAIL` with `knownDefect` (reported, but it does not fail the
    /// test run); once the defect is fixed the check simply passes (detail notes the fix).
    pub fn check_known(&mut self, id: &str, matrix: &[&str], defect: &str, ok: bool, detail: Value) -> bool {
        if !ok {
            eprintln!("[{}] KNOWN DEFECT {defect} {id}: {detail}", self.scenario);
        }
        self.checks.push(Check {
            id: id.into(),
            matrix: matrix.iter().map(|s| s.to_string()).collect(),
            evidence: "AUTO".into(),
            status: if ok { Status::Pass } else { Status::Fail },
            blocker: None,
            known_defect: Some(defect.into()),
            detail,
        });
        ok
    }

    /// Record a sub-check that cannot run here (hardware, Adobe, missing tool).
    /// `blocker` is a `docs/BLOCKERS.md` id (`B-2`, …) or `TOOL:<name>` for a missing oracle tool.
    pub fn not_run(&mut self, id: &str, matrix: &[&str], evidence: &str, blocker: &str, reason: &str) {
        self.checks.push(Check {
            id: id.into(),
            matrix: matrix.iter().map(|s| s.to_string()).collect(),
            evidence: evidence.into(),
            status: Status::NotRun,
            blocker: Some(blocker.into()),
            known_defect: None,
            detail: Value::String(reason.into()),
        });
    }

    pub fn metric(&mut self, key: &str, v: Value) {
        self.metrics.insert(key.into(), v);
    }

    /// Failed checks that are not tracked known defects.
    pub fn failures(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|c| c.status == Status::Fail && c.known_defect.is_none())
            .map(|c| format!("{}: {}", c.id, c.detail))
            .collect()
    }

    /// Write `result.json`; panic (fail the test) if any automated check failed.
    pub fn finish(self) -> Value {
        let count = |s: Status| self.checks.iter().filter(|c| c.status == s).count();
        let (pass, fail, not_run) = (count(Status::Pass), count(Status::Fail), count(Status::NotRun));
        let status = if fail > 0 {
            "FAIL"
        } else if not_run > 0 {
            "AUTO_PASS"
        } else {
            "PASS"
        };
        let checks: Vec<Value> = self
            .checks
            .iter()
            .map(|c| {
                let mut o = json!({"id": c.id, "matrix": c.matrix, "evidence": c.evidence,
                                   "status": c.status.as_str(), "detail": c.detail});
                if let (Some(b), Some(m)) = (&c.blocker, o.as_object_mut()) {
                    m.insert("blocker".into(), json!(b));
                }
                if let (Some(k), Some(m)) = (&c.known_defect, o.as_object_mut()) {
                    m.insert("knownDefect".into(), json!(k));
                }
                o
            })
            .collect();
        let out = json!({
            "scenario": self.scenario,
            "title": self.title,
            "environment": "HOST (headless engine; synthetic input) - NOT device or Adobe evidence",
            "status": status,
            "counts": {"pass": pass, "fail": fail, "notRun": not_run,
                       "knownDefectFailures": self.checks.iter().filter(|c| c.status == Status::Fail && c.known_defect.is_some()).count()},
            "checks": checks,
            "metrics": self.metrics,
            "artifacts": self.artifacts,
        });
        let path = self.dir.join("result.json");
        let text = serde_json::to_string_pretty(&out).unwrap_or_default();
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        let failures = self.failures();
        assert!(failures.is_empty(), "{} failed checks:\n{}", self.scenario, failures.join("\n"));
        out
    }
}
