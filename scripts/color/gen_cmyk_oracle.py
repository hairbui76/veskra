#!/usr/bin/env python3
"""Generate the independent LittleCMS oracle for Veskra's CMYK output (ADR-018).

Oracle (test-time tool only; nothing here ships in Veskra): LittleCMS 2 through Pillow's
``PIL.ImageCms``. Only Pillow (pinned in scripts/requirements-oracles.txt) is needed.

Inputs:
  crates/veskra-color/profiles/Coated_Fogra39L_VIGC_300.icc   bundled CMYK output profile (unaltered)
  tests/fixtures/color/icc/veskra-srgb-v4.icc                 Veskra's generated sRGB (the source space)

Outputs (deterministic for a given tool-version set; versions are recorded in the JSON):
  tests/fixtures/color/icc/veskra-synthetic-cmyk-v4.icc       small synthetic ICC v4 CMYK profile written
                                                              by this script (mAB/mBA with curv + para
                                                              curves, matrix + offset, 16-bit and 8-bit
                                                              CLUTs, non-uniform grid) to test the v4 parser
                                                              against LittleCMS
  tests/fixtures/color/cmyk_expected.json                     every oracle value consumed by
                                                              crates/veskra-color/tests/cmyk_oracle.rs

What is computed (all 8-bit, because Pillow exposes only 8-bit RGB / CMYK / Lab buffers):
  * sRGB -> CMYK for >= 512 swatches, intents relative colorimetric and perceptual, with and without
    black point compensation, with LittleCMS's default optimisation (33^3 device link) and with
    cmsFLAGS_NOOPTIMIZE (full-precision pipeline);
  * CMYK -> Lab (relative and perceptual, NOOPTIMIZE) for a 5^4 CMYK grid and for LittleCMS's own CMYK
    of every swatch (the "round trip");
  * CMYK -> sRGB (relative + BPC, relative, perceptual + BPC; NOOPTIMIZE) for the same CMYK sets;
  * the synthetic v4 profile: sRGB -> CMYK and CMYK -> Lab (relative, i.e. the A2B0/B2A0 fallback).

Usage (from the repository root):
  python3 -I scripts/color/gen_cmyk_oracle.py
  cargo test -p veskra-color --test cmyk_oracle -- --nocapture
"""

from __future__ import annotations

import hashlib
import io
import json
import platform
import re
import struct
import sys
from pathlib import Path

import PIL
from PIL import Image, ImageCms

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "tests" / "fixtures" / "color"
PROFILE = ROOT / "crates" / "veskra-color" / "profiles" / "Coated_Fogra39L_VIGC_300.icc"
PROFILE_SHA256 = "6df43849a84d2632e7900a125f657bd09364ed56775bc4740c1e39a5492d47e3"
SRGB = FIX / "icc" / "veskra-srgb-v4.icc"
SYNTH = FIX / "icc" / "veskra-synthetic-cmyk-v4.icc"
OUT = FIX / "cmyk_expected.json"


# ------------------------------------------------------------------------------------------------
# Swatches


