#!/usr/bin/env python3
"""Independent oracles for Veskra's CMYK print exports (ADR-018).

Usage: cmyk_check.py <dir>   (files written by crates/veskra-io/tests/cmyk_interop.rs)

For every <name>.expected.json in <dir>:
  {"name", "width", "height", "composite_rgba": <straight RGBA8 sRGB of Veskra's composite>,
   "psd": [<files>], "pdf": <file>, "profile_sha256": <sha of the CMYK profile Veskra embeds>,
   "srgb_icc": <Veskra's sRGB profile file>}

Checks:
  * psd-tools 1.24 opens each PSD: color mode CMYK, depth, size, layer count > 0, ICC resource 1039
    == the CMYK profile (SHA-256); merged image read as CMYK;
  * LittleCMS (Pillow ImageCms, relative colorimetric + BPC, the embedded profile) on the opaque
    pixels of the 8-bit PSD: Veskra's ink vs LittleCMS's ink for the same source pixels (every
    channel, percent; median <= 1, p95 <= 2, max <= 4), and Veskra's CMYK -> sRGB vs LittleCMS's
    sRGB -> CMYK -> sRGB (CIEDE2000 on D50 Lab; median <= 1, p95 <= 2.5 — one ink code can move a
    light neutral by one RGB code);
  * pypdf 6.17 opens the PDF: catalog /OutputIntents with /S /GTS_PDFX whose /DestOutputProfile is
    the CMYK profile (SHA-256, /N 4), every image XObject is /DeviceCMYK;
  * poppler (pdftoppm), when installed, renders the PDF without error.
Prints one JSON object per file set; exit 1 on any failed check.
"""
import glob
import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image, ImageCms
from psd_tools import PSDImage
from psd_tools.constants import ColorMode, Resource


def lab_of(img_rgb, srgb):
    lab = ImageCms.ImageCmsProfile(ImageCms.createProfile("LAB"))
    t = ImageCms.buildTransform(srgb, lab, "RGB", "LAB", flags=ImageCms.Flags.NOOPTIMIZE)
    # Pillow keeps LAB a*/b* as signed bytes in memory (getpixel() adds 128, the array does not).
    raw = np.asarray(ImageCms.applyTransform(img_rgb, t))
    ab = raw[..., 1:3].view(np.int8).astype(np.float64)
    return np.stack([raw[..., 0].astype(np.float64) * 100.0 / 255.0, ab[..., 0], ab[..., 1]], axis=-1)


def de2000(l1, l2):
    """CIEDE2000 (Sharma 2005) on arrays of Lab."""
    L1, a1, b1 = l1[..., 0], l1[..., 1], l1[..., 2]
    L2, a2, b2 = l2[..., 0], l2[..., 1], l2[..., 2]
    c1, c2 = np.hypot(a1, b1), np.hypot(a2, b2)
    cb = (c1 + c2) / 2
    g = 0.5 * (1 - np.sqrt(cb**7 / (cb**7 + 25.0**7)))
    a1p, a2p = (1 + g) * a1, (1 + g) * a2
    c1p, c2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.degrees(np.arctan2(b1, a1p)) % 360
    h2p = np.degrees(np.arctan2(b2, a2p)) % 360
    dl, dc = L2 - L1, c2p - c1p
    dh = h2p - h1p
    dh = np.where(dh > 180, dh - 360, np.where(dh < -180, dh + 360, dh))
    dh = np.where(c1p * c2p == 0, 0, dh)
    dhh = 2 * np.sqrt(c1p * c2p) * np.sin(np.radians(dh) / 2)
    lb, cbp = (L1 + L2) / 2, (c1p + c2p) / 2
    hb = np.where(np.abs(h1p - h2p) <= 180, (h1p + h2p) / 2,
                  np.where(h1p + h2p < 360, (h1p + h2p + 360) / 2, (h1p + h2p - 360) / 2))
    hb = np.where(c1p * c2p == 0, h1p + h2p, hb)
    t = (1 - 0.17 * np.cos(np.radians(hb - 30)) + 0.24 * np.cos(np.radians(2 * hb))
         + 0.32 * np.cos(np.radians(3 * hb + 6)) - 0.20 * np.cos(np.radians(4 * hb - 63)))
    dth = 30 * np.exp(-(((hb - 275) / 25) ** 2))
    rc = 2 * np.sqrt(cbp**7 / (cbp**7 + 25.0**7))
    sl = 1 + 0.015 * (lb - 50) ** 2 / np.sqrt(20 + (lb - 50) ** 2)
    sc, sh = 1 + 0.045 * cbp, 1 + 0.015 * cbp * t
    rt = -np.sin(np.radians(2 * dth)) * rc
    return np.sqrt((dl / sl) ** 2 + (dc / sc) ** 2 + (dhh / sh) ** 2 + rt * (dc / sc) * (dhh / sh))


