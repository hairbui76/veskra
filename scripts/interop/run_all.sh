#!/usr/bin/env bash
# Unified independent-oracle runner (QAP; referenced by .github/workflows/ci.yml, job `interop`).
#
# NOT Adobe evidence: Photoshop/Illustrator checks stay NOT_RUN (docs/BLOCKERS.md B-3).
#
# Steps (each recorded as PASS / FAIL / NOT_RUN with exit code, duration and log):
#   1. tool versions                                   -> target/interop/tools.json
#   2. PNG fixtures + Pillow oracle (cargo test veskra-io png_export; then png_check.py directly)
#   3. PSD fixtures + psd-tools/ImageMagick (cargo test veskra-io psd_interop; then psd_check.py directly)
#   4. SVG/PDF fixtures + rsvg/poppler/qpdf + pypdf + Inkscape (scripts/interop/svg_pdf_oracles.sh)
#   5. QA scenarios S1-S8 + QAP-03 fault matrix (cargo test veskra-qa -> target/qa/*/result.json)
#   6. pypdf + Inkscape on the scenario SVG/PDF exports (target/qa/S4..S6)
#   7. Krita (xvfb-run when there is no DISPLAY) on the PSD fixtures and on the S3 12-layer PSD
#   8. Color oracle regeneration check: regenerated ICC + LittleCMS/colour-science fixtures must be
#      byte-identical to the committed ones (originals are restored if not), then the oracle test
#   9. summary                                         -> target/interop/summary.json
#
# Exit status: 1 if any step FAILs; 3 if every step passed but a scenario check failed because of a
# known, routed defect (`knownDefect` in target/qa/*/result.json; VESKRA_ALLOW_KNOWN_DEFECTS=1 → 0). A missing tool makes its step NOT_RUN (not a failure);
# set VESKRA_REQUIRE_ORACLES=1 (CI) to turn NOT_RUN for a missing tool into FAIL.
# Usage (repository root): bash scripts/interop/run_all.sh
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
CT="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT="$ROOT/target/interop"
LOGS="$OUT/logs"
STEPS="$OUT/steps.jsonl"
REQUIRE="${VESKRA_REQUIRE_ORACLES:-0}"
mkdir -p "$LOGS"
: > "$STEPS"
START_ALL=$(date +%s)

json_str() { python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }

record() { # name status exit seconds log detail
  printf '{"name":%s,"status":"%s","exit":%s,"seconds":%s,"log":%s,"detail":%s}\n' \
    "$(json_str "$1")" "$2" "$3" "$4" "$(json_str "$5")" "$(json_str "$6")" >> "$STEPS"
  printf '%-34s %-8s (%ss) %s\n' "$1" "$2" "$4" "$6"
}

step() { # name detail -- command...
  local name="$1" detail="$2"; shift 3
  local log="$LOGS/$name.log" t0 t1 rc
  t0=$(date +%s)
  ( "$@" ) >"$log" 2>&1
  rc=$?
  t1=$(date +%s)
  if [ $rc -eq 0 ]; then record "$name" PASS $rc $((t1 - t0)) "$log" "$detail"
  else record "$name" FAIL $rc $((t1 - t0)) "$log" "$detail (exit $rc; see log)"; fi
}

not_run() { # name reason
  if [ "$REQUIRE" = "1" ]; then record "$1" FAIL 2 0 "" "required oracle missing: $2"
  else record "$1" NOT_RUN 0 0 "" "$2"; fi
}

have() { command -v "$1" >/dev/null 2>&1; }
py_has() { python3 -I -c "$1" >/dev/null 2>&1; }

echo "== Veskra interop oracle run ($(date -u +%Y-%m-%dT%H:%M:%SZ)); CARGO_TARGET_DIR=$CT"

# ---- 1. tool versions -------------------------------------------------------------------------
python3 - "$OUT/tools.json" <<'PY'
import json, shutil, subprocess, sys
def first(cmd):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
        s = (p.stdout or p.stderr).strip().splitlines()
        return s[0] if s else ""
    except Exception as e:  # noqa: BLE001
        return f"NOT INSTALLED ({type(e).__name__})"
tools = {
    "rustc": first(["rustc", "--version"]),
    "python3": first(["python3", "--version"]),
    "rsvg-convert": first(["rsvg-convert", "--version"]),
    "pdftocairo": first(["pdftocairo", "-v"]),
    "qpdf": first(["qpdf", "--version"]),
    "inkscape": first(["inkscape", "--version"]),
    "imagemagick": first(["convert", "-version"]),
    "krita": first(["dpkg-query", "-W", "-f=${Version}", "krita"]) if shutil.which("krita") else "NOT INSTALLED",
    "xvfb-run": "present" if shutil.which("xvfb-run") else "NOT INSTALLED",
}
for mod in ["psd_tools", "PIL", "numpy", "pypdf", "colour"]:
    try:
        m = __import__(mod)
        tools[mod] = getattr(m, "__version__", "present")
    except Exception:  # noqa: BLE001
        tools[mod] = "NOT INSTALLED"