def swatches() -> list[tuple[str, str, tuple[int, int, int]]]:
    """Primaries, secondaries, neutral ramp, near-black, near-white, skin-like, desaturated,
    saturated, out-of-CMYK-gamut saturated colors, the 5^3 grid of the RGB oracle and an 8^3 grid.
    Duplicates removed (first occurrence wins)."""
    levels8 = (0, 36, 73, 109, 146, 182, 219, 255)
    groups: list[tuple[str, list[tuple[int, int, int]]]] = [
        ("primary", [(255, 0, 0), (0, 255, 0), (0, 0, 255), (128, 0, 0), (0, 128, 0), (0, 0, 128)]),
        ("secondary", [(0, 255, 255), (255, 0, 255), (255, 255, 0), (0, 128, 128), (128, 0, 128), (128, 128, 0)]),
        ("neutral", [(v, v, v) for v in (0, 1, 2, 3, 5, 8, 12, 17, 23, 32, 45, 64, 80, 96, 112, 128, 144, 160,
                                         176, 192, 208, 224, 240, 250, 253, 254, 255)]),
        ("near-black", [(1, 0, 0), (0, 1, 0), (0, 0, 1), (2, 1, 3), (5, 3, 1), (8, 8, 4), (10, 0, 12), (16, 8, 0),
                        (4, 2, 0), (0, 2, 4), (12, 6, 9), (20, 18, 22), (6, 6, 6), (10, 10, 10)]),
        ("near-white", [(255, 255, 254), (254, 255, 255), (255, 254, 255), (252, 252, 252), (255, 250, 245),
                        (245, 250, 255), (250, 255, 250), (248, 244, 236), (240, 240, 240), (235, 240, 245)]),
        ("skin-like", [(255, 224, 196), (241, 194, 167), (224, 172, 105), (198, 134, 66), (141, 85, 36),
                       (92, 51, 23), (255, 205, 148), (234, 192, 134), (255, 173, 96), (197, 140, 133),
                       (161, 102, 94), (80, 51, 53)]),
        ("desaturated", [(120, 130, 125), (140, 120, 110), (100, 110, 140), (180, 170, 150), (90, 100, 95),
                         (200, 190, 210), (60, 70, 65), (150, 160, 170)]),
        ("saturated", [(200, 30, 40), (30, 200, 60), (40, 60, 220), (250, 140, 0), (140, 0, 250), (0, 180, 180),
                       (230, 20, 140), (120, 220, 20)]),
        ("out-of-gamut", [(0, 255, 64), (0, 255, 128), (0, 255, 192), (0, 200, 255), (0, 128, 255), (0, 64, 255),
                          (64, 0, 255), (128, 0, 255), (192, 0, 255), (255, 0, 192), (255, 0, 128), (255, 0, 64),
                          (64, 255, 0), (128, 255, 0), (30, 255, 30), (30, 30, 255), (255, 60, 0), (0, 230, 230)]),
        ("grid5", [(r, g, b) for r in (0, 64, 128, 192, 255) for g in (0, 64, 128, 192, 255)
                   for b in (0, 64, 128, 192, 255)]),
        ("grid8", [(r, g, b) for r in levels8 for g in levels8 for b in levels8]),
    ]
    seen: set[tuple[int, int, int]] = set()
    out = []
    for cat, cols in groups:
        for c in cols:
            if c in seen:
                continue
            seen.add(c)
            out.append((f"{cat}-{len(out):03d}", cat, c))
    return out


def cmyk_grid() -> list[tuple[int, int, int, int]]:
    lv = (0, 64, 128, 191, 255)
    return [(c, m, y, k) for c in lv for m in lv for y in lv for k in lv]


# ------------------------------------------------------------------------------------------------
# LittleCMS helpers (Pillow)

I = ImageCms.Intent
F = ImageCms.Flags


def run(src, dst, in_mode: str, out_mode: str, pixels: list, intent, flags) -> list[list[int]]:
    t = ImageCms.buildTransform(src, dst, in_mode, out_mode, renderingIntent=intent, flags=flags)
    im = Image.new(in_mode, (len(pixels), 1))
    im.putdata([tuple(p) for p in pixels])
    out = ImageCms.applyTransform(im, t)
    return [list(out.getpixel((i, 0))) for i in range(len(pixels))]


def lab_decode(v: list[int]) -> list[float]:
    """LittleCMS 8-bit Lab (Pillow "LAB"): L = v * 100 / 255, a = v - 128, b = v - 128."""
    return [v[0] * 100.0 / 255.0, float(v[1] - 128), float(v[2] - 128)]


# ------------------------------------------------------------------------------------------------
# Synthetic ICC v4 CMYK profile (deterministic bytes)


def s15(v: float) -> bytes:
    return struct.pack(">i", int(round(v * 65536.0)))


def pad4(b: bytes) -> bytes:
    return b + b"\0" * (-len(b) % 4)


def curv(entries: list[int]) -> bytes:
    return pad4(b"curv\0\0\0\0" + struct.pack(">I", len(entries)) + b"".join(struct.pack(">H", e) for e in entries))


def para(ftype: int, params: list[float]) -> bytes:
    return pad4(b"para\0\0\0\0" + struct.pack(">HH", ftype, 0) + b"".join(s15(p) for p in params))


def matrix(m: list[float], off: list[float]) -> bytes:
    return b"".join(s15(v) for v in m) + b"".join(s15(v) for v in off)


