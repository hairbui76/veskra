#!/usr/bin/env bash
# Interim open-source oracle run for "Export for Illustrator" (SVG/PDF) — FMT-15 / SPK-07.
#
# THIS IS NOT ADOBE ILLUSTRATOR EVIDENCE. ILL-001/002/003/005/007/009 need a human with
# Illustrator: see scripts/interop/illustrator_manual_checklist.md.
#
# Steps:
#   1. Rust interop test: builds the fixtures, exports every option/strategy combination to
#      target/interop/illustrator/, validates them, renders with rsvg-convert / pdftocairo /
#      pdftoppm, runs qpdf --check, compares renders with Veskra's composite (no double
#      rendering, incl. a negative control) and writes metrics.json.
#   2. pypdf parse (pages, MediaBox, OCGs and their /OFF state)          -> pypdf.json
#   3. Inkscape render / SVG round-trip / PDF import path counts          -> inkscape.json
#   4. Optional: VESKRA_COPY_SAMPLES=1 refreshes tests/fixtures/exports/illustrator/ samples.
#
# Usage (repository root):  scripts/interop/svg_pdf_oracles.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/target/interop/illustrator"
cd "$ROOT"

echo "== tool versions"
rsvg-convert --version 2>/dev/null || echo "rsvg-convert: NOT INSTALLED"
pdftocairo -v 2>&1 | head -1 || echo "pdftocairo: NOT INSTALLED"
qpdf --version 2>/dev/null | head -1 || echo "qpdf: NOT INSTALLED"
inkscape --version 2>/dev/null || echo "inkscape: NOT INSTALLED"
python3 -I -c 'import pypdf; print("pypdf", pypdf.__version__)' 2>/dev/null || echo "pypdf: NOT INSTALLED"

echo "== 1. Rust export + render comparison"
cargo test -p veskra-io --test svg_pdf_interop -- --nocapture --test-threads=1

echo "== 2. pypdf"
python3 -I scripts/interop/pdf_pypdf_check.py "$OUT"

echo "== 3. Inkscape"
if command -v inkscape >/dev/null; then
  python3 -I scripts/interop/svg_pdf_inkscape_check.py "$OUT"
else
  echo "SKIP: inkscape not installed (NOT RUN)"
fi

if [[ "${VESKRA_COPY_SAMPLES:-0}" == "1" ]]; then
  echo "== 4. refresh sample fixtures"
  DEST="$ROOT/tests/fixtures/exports/illustrator"
  mkdir -p "$DEST"
  for f in hybrid-both-default.svg hybrid-both-default.pdf hybrid-both-residual.svg hybrid-both-residual.pdf \
           ink_vector_only-vector-only.svg ink_vector_only-vector-only.pdf blend_multiply-both-residual.pdf; do
    cp "$OUT/$f" "$DEST/$f"
  done
  du -ch "$DEST"/*.svg "$DEST"/*.pdf | tail -1
fi
echo "== done: results in $OUT (metrics.json, pypdf.json, inkscape.json)"
