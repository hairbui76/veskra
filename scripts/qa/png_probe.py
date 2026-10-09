#!/usr/bin/env python3
"""Independent PNG probe for QA scenario S7 (COL-001/002/003): decode with Pillow.

Usage: png_probe.py <file.png> <points.json>
  points.json: [[x, y], ...] pixel coordinates.
Prints one JSON object: Pillow version, mode, size, embedded ICC (length + SHA-256),
sRGB chunk intent, and the straight RGBA8 value at every requested point.
Pillow's decoder is independent of Veskra's PNG writer and of the Rust `png` crate.
"""
import hashlib
import json
import sys

import PIL
from PIL import Image


def main() -> int:
    png, pts_path = sys.argv[1], sys.argv[2]
    pts = json.load(open(pts_path))
    im = Image.open(png)
    im.load()
    icc = im.info.get("icc_profile")
    rgba = im.convert("RGBA")
    out = {
        "pillow": PIL.__version__,
        "mode": im.mode,
        "size": list(im.size),
        "icc_len": len(icc) if icc else 0,
        "icc_sha256": hashlib.sha256(icc).hexdigest() if icc else None,
        "srgb_intent": im.info.get("srgb"),
        "pixels": [list(rgba.getpixel((int(x), int(y)))) for x, y in pts],
    }
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