json.dump(tools, open(sys.argv[1], "w"), indent=2)
print(json.dumps(tools, indent=2))
PY

# ---- 2. PNG -----------------------------------------------------------------------------------
step rust_png_export "veskra-io png_export (writes $CT/interop/png, runs png_check.py when Pillow present)" -- \
  cargo test -p veskra-io --test png_export -- --nocapture
if py_has "import PIL, numpy"; then
  step png_check_pillow "scripts/interop/png_check.py on every PNG sidecar" -- \
    bash -c 'python3 -I scripts/interop/png_check.py "$0"/*.expected.json' "$CT/interop/png"
else not_run png_check_pillow "python3 Pillow/numpy not installed"; fi

# ---- 3. PSD -----------------------------------------------------------------------------------
step rust_psd_interop "veskra-io psd_interop (writes $CT/interop/psd, runs psd_check.py when psd-tools present)" -- \
  cargo test -p veskra-io --test psd_interop -- --nocapture
if py_has "import psd_tools, numpy, PIL"; then
  step psd_check_psd_tools "scripts/interop/psd_check.py (psd-tools + ImageMagick) on every fixture PSD" -- \
    bash -c 'rc=0; for f in "$0"/*.psd; do python3 -I scripts/interop/psd_check.py "$f" "${f%.psd}.expected.json" || rc=1; done; exit $rc' \
    "$CT/interop/psd"
else not_run psd_check_psd_tools "python3 psd_tools not installed"; fi

# ---- 4. SVG / PDF -----------------------------------------------------------------------------
if have rsvg-convert && have pdftocairo && py_has "import pypdf"; then
  step svg_pdf_oracles "scripts/interop/svg_pdf_oracles.sh (rsvg, poppler, qpdf, pypdf, Inkscape)" -- \
    bash scripts/interop/svg_pdf_oracles.sh
else not_run svg_pdf_oracles "rsvg-convert / pdftocairo / pypdf missing"; fi

# ---- 5. QA scenarios ----------------------------------------------------------------------------
step qa_scenarios "cargo test -p veskra-qa --test scenarios (S1-S8 -> target/qa/*/result.json)" -- \
  env VESKRA_QA_DIR="$ROOT/target/qa" cargo test -p veskra-qa --test scenarios -- --nocapture

step qa_fault_matrix "cargo test -p veskra-qa --test fault_matrix (QAP-03, 100 save cycles -> target/qa/QAP-03)" -- \
  env VESKRA_QA_DIR="$ROOT/target/qa" cargo test -p veskra-qa --test fault_matrix -- --nocapture

# ---- 6. oracles on scenario exports -----------------------------------------------------------
for s in S4 S5 S6; do
  d="$ROOT/target/qa/$s"
  [ -d "$d" ] || { record "qa_${s}_pypdf" FAIL 1 0 "" "missing $d (scenario step failed?)"; continue; }
  if py_has "import pypdf"; then
    step "qa_${s}_pypdf" "pdf_pypdf_check.py on $s PDFs" -- python3 -I scripts/interop/pdf_pypdf_check.py "$d"
  else not_run "qa_${s}_pypdf" "pypdf not installed"; fi
  if have inkscape && py_has "import numpy, PIL"; then
    step "qa_${s}_inkscape" "svg_pdf_inkscape_check.py on $s SVG/PDF" -- python3 -I scripts/interop/svg_pdf_inkscape_check.py "$d"
  else not_run "qa_${s}_inkscape" "inkscape not installed"; fi
done

# ---- 7. Krita -----------------------------------------------------------------------------------
if have krita && { [ -n "${DISPLAY:-}" ] || have xvfb-run; }; then
  step krita_fixtures "psd_krita_check.sh on $CT/interop/psd (renders; metrics in log)" -- \
    bash scripts/interop/psd_krita_check.sh "$CT/interop/psd"
  step krita_qa_s3 "psd_krita_check.sh on target/qa/S3 (12-layer scenario PSD)" -- \
    bash scripts/interop/psd_krita_check.sh "$ROOT/target/qa/S3"
else
  not_run krita_fixtures "krita (and DISPLAY or xvfb-run) not installed"
  not_run krita_qa_s3 "krita (and DISPLAY or xvfb-run) not installed"
fi

