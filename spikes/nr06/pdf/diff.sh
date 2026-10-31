#!/usr/bin/env bash
# Compares each renderer's first two pages against pdfium (Chromium's engine, which rt's iframe uses).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/results/pdf-pixel-diff.jsonl"
: > "$out"
for f in rt-readme-sample-p1 attention-p1 attention-p2 irs-f1040-p1 irs-f1040-p2 irs-p17-p1 irs-p17-p2; do
  for r in hayro mupdf pdfkit; do
    "$here/pdf/target/release/diff_png" "$here/out/pdfium-$f.png" "$here/out/$r-$f.png" | tee -a "$out"
  done
done