def clut(grid: list[int], out_ch: int, precision: int, fn) -> bytes:
    """CLUT body (grid points[16], precision, 3 pad, data); first input varies slowest."""
    head = bytes(grid + [0] * (16 - len(grid))) + bytes([precision, 0, 0, 0])
    data = bytearray()

    def rec(prefix: list[float], dim: int):
        if dim == len(grid):
            for v in fn(prefix):
                v = min(max(v, 0.0), 1.0)
                if precision == 1:
                    data.append(int(round(v * 255.0)))
                else:
                    data.extend(struct.pack(">H", int(round(v * 65535.0))))
            return
        n = grid[dim]
        for i in range(n):
            rec(prefix + [i / (n - 1)], dim + 1)

    rec([], 0)
    return pad4(head + bytes(data))


def lut_ab(sig: bytes, in_ch: int, out_ch: int, parts: dict[str, bytes], order: list[str]) -> bytes:
    """mAB / mBA tag: header (32 bytes) then the parts in `order`, offsets from the tag start.
    Header offsets are always listed as B, matrix, M, CLUT, A."""
    body = b""
    offsets = {}
    pos = 32
    for name in order:
        offsets[name] = pos
        body += parts[name]
        pos += len(parts[name])
    head = sig + b"\0\0\0\0" + bytes([in_ch, out_ch, 0, 0])
    for name in ("B", "matrix", "M", "clut", "A"):
        head += struct.pack(">I", offsets.get(name, 0))
    return head + body


def mluc(text: str) -> bytes:
    s = text.encode("utf-16-be")
    return pad4(b"mluc\0\0\0\0" + struct.pack(">II", 1, 12) + b"enUS" + struct.pack(">II", len(s), 28) + s)


def xyz_tag(v: list[float]) -> bytes:
    return b"XYZ \0\0\0\0" + b"".join(s15(x) for x in v)


def synth_a2b(cmyk: list[float]) -> list[float]:
    """Smooth subtractive toy model -> normalised v4 Lab (L/100, (a+128)/255, (b+128)/255)."""
    c, m, y, k = cmyk
    L = 96.0 * (1 - 0.55 * c) * (1 - 0.6 * m) * (1 - 0.25 * y) * (1 - 0.85 * k) + 2.0
    a = 60.0 * m - 40.0 * c - 8.0 * y * (1 - m)
    b = 70.0 * y - 45.0 * c - 15.0 * m
    return [L / 100.0, (a + 128.0) / 255.0, (b + 128.0) / 255.0]


def synth_b2a(lab: list[float]) -> list[float]:
    """Arbitrary smooth PCS -> CMYK map (needs to be well-defined, not an exact inverse)."""
    L, a, b = lab
    k = max(0.0, 0.7 - L) * 0.9
    c = 0.5 * (1 - L) + 0.4 * max(0.0, 0.5 - a)
    m = 0.5 * (1 - L) + 0.5 * max(0.0, a - 0.5)
    y = 0.4 * (1 - L) + 0.6 * max(0.0, b - 0.5) + 0.1 * a
    return [c, m, y, k]