def check_psd(path, exp, comp, srgb, errors):
    out = {"file": os.path.basename(path)}
    psd = PSDImage.open(path)
    out.update(color_mode=str(psd.color_mode), depth=psd.depth, size=[psd.width, psd.height],
               layers=len(list(psd.descendants())))
    if psd.color_mode != ColorMode.CMYK:
        errors.append(f"{path}: color mode {psd.color_mode}")
    if (psd.width, psd.height) != (exp["width"], exp["height"]):
        errors.append(f"{path}: size")
    if out["layers"] == 0:
        errors.append(f"{path}: no layers")
    icc = psd.image_resources.get_data(Resource.ICC_PROFILE)
    if icc is None or hashlib.sha256(icc).hexdigest() != exp["profile_sha256"]:
        errors.append(f"{path}: ICC resource is not the CMYK profile")
        return out
    merged = psd.topil(apply_icc=False)
    out["merged_mode"] = merged.mode if merged is not None else None
    if merged is None or merged.mode not in ("CMYK",):
        errors.append(f"{path}: merged image not CMYK ({out['merged_mode']})")
        return out
    if psd.depth != 8:
        return out  # the LittleCMS round trip below uses 8-bit buffers
    # Opaque pixels only: Veskra's CMYK -> sRGB vs LittleCMS sRGB -> CMYK -> sRGB.
    cmyk_prof = ImageCms.ImageCmsProfile(io.BytesIO(icc))
    flags = ImageCms.Flags.NOOPTIMIZE | ImageCms.Flags.BLACKPOINTCOMPENSATION
    rel = ImageCms.Intent.RELATIVE_COLORIMETRIC
    opaque = comp[..., 3] == 255
    src = Image.fromarray(np.ascontiguousarray(comp[..., :3]), "RGB")
    to_cmyk = ImageCms.buildTransform(srgb, cmyk_prof, "RGB", "CMYK", rel, flags)
    to_rgb = ImageCms.buildTransform(cmyk_prof, srgb, "CMYK", "RGB", rel, flags)
    lcms_rt = ImageCms.applyTransform(ImageCms.applyTransform(src, to_cmyk), to_rgb)
    veskra_rt = ImageCms.applyTransform(merged, to_rgb)
    d = de2000(lab_of(veskra_rt, srgb), lab_of(lcms_rt, srgb))[opaque]
    out["lcms_roundtrip_de00"] = {"median": round(float(np.median(d)), 4),
                                  "p95": round(float(np.percentile(d, 95)), 4),
                                  "max": round(float(d.max()), 4), "pixels": int(d.size)}
    # One 8-bit ink code (0.39 %) can move a light neutral by one RGB code, i.e. ~0.9 dE00, so the
    # round trip is bounded loosely; the ink comparison below carries the ADR-018 targets.
    if d.size and (float(np.median(d)) > 1.0 or float(np.percentile(d, 95)) > 2.5):
        errors.append(f"{path}: LittleCMS round trip dE00 median {np.median(d):.3f} / "
                      f"p95 {np.percentile(d, 95):.3f} above 1.0 / 2.5")
    # Ink actually written vs LittleCMS's ink for the same pixels (percent, every channel).
    lcms_ink = np.asarray(ImageCms.applyTransform(src, to_cmyk)).astype(np.float64)
    ink = np.asarray(merged).astype(np.float64)
    di = np.abs(ink - lcms_ink)[opaque] / 255 * 100
    s = {"median": round(float(np.median(di)), 4), "p95": round(float(np.percentile(di, 95)), 4),
         "max": round(float(di.max()), 4)}
    out["ink_vs_lcms_percent"] = s
    if s["median"] > 1.0 or s["p95"] > 2.0 or s["max"] > 4.0:
        errors.append(f"{path}: ink vs LittleCMS {s} above 1 / 2 / 4 %")
    return out


def check_pdf(path, exp, errors):
    import pypdf
    out = {"file": os.path.basename(path)}
    r = pypdf.PdfReader(path)
    root = r.trailer["/Root"]
    intents = root.get("/OutputIntents")
    if exp.get("tagged", True):
        if not intents:
            errors.append(f"{path}: no /OutputIntents")
        else:
            oi = intents[0].get_object()
            prof = oi["/DestOutputProfile"].get_object()
            data = prof.get_data()
            out["output_intent"] = {"S": str(oi["/S"]), "N": int(prof["/N"]), "id": str(oi["/OutputConditionIdentifier"])}
            if str(oi["/S"]) != "/GTS_PDFX" or int(prof["/N"]) != 4 or hashlib.sha256(data).hexdigest() != exp["profile_sha256"]:
                errors.append(f"{path}: output intent profile mismatch")
    spaces = []
    for page in r.pages:
        xo = page["/Resources"].get("/XObject", {})
        for k in xo:
            o = xo[k].get_object()
            if o.get("/Subtype") == "/Image":
                spaces.append(str(o["/ColorSpace"]))
    out["image_color_spaces"] = sorted(set(spaces))
    if not spaces or any(s != "/DeviceCMYK" for s in spaces):
        errors.append(f"{path}: image color spaces {sorted(set(spaces))}")
    if shutil.which("pdftoppm"):
        with tempfile.TemporaryDirectory() as td:
            p = subprocess.run(["pdftoppm", "-r", "36", "-png", path, os.path.join(td, "r")],
                               capture_output=True, text=True)
            out["poppler_render"] = p.returncode == 0 and bool(glob.glob(os.path.join(td, "r*.png")))
            if not out["poppler_render"]:
                errors.append(f"{path}: pdftoppm failed: {p.stderr.strip()}")
    else:
        out["poppler_render"] = None
    return out


def main():
    d = sys.argv[1]
    errors = []
    results = []
    for jp in sorted(glob.glob(os.path.join(d, "*.expected.json"))):
        exp = json.load(open(jp))
        comp = np.fromfile(exp["composite_rgba"], dtype=np.uint8).reshape(exp["height"], exp["width"], 4)
        srgb = ImageCms.ImageCmsProfile(exp["srgb_icc"])
        res = {"name": exp["name"], "psd": [], "pdf": None}
        for p in exp["psd"]:
            res["psd"].append(check_psd(p, exp, comp, srgb, errors))
        if exp.get("pdf"):
            res["pdf"] = check_pdf(exp["pdf"], exp, errors)
        results.append(res)
    print(json.dumps({"results": results, "errors": errors}))
    return 1 if errors or not results else 0


if __name__ == "__main__":
    sys.exit(main())