# ---- 8. color oracle regeneration --------------------------------------------------------------
if py_has "import PIL, numpy, colour"; then
  BK="$OUT/color-fixtures-backup"
  rm -rf "$BK"; mkdir -p "$BK"; cp -a tests/fixtures/color/. "$BK/"
  color_regen() {
    VESKRA_REGEN_COLOR_FIXTURES=1 cargo test -p veskra-color --test icc_fixtures || return 10
    python3 -I scripts/color/gen_oracle_fixtures.py || return 11
    if diff -r "$BK" tests/fixtures/color; then
      echo "regenerated color fixtures are byte-identical to the committed ones"
    else
      echo "REGENERATED COLOR FIXTURES DIFFER - restoring committed files"
      rm -rf tests/fixtures/color && mkdir -p tests/fixtures/color && cp -a "$BK/." tests/fixtures/color/
      return 12
    fi
  }
  step color_oracle_regen "ICC + LittleCMS/colour-science fixtures regenerate byte-identically" -- color_regen
  step color_oracle_test "cargo test -p veskra-color --test oracle (COL-002/003/008)" -- \
    cargo test -p veskra-color --test oracle -- --nocapture
else
  not_run color_oracle_regen "Pillow/numpy/colour-science not installed"
  not_run color_oracle_test "Pillow/numpy/colour-science not installed"
fi

# ---- 9. summary ---------------------------------------------------------------------------------
SUMMARY_OK=1
python3 - "$OUT" "$ROOT/target/qa" "$CT/interop" "$(( $(date +%s) - START_ALL ))" <<'PY' || SUMMARY_OK=0
import glob, json, os, subprocess, sys
out, qa, ct_interop, secs = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
steps = [json.loads(l) for l in open(os.path.join(out, "steps.jsonl")) if l.strip()]
def krita_lines(name):
    p = os.path.join(out, "logs", name + ".log")
    rows = []
    if os.path.exists(p):
        for l in open(p):
            l = l.strip()
            if l.startswith("{") and '"name"' in l:
                try: rows.append(json.loads(l))
                except ValueError: pass
    return rows
scen = {}
for f in sorted(glob.glob(os.path.join(qa, "*", "result.json"))):
    d = json.load(open(f))
    if "scenario" not in d or "checks" not in d:
        continue  # e.g. target/qa/fuzz/result.json (reported in docs/reports/qa-fuzz.md)
    scen[d["scenario"]] = {"status": d["status"], "counts": d["counts"],
                           "failed": [c["id"] + (" [" + c["knownDefect"] + "]" if c.get("knownDefect") else "")
                                      for c in d["checks"] if c["status"] == "FAIL"],
                           "notRun": [f'{c["id"]} ({c.get("blocker")})' for c in d["checks"] if c["status"] == "NOT_RUN"]}
def readj(p):
    try: return json.load(open(p))
    except Exception: return None  # noqa: BLE001
try:
    head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    dirty = bool(subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.strip())
except Exception:  # noqa: BLE001
    head, dirty = "unknown", None
counts = {k: sum(1 for s in steps if s["status"] == k) for k in ("PASS", "FAIL", "NOT_RUN")}
summary = {
    "evidence": "AUTO (host, open-source oracles). NOT Photoshop/Illustrator evidence (B-3); NOT device evidence (B-2).",
    "git": {"head": head, "dirtyWorkingTree": dirty},
    "seconds": secs,
    "tools": readj(os.path.join(out, "tools.json")),
    "steps": steps,
    "counts": counts,
    "scenarios": scen,
    "krita": {"fixtures": krita_lines("krita_fixtures"), "qa_s3": krita_lines("krita_qa_s3")},
    "illustrator_metrics": readj(os.path.join(out, "illustrator", "metrics.json")) is not None,
    "knownDefects": sorted({c.split("[")[1].rstrip("]") for v in scen.values() for c in v["failed"] if "[" in c}),
}
scen_fail = [k for k, v in scen.items() if v["status"] == "FAIL"]
summary["status"] = ("FAIL" if counts["FAIL"] else
                     "FAIL_KNOWN_DEFECTS" if scen_fail else
                     "PASS_WITH_NOT_RUN" if counts["NOT_RUN"] else "PASS")
summary["scenarioFailures"] = scen_fail
json.dump(summary, open(os.path.join(out, "summary.json"), "w"), indent=2)
print(json.dumps({"status": summary["status"], "counts": counts, "seconds": secs,
                  "knownDefects": summary["knownDefects"],
                  "scenarios": {k: v["status"] for k, v in scen.items()}}, indent=2))
open(os.path.join(out, "status.txt"), "w").write(summary["status"] + "\n")
PY
FAILS=$(grep -c '"status":"FAIL"' "$STEPS" || true)
echo "== summary: $OUT/summary.json (failed steps: $FAILS, summary written: $SUMMARY_OK)"
STATUS="$(cat "$OUT/status.txt" 2>/dev/null || echo UNKNOWN)"
echo "== overall: $STATUS"
[ "$FAILS" -eq 0 ] && [ "$SUMMARY_OK" -eq 1 ] || exit 1
# Scenario checks that fail because of a known, routed defect (`knownDefect` in result.json) do
# not fail `cargo test`, but they are failures: exit 3 unless explicitly allowed.
if [ "$STATUS" = "FAIL_KNOWN_DEFECTS" ] && [ "${VESKRA_ALLOW_KNOWN_DEFECTS:-0}" != "1" ]; then exit 3; fi
