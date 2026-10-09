#!/usr/bin/env python3
"""Inkscape oracle for Veskra "Export for Illustrator" SVG/PDF files (FMT-15 / SPK-07).

NOT Adobe Illustrator evidence. Uses Inkscape (tested with 1.2.2) as an independent
reader/editor:

  * SVG render: ``inkscape --export-type=png`` of every exported SVG, compared with Veskra's own
    composite (``<fixture>-reference.png`` written by the Rust interop test). Metric: max /
    mean absolute difference of premultiplied RGBA8 and % of pixels whose worst channel differs
    by more than 16. Doubled strokes would show up as large differences.
  * SVG edit round-trip: ``inkscape --export-plain-svg`` and count ``<path>`` elements that
    survive (hidden "Editable vectors" content must survive as editable paths).
  * PDF import: ``inkscape --pdf-poppler=false file.pdf --export-type=svg`` (Inkscape's own
    importer, paths become editable objects) and count imported ``<path>`` / ``<image>``.

Usage (repository root, after ``cargo test -p veskra-io --test svg_pdf_interop``):
  python3 -I scripts/interop/svg_pdf_inkscape_check.py target/interop/illustrator

Writes ``inkscape.json`` next to the inputs; exits non-zero if Inkscape fails on any file.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def run(args: list[str]) -> None:
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=300)


def premul(path: Path) -> np.ndarray:
    a = np.asarray(Image.open(path).convert("RGBA")).astype(np.float64)
    al = a[..., 3:4] / 255.0
    return np.concatenate([np.round(a[..., :3] * al), a[..., 3:4]], axis=2)


def diff(a: np.ndarray, b: np.ndarray) -> dict:
    d = np.abs(a - b)
    worst = d.max(axis=2)
    return {"max": int(d.max()), "mean": round(float(d.mean()), 4), "pct_over_16": round(float((worst > 16).mean() * 100), 4)}


def visible_and_hidden_paths(svg_text: str) -> tuple[int, int]:
    """Count Veskra paths before / inside the display="none" group (writer emits it last)."""
    idx = svg_text.find('display="none"')
    if idx < 0:
        return svg_text.count("<path "), 0
    return svg_text[:idx].count("<path "), svg_text[idx:].count("<path ")


def main() -> int:
    d = Path(sys.argv[1] if len(sys.argv) > 1 else "target/interop/illustrator")
    work = d / "inkscape"
    work.mkdir(exist_ok=True)
    version = subprocess.run(["inkscape", "--version"], capture_output=True, text=True).stdout.strip()
    rows, failed = [], 0
    for svg in sorted(d.glob("*.svg")):
        fixture = svg.name.split("-")[0]
        ref = d / f"{fixture}-reference.png"
        text = svg.read_text(encoding="utf-8")
        vis, hid = visible_and_hidden_paths(text)
        row = {"file": svg.name, "veskra_visible_paths": vis, "veskra_hidden_paths": hid}
        try:
            png = work / (svg.stem + ".inkscape.png")
            run(["inkscape", "--export-type=png", f"--export-filename={png}", str(svg)])
            if ref.exists():
                a, b = premul(png), premul(ref)
                row["render_size_ok"] = a.shape == b.shape
                if a.shape == b.shape:
                    row["render_vs_composite"] = diff(a, b)
            plain = work / (svg.stem + ".plain.svg")
            run(["inkscape", "--export-plain-svg", f"--export-filename={plain}", str(svg)])
            ptxt = plain.read_text(encoding="utf-8")
            row["inkscape_roundtrip_paths"] = len(re.findall(r"<path\b", ptxt))
            row["inkscape_roundtrip_hidden_groups"] = ptxt.count("display:none") + ptxt.count('display="none"')
            row["ok"] = True
        except Exception as e:  # noqa: BLE001
            row.update({"ok": False, "error": repr(e)})
            failed += 1
        rows.append(row)
        print(json.dumps(row, sort_keys=True))
    for pdf in sorted(d.glob("*.pdf")):
        sib = pdf.with_suffix(".svg")
        row = {"file": pdf.name}
        if sib.exists():
            vis, hid = visible_and_hidden_paths(sib.read_text(encoding="utf-8"))
            row.update({"veskra_visible_paths": vis, "veskra_hidden_paths": hid})
        try:
            out = work / (pdf.stem + ".pdf-import.svg")
            run(["inkscape", "--pdf-poppler=false", str(pdf), "--export-type=svg", f"--export-filename={out}"])
            t = out.read_text(encoding="utf-8")
            row["inkscape_import_paths"] = len(re.findall(r"<path\b", t))
            row["inkscape_import_images"] = len(re.findall(r"<image\b", t))
            row["ok"] = True
        except Exception as e:  # noqa: BLE001
            row.update({"ok": False, "error": repr(e)})
            failed += 1
        rows.append(row)
        print(json.dumps(row, sort_keys=True))
    (d / "inkscape.json").write_text(json.dumps({"inkscape": version, "rows": rows}, indent=2, sort_keys=True))
    print(f"{version}: {len(rows) - failed}/{len(rows)} files processed", file=sys.stderr)
    return 1 if failed or not rows else 0


if __name__ == "__main__":
    sys.exit(main())
