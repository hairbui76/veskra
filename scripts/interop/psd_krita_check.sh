#!/usr/bin/env bash
# Krita headless PSD check (FMT-11, PSD-002 "manual Krita" part, automated where possible).
#
# Usage: scripts/interop/psd_krita_check.sh [interop_dir]
#   interop_dir defaults to ${CARGO_TARGET_DIR:-target}/interop/psd and must contain the files written
#   by `cargo test -p veskra-io --test psd_interop` (<name>.psd, <name>.composite.rgba, <name>.expected.json).
#
# Krita 5.2.2's Qt ignores QT_QPA_PLATFORM=offscreen here (it insists on xcb), so without a $DISPLAY
# the export runs under `xvfb-run -a`. Each export takes ~20 s (Krita start-up).
#
# For each PSD: `krita --export --export-filename <name>.krita.png <name>.psd` (Krita re-composites
# the layers itself; it ignores the merged image), then compares Krita's render with Veskra's
# composite using the same metrics as psd_check.py. Prints one JSON line per file.
set -euo pipefail
DIR="${1:-${CARGO_TARGET_DIR:-target}/interop/psd}"
KHOME="$(mktemp -d)"
trap 'rm -rf "$KHOME"' EXIT
command -v krita >/dev/null || { echo '{"error":"krita not installed"}'; exit 2; }
WRAP=()
if [ -z "${DISPLAY:-}" ]; then
  command -v xvfb-run >/dev/null || { echo '{"error":"no DISPLAY and no xvfb-run"}'; exit 2; }
  WRAP=(xvfb-run -a)
fi
echo "{\"tool\":\"krita\",\"version\":\"$(dpkg-query -W -f='${Version}' krita 2>/dev/null || echo unknown)\"}"
status=0
for psd in "$DIR"/*.psd; do
  name="$(basename "$psd" .psd)"
  out="$DIR/$name.krita.png"
  rm -f "$out"
  start=$(date +%s.%N)
  if ! HOME="$KHOME" timeout 300 "${WRAP[@]}" krita --export --export-filename "$out" "$psd" \
      >"$DIR/$name.krita.log" 2>&1 || [ ! -s "$out" ]; then
    echo "{\"name\":\"$name\",\"ok\":false,\"error\":\"krita export failed (see $name.krita.log)\"}"
    status=1
    continue
  fi
  secs=$(python3 -c "import sys; print(float(sys.argv[2]) - float(sys.argv[1]))" "$start" "$(date +%s.%N)")
  python3 - "$out" "$DIR/$name.expected.json" "$secs" <<'PY' || status=1
import json, sys
import numpy as np
from PIL import Image
out, exp_path, secs = sys.argv[1], sys.argv[2], float(sys.argv[3])
exp = json.load(open(exp_path))
w, h = exp["width"], exp["height"]
want = np.fromfile(exp["composite_rgba"], dtype=np.uint8).reshape(h, w, 4).astype(np.int32)
im = Image.open(out)
im.load()
res = {"name": exp["name"], "seconds": round(secs, 1), "krita_mode": im.mode, "size": list(im.size),
       "krita_png_has_icc": bool(im.info.get("icc_profile"))}
got = np.asarray(im.convert("RGBA")).astype(np.int32)
if got.shape != want.shape:
    res["ok"] = False
    res["error"] = "size mismatch"
    print(json.dumps(res)); sys.exit(1)
da = np.abs(got[..., 3] - want[..., 3])
dp = np.abs(got[..., :3] * got[..., 3:4] / 255.0 - want[..., :3] * want[..., 3:4] / 255.0)
both = (got[..., 3] == 255) & (want[..., 3] == 255)
dc = np.abs(got[..., :3] - want[..., :3])[both]
res.update({
    "alpha_max": int(da.max()), "alpha_mean": round(float(da.mean()), 5),
    "premul_max": round(float(dp.max()), 3), "premul_mean": round(float(dp.mean()), 5),
    "premul_pixels_over_2": int((dp.max(axis=2) > 2).sum()),
    "opaque_color_max": int(dc.max()) if dc.size else 0,
    "opaque_color_mean": round(float(dc.mean()), 5) if dc.size else 0.0,
})
res["ok"] = True
print(json.dumps(res))
PY
done
exit $status