def synthetic_profile() -> bytes:
    a2b = lut_ab(b"mAB ", 4, 3, {
        "A": curv([256 * 2 - 52]) + para(0, [1.0]) + para(3, [2.4, 1 / 1.055, 0.055 / 1.055, 1 / 12.92, 0.04045])
             + curv([0, 9000, 30000, 52000, 65535]),
        "clut": clut([3, 4, 3, 5], 3, 2, synth_a2b),
        "M": para(1, [1.2, 1.0, 0.0]) + curv([]) + curv([0, 30000, 65535]),
        "matrix": matrix([1.0, 0.02, 0.0, 0.0, 0.98, 0.01, 0.0, 0.0, 1.0], [0.001, -0.002, 0.0]),
        "B": para(2, [1.1, 0.95, 0.02, 0.01]) + curv([]) + para(4, [2.2, 0.9, 0.1, 0.05, 0.03, 0.0, 0.01]),
    }, ["A", "clut", "M", "matrix", "B"])
    b2a = lut_ab(b"mBA ", 3, 4, {
        "B": curv([]) + para(0, [1.1]) + curv([0, 20000, 41000, 65535]),
        "matrix": matrix([1.0, 0.0, 0.0, 0.01, 1.0, 0.0, 0.0, 0.0, 0.99], [0.0, 0.001, 0.0]),
        "M": curv([]) + curv([]) + curv([]),
        "clut": clut([5, 4, 3], 4, 1, synth_b2a),
        "A": para(3, [2.0, 1.0, 0.0, 0.5, 0.1]) + curv([0, 40000, 65535]) + curv([256 + 64]) + curv([]),
    }, ["B", "matrix", "M", "clut", "A"])
    tags = [
        (b"desc", mluc("Veskra synthetic CMYK v4 (test only)")),
        (b"cprt", mluc("No copyright, use freely")),
        (b"wtpt", xyz_tag([0.9642, 1.0, 0.8249])),
        (b"A2B0", pad4(a2b)),
        (b"B2A0", pad4(b2a)),
    ]
    table_len = 4 + 12 * len(tags)
    offset = 128 + table_len
    entries = b""
    data = b""
    for sig, body in tags:
        entries += sig + struct.pack(">II", offset + len(data), len(body))
        data += body
    size = 128 + table_len + len(data)
    hdr = bytearray(128)
    hdr[0:4] = struct.pack(">I", size)
    hdr[8:12] = bytes([4, 0x30, 0, 0])
    hdr[12:16] = b"prtr"
    hdr[16:20] = b"CMYK"
    hdr[20:24] = b"Lab "
    hdr[24:36] = bytes([0x07, 0xEA, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0])
    hdr[36:40] = b"acsp"
    hdr[64:68] = struct.pack(">I", 1)
    hdr[68:80] = s15(0.9642) + s15(1.0) + s15(0.8249)
    return bytes(hdr) + struct.pack(">I", len(tags)) + entries + data


# ------------------------------------------------------------------------------------------------


def dump_json(doc) -> str:
    """Indented JSON with numeric arrays kept on one line (diff-friendly)."""
    text = json.dumps(doc, indent=1)
    text = re.sub(r"\[\s*([-0-9.eE,\s]+?)\s*\]",
                  lambda m: "[" + ",".join(x.strip() for x in m.group(1).split(",")) + "]", text)
    return text + "\n"


