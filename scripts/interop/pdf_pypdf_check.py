#!/usr/bin/env python3
"""Independent PDF parse of Veskra "Export for Illustrator" PDFs with pypdf (FMT-15 / SPK-07).

NOT Adobe Illustrator evidence. Checks, for every ``*.pdf`` in the given directory:
  * pypdf opens the file (strict=True) and finds exactly one page;
  * MediaBox in points (reported; the Rust test checks it equals px * 72 / dpi);
  * Optional Content Groups: names, and which ones the default configuration lists in ``/OFF``;
  * image XObjects on the page (with /SMask presence) and Form XObjects.

Usage (repository root):
  cargo test -p veskra-io --test svg_pdf_interop      # writes target/interop/illustrator/*.pdf
  python3 -I scripts/interop/pdf_pypdf_check.py target/interop/illustrator

Writes ``pypdf.json`` next to the PDFs and exits non-zero if any file fails to parse.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pypdf


def name_of(obj) -> str:
    n = obj.get_object().get("/Name")
    return str(n) if n is not None else "?"


def check(path: Path) -> dict:
    r = pypdf.PdfReader(str(path), strict=True)
    out: dict = {"file": path.name, "pypdf": pypdf.__version__, "pages": len(r.pages)}
    if len(r.pages) != 1:
        raise ValueError(f"{path.name}: expected 1 page, got {len(r.pages)}")
    page = r.pages[0]
    out["media_box_pt"] = [float(v) for v in page.mediabox]
    root = r.trailer["/Root"]
    ocp = root.get("/OCProperties")
    if ocp is not None:
        ocp = ocp.get_object()
        out["ocgs"] = [name_of(o) for o in ocp["/OCGs"]]
        d = ocp["/D"].get_object()
        out["ocg_off"] = [name_of(o) for o in d.get("/OFF", [])]
        out["ocg_on"] = [name_of(o) for o in d.get("/ON", [])]
    else:
        out["ocgs"], out["ocg_off"], out["ocg_on"] = [], [], []
    xobjs = page["/Resources"].get_object().get("/XObject", {})
    imgs = forms = smasks = 0
    for _, ref in sorted(xobjs.items()):
        x = ref.get_object()
        if x.get("/Subtype") == "/Image":
            imgs += 1
            smasks += 1 if "/SMask" in x else 0
            cs = x.get("/ColorSpace")
            cs = cs.get_object() if cs is not None else None
            out.setdefault("image_color_spaces", []).append(str(cs[0]) if isinstance(cs, list) else str(cs))
            # Decode to make sure the Flate data is complete.
            data = x.get_data()
            expected = int(x["/Width"]) * int(x["/Height"]) * 3
            if len(data) != expected:
                raise ValueError(f"{path.name}: image data {len(data)} != {expected}")
        elif x.get("/Subtype") == "/Form":
            forms += 1
    out.update({"images": imgs, "images_with_smask": smasks, "forms": forms})
    content = page.get_contents().get_data().decode("latin-1")
    out["bdc"] = content.count(" BDC")
    return out


def main() -> int:
    d = Path(sys.argv[1] if len(sys.argv) > 1 else "target/interop/illustrator")
    results, failed = [], 0
    for p in sorted(d.glob("*.pdf")):
        try:
            res = check(p)
            res["ok"] = True
        except Exception as e:  # noqa: BLE001 - report every failure
            res = {"file": p.name, "ok": False, "error": repr(e)}
            failed += 1
        results.append(res)
        print(json.dumps(res, sort_keys=True))
    (d / "pypdf.json").write_text(json.dumps(results, indent=2, sort_keys=True))
    print(f"pypdf {pypdf.__version__}: {len(results) - failed}/{len(results)} PDFs parsed", file=sys.stderr)
    return 1 if failed or not results else 0


if __name__ == "__main__":
    sys.exit(main())
