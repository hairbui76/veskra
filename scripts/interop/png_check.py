#!/usr/bin/env python3
"""Independent PNG oracle for Veskra exports (CLR-05, COL-001, COL-002).

Usage: png_check.py <expected.json> [<expected.json> ...]

Each JSON sidecar (written by crates/veskra-io/tests/png_export.rs) names the PNG, a raw straight
RGBA8 file with Veskra's composite, the size, the expected embedded ICC bytes (hex, or null) and
whether an sRGB chunk is expected. The PNG is decoded with Pillow (independent of Veskra's encoder
and of the Rust `png` crate). Checks:
  * size and RGBA mode;
  * `icc_profile` in Pillow's info equals the document ICC bytes (or is absent when not embedding);
  * `srgb` info present iff expected;
  * pixel values: max/mean absolute difference per channel vs the expected composite (threshold 0).
Prints one JSON object per file and exits non-zero if any check fails.
"""
import json
import sys

import numpy as np
from PIL import Image


def check(path):
    exp = json.load(open(path))
    res = {"name": exp["name"], "ok": True, "errors": []}
    im = Image.open(exp["png"])
    im.load()
    res["pillow_mode"] = im.mode
    res["size"] = list(im.size)
    if tuple(im.size) != (exp["width"], exp["height"]):
        res["errors"].append("size mismatch")
    icc = im.info.get("icc_profile")
    if exp["icc_hex"] is None:
        if icc:
            res["errors"].append("unexpected icc_profile")
    else:
        want = bytes.fromhex(exp["icc_hex"])
        res["icc_bytes"] = len(icc or b"")
        if icc != want:
            res["errors"].append("icc_profile bytes differ from document profile")
    has_srgb = "srgb" in im.info
    res["srgb_intent"] = im.info.get("srgb")
    if has_srgb != bool(exp["srgb_chunk"]):
        res["errors"].append("sRGB chunk presence mismatch")
    got = np.asarray(im.convert("RGBA"), dtype=np.int16)
    want = np.fromfile(exp["expected_rgba"], dtype=np.uint8).astype(np.int16)
    want = want.reshape(exp["height"], exp["width"], 4)
    diff = np.abs(got - want)
    res["max_abs_per_channel"] = [int(diff[..., c].max()) for c in range(4)]
    res["mean_abs_per_channel"] = [round(float(diff[..., c].mean()), 6) for c in range(4)]
    if diff.max() > 0:
        res["errors"].append("pixel mismatch")
    res["ok"] = not res["errors"]
    return res


def main():
    import PIL

    all_ok = True
    print(json.dumps({"tool": "Pillow", "version": PIL.__version__, "numpy": np.__version__}))
    for p in sys.argv[1:]:
        r = check(p)
        all_ok &= r["ok"]
        print(json.dumps(r))
    sys.exit(0 if all_ok else 1)


if __name__ == "__main__":
    main()