def main() -> int:
    data = PROFILE.read_bytes()
    sha = hashlib.sha256(data).hexdigest()
    if sha != PROFILE_SHA256:
        print(f"bundled profile hash {sha} != {PROFILE_SHA256}: the file must not be altered", file=sys.stderr)
        return 2
    if not SRGB.exists():
        print(f"missing {SRGB}", file=sys.stderr)
        return 2
    srgb = ImageCms.ImageCmsProfile(str(SRGB))
    cmyk = ImageCms.ImageCmsProfile(io.BytesIO(data))
    lab = ImageCms.ImageCmsProfile(ImageCms.createProfile("LAB"))

    # Self-check of the Lab decoding: white and black.
    wb = run(srgb, lab, "RGB", "LAB", [(255, 255, 255), (0, 0, 0)], I.RELATIVE_COLORIMETRIC, F.NOOPTIMIZE)
    if wb[0] != [255, 128, 128] or wb[1][0] != 0:
        print(f"unexpected 8-bit Lab encoding: {wb}", file=sys.stderr)
        return 3

    sw = swatches()
    rgb = [c for _, _, c in sw]
    intents = [("relative-colorimetric", I.RELATIVE_COLORIMETRIC), ("perceptual", I.PERCEPTUAL)]
    rgb_to_cmyk = []
    for iname, intent in intents:
        for bpc in (True, False):
            for fname, fl in (("default", F.NONE), ("NOOPTIMIZE", F.NOOPTIMIZE)):
                flags = fl | (F.BLACKPOINTCOMPENSATION if bpc else F.NONE)
                rgb_to_cmyk.append({
                    "intent": iname, "bpc": bpc, "lcms_flags": fname,
                    "cmyk8": run(srgb, cmyk, "RGB", "CMYK", rgb, intent, flags),
                })

    ref = next(t for t in rgb_to_cmyk if t["intent"] == "relative-colorimetric" and t["bpc"]
               and t["lcms_flags"] == "NOOPTIMIZE")["cmyk8"]
    grid = [list(c) for c in cmyk_grid()]
    cmyk_to_lab = []
    for iname, intent in intents:
        cmyk_to_lab.append({
            "set": "grid", "intent": iname,
            "lab": [lab_decode(v) for v in run(cmyk, lab, "CMYK", "LAB", grid, intent, F.NOOPTIMIZE)],
        })
    cmyk_to_lab.append({
        "set": "swatch_roundtrip", "intent": "relative-colorimetric",
        "lab": [lab_decode(v) for v in run(cmyk, lab, "CMYK", "LAB", ref, I.RELATIVE_COLORIMETRIC, F.NOOPTIMIZE)],
    })
    cmyk_to_srgb = []
    for iname, intent, bpc in (("relative-colorimetric", I.RELATIVE_COLORIMETRIC, True),
                               ("relative-colorimetric", I.RELATIVE_COLORIMETRIC, False),
                               ("perceptual", I.PERCEPTUAL, True)):
        flags = F.NOOPTIMIZE | (F.BLACKPOINTCOMPENSATION if bpc else F.NONE)
        cmyk_to_srgb.append({"intent": iname, "bpc": bpc, "lcms_flags": "NOOPTIMIZE",
                             "rgb8": run(cmyk, srgb, "CMYK", "RGB", grid, intent, flags)})

    synth_bytes = synthetic_profile()
    SYNTH.write_bytes(synth_bytes)
    synth = ImageCms.ImageCmsProfile(io.BytesIO(synth_bytes))
    synth_doc = {
        "file": str(SYNTH.relative_to(FIX)).replace("\\", "/"),
        "bytes": len(synth_bytes),
        "sha256": hashlib.sha256(synth_bytes).hexdigest(),
        "lcms_description": ImageCms.getProfileDescription(synth).strip(),
        "rgb_to_cmyk_relative": run(srgb, synth, "RGB", "CMYK", rgb, I.RELATIVE_COLORIMETRIC, F.NOOPTIMIZE),
        "cmyk_to_lab_relative": [lab_decode(v) for v in run(synth, lab, "CMYK", "LAB", grid,
                                                            I.RELATIVE_COLORIMETRIC, F.NOOPTIMIZE)],
    }

    p = cmyk.profile
    doc = {
        "generator": "scripts/color/gen_cmyk_oracle.py",
        # The Python version is printed but not recorded: the numbers depend only on Pillow /
        # LittleCMS, so regenerating with the pinned Pillow is byte-identical on any Python.
        "tools": {"pillow": PIL.__version__, "littlecms": ImageCms.core.littlecms_version},
        "notes": "All LittleCMS buffers are 8-bit (Pillow): CMYK 0..255 = 0..100 % ink (not inverted); "
                 "Lab decoded from 8-bit as L = v*100/255, a/b = v-128 (quantisation: 0.39 L*, 1 a*/b*).",
        "source_profile": {"file": str(SRGB.relative_to(FIX)).replace("\\", "/"),
                           "lcms_description": ImageCms.getProfileDescription(srgb).strip()},
        "profile": {
            "file": "crates/veskra-color/profiles/Coated_Fogra39L_VIGC_300.icc",
            "bytes": len(data),
            "sha256": sha,
            "lcms_description": ImageCms.getProfileDescription(cmyk).strip(),
            "lcms_copyright": (p.copyright or "").strip(),
            "lcms_version": p.version,
            "lcms_media_white_point": list(p.media_white_point[0]),
            "lcms_intents_supported_output": {
                name: bool(ImageCms.isIntentSupported(cmyk, intent, ImageCms.Direction.OUTPUT))
                for name, intent in [("perceptual", I.PERCEPTUAL),
                                     ("relative-colorimetric", I.RELATIVE_COLORIMETRIC),
                                     ("saturation", I.SATURATION)]
            },
        },
        "swatches": [{"id": sid, "category": cat, "rgb8": list(c)} for sid, cat, c in sw],
        "rgb_to_cmyk": rgb_to_cmyk,
        "cmyk_grid": grid,
        "cmyk_to_lab": cmyk_to_lab,
        "cmyk_to_srgb": cmyk_to_srgb,
        "synthetic_v4": synth_doc,
    }
    OUT.write_text(dump_json(doc), encoding="utf-8", newline="\n")
    print(f"tools: {doc['tools']} (python {platform.python_version()})")
    print(f"swatches: {len(sw)}; rgb->cmyk sets: {len(rgb_to_cmyk)}; cmyk grid: {len(grid)}")
    print(f"synthetic v4 profile: {len(synth_bytes)} bytes, lcms description {synth_doc['lcms_description']!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
