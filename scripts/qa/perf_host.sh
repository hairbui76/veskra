#!/usr/bin/env bash
# Host CPU performance probes for docs/reports/performance-host.md.
# HOST CPU (container) — NOT device evidence; PERF-001..005 remain NOT_RUN (B-2).
#
# Every command runs under a small Python wrapper that records wall time and the peak RSS of the
# child process (getrusage RUSAGE_CHILDREN ru_maxrss), since /usr/bin/time may be absent.
# Output: target/qa/perf/<name>.log and target/qa/perf/perf_runs.jsonl
# Usage (repository root, release builds): bash scripts/qa/perf_host.sh
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
CT="${CARGO_TARGET_DIR:-$ROOT/target}"
OUT="$ROOT/target/qa/perf"
mkdir -p "$OUT"
RUNS="$OUT/perf_runs.jsonl"
: > "$RUNS"

{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "nproc: $(nproc)"
  lscpu | grep -E "Model name|^CPU\(s\)|Thread|MHz|L2|L3" || true
  echo "loadavg_before: $(cat /proc/loadavg)"
  free -m | head -2
  rustc --version
  git rev-parse HEAD 2>/dev/null
} > "$OUT/machine.txt"
cat "$OUT/machine.txt"

cargo build --release -p veskra-cli >/dev/null 2>&1 || { echo "release build failed"; exit 1; }
VESKRA="$CT/release/veskra"
# Pre-build test binaries so the measured runs do not include compilation.
cargo test --release -p veskra-compose -p veskra-brush -p veskra-raster -p veskra-qa --no-run >/dev/null 2>&1

measure() { # name -- command...
  local name="$1"; shift 2
  python3 - "$name" "$OUT/$name.log" "$RUNS" "$@" <<'PY'
import json, resource, subprocess, sys, time
name, log, runs, cmd = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
load = open("/proc/loadavg").read().split()[0]
t = time.time()
with open(log, "w") as f:
    rc = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT).returncode
wall = time.time() - t
rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss  # KiB on Linux
rec = {"name": name, "cmd": " ".join(cmd), "exit": rc, "wallSeconds": round(wall, 3),
       "peakRssMiB": round(rss / 1024, 1), "loadavg1Before": load}
open(runs, "a").write(json.dumps(rec) + "\n")
print(json.dumps(rec))
PY
}

for b in pencil.rough pencil.smooth pen.technical pen.brush marker charcoal round.hard round.soft eraser.hard; do
  measure "stroke-$b-2048" -- "$VESKRA" benchmark-stroke --size 2048 --samples 2000 --brush "$b"
done
for s in 1024 4096; do
  measure "stroke-pen.brush-$s" -- "$VESKRA" benchmark-stroke --size "$s" --samples 2000 --brush pen.brush
done
measure stroke-round.soft-4096-8000 -- "$VESKRA" benchmark-stroke --size 4096 --samples 8000 --brush round.soft
measure compose_golden_perf -- cargo test -p veskra-compose --release --test golden_perf -- --ignored --nocapture
measure brush_perf -- cargo test -p veskra-brush --release --test perf -- --ignored --nocapture
measure raster_benches -- cargo test -p veskra-raster --release -- --ignored --nocapture
measure qa_perf_host_2048_12_layer -- cargo test -p veskra-qa --release --test perf_host -- --ignored --nocapture
echo "loadavg_after: $(cat /proc/loadavg)" >> "$OUT/machine.txt"
echo "== results: $RUNS"
