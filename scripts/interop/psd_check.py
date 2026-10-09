#!/usr/bin/env python3
"""Independent PSD oracle for Veskra exports (PSD-001, PSD-003, PSD-004, PSD-005; FMT-11 automated part).

Usage: psd_check.py <file.psd> <expected.json>

`expected.json` is written by crates/veskra-io/tests/psd_interop.rs:
  {"name", "width", "height", "composite_rgba": <raw straight RGBA8 of Veskra's composite>,
   "icc_hex": <document ICC bytes or null>, "layers": [<tree, bottom -> top>]}
  layer node: {"name", "group": bool, "visible": bool, "opacity": 0..255, "blend": "NORMAL"|...,
               "clipping": bool, "children": [...]}

Checks with psd-tools (independent decoder + compositor):
  * opens; size; layer tree (names, order, groups, visibility, opacity, blend modes, clipping);
  * ICC image resource 1039 bytes == document profile (or absent when not embedded);
  * merged image `psd.topil(apply_icc=False)` (psd-tools un-mattes Photoshop's white matte) vs the
    Veskra composite;
  * psd-tools' own recomposite `psd.composite(ignore_preview=True, apply_icc=False)` vs Veskra;
  * if ImageMagick `convert` exists: `convert file.psd[0]` (merged image) vs Veskra.
Image metrics (all 8-bit): alpha |dA| max/mean; premultiplied color |C*A/255| max/mean (the
quantity a viewer composites, insensitive to un-matting noise at tiny alpha); straight color
max/mean over pixels opaque in both images. Prints one JSON object; exit 1 if the tree/ICC check
fails or the file cannot be opened (image thresholds are asserted by the Rust test).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

import numpy as np
import psd_tools
from PIL import Image
from psd_tools import PSDImage
from psd_tools.constants import Resource


def metrics(got, want):
    got = got.astype(np.int32)
    want = want.astype(np.int32)
    da = np.abs(got[..., 3] - want[..., 3])
    pg = got[..., :3] * got[..., 3:4] / 255.0
    pw = want[..., :3] * want[..., 3:4] / 255.0
    dp = np.abs(pg - pw)
    both = (got[..., 3] == 255) & (want[..., 3] == 255)
    dc = np.abs(got[..., :3] - want[..., :3])[both]
    return {
        "alpha_max": int(da.max()),
        "alpha_mean": round(float(da.mean()), 5),
        "premul_max": round(float(dp.max()), 3),
        "premul_mean": round(float(dp.mean()), 5),
        "premul_pixels_over_2": int((dp.max(axis=2) > 2).sum()),
        "opaque_color_max": int(dc.max()) if dc.size else 0,
        "opaque_color_mean": round(float(dc.mean()), 5) if dc.size else 0.0,
        "opaque_pixels": int(both.sum()),
    }


def as_rgba(im, size):
    if im is None:
        return np.zeros((size[1], size[0], 4), dtype=np.uint8)
    if im.mode != "RGBA":
        im = im.convert("RGBA")
    return np.asarray(im)


def tree(layers):
    out = []
    for layer in layers:  # psd-tools iterates bottom -> top
        node = {
            "name": layer.name,
            "group": bool(layer.is_group()),
            "visible": bool(layer.visible),
            "opacity": int(layer.opacity),
            "blend": layer.blend_mode.name,
            "clipping": bool(layer.clipping),
        }
        node["children"] = tree(layer) if layer.is_group() else []
        out.append(node)
    return out


def diff_tree(got, want, path, errors):
    if len(got) != len(want):
        errors.append(f"{path}: {len(got)} layers, expected {len(want)}: {[g['name'] for g in got]}")
        return
    for g, w in zip(got, want):
        p = f"{path}/{w['name']}"
        for k in ("name", "group", "visible", "opacity", "blend", "clipping"):
            if g[k] != w[k]:
                errors.append(f"{p}: {k} = {g[k]!r}, expected {w[k]!r}")
        diff_tree(g["children"], w["children"], p, errors)


def main():
    psd_path, exp_path = sys.argv[1], sys.argv[2]
    exp = json.load(open(exp_path))
    res = {"name": exp["name"], "psd_tools": psd_tools.__version__, "errors": []}
    w, h = exp["width"], exp["height"]
    want = np.fromfile(exp["composite_rgba"], dtype=np.uint8).reshape(h, w, 4)
    try:
        psd = PSDImage.open(psd_path)
    except Exception as e:  # noqa: BLE001
        res["errors"].append(f"psd-tools failed to open: {e!r}")
        print(json.dumps(res))
        sys.exit(1)
    res["size"] = [psd.width, psd.height]
    res["channels"] = psd.channels
    if (psd.width, psd.height) != (w, h):
        res["errors"].append("size mismatch")
    got_tree = tree(psd)
    diff_tree(got_tree, exp["layers"], "", res["errors"])
    res["layer_tree_ok"] = not res["errors"]
    icc = psd.image_resources.get_data(Resource.ICC_PROFILE) if Resource.ICC_PROFILE in psd.image_resources else None
    if exp["icc_hex"] is None:
        res["icc_ok"] = icc is None
    else:
        res["icc_ok"] = icc == bytes.fromhex(exp["icc_hex"])
        res["icc_bytes"] = len(icc or b"")
    if not res["icc_ok"]:
        res["errors"].append("ICC resource 1039 mismatch")

    res["merged_psd_tools"] = metrics(as_rgba(psd.topil(apply_icc=False), (w, h)), want)
    try:
        comp = psd.composite(ignore_preview=True, apply_icc=False)
        res["recomposite_psd_tools"] = metrics(as_rgba(comp, (w, h)), want)
    except Exception as e:  # noqa: BLE001
        res["errors"].append(f"psd-tools recomposite failed: {e!r}")

    if shutil.which("convert"):
        with tempfile.TemporaryDirectory() as d:
            out = os.path.join(d, "merged.png")
            p = subprocess.run(["convert", f"{psd_path}[0]", out], capture_output=True, text=True)
            if p.returncode == 0 and os.path.exists(out):
                im = Image.open(out)
                im.load()
                res["merged_imagemagick"] = metrics(as_rgba(im, (w, h)), want)
            else:
                res["errors"].append(f"ImageMagick failed: {p.stderr.strip()[:200]}")
    print(json.dumps(res))
    sys.exit(0 if not res["errors"] else 1)


if __name__ == "__main__":
    main()
