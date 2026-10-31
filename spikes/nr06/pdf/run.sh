#!/usr/bin/env bash
# Runs the PDF renderer benchmarks and appends JSON lines to results/pdf-bench.jsonl.
# Prerequisites: fetch-fixtures.sh has run, `cargo build --release --features pdfium,hayro,mupdf`,
# and `swiftc -O pdfkit-bench.swift -o target/pdfkit-bench` (macOS).
# Env: RENDERERS (default "pdfium hayro mupdf pdfkit"), WIDTH (1600), KEEP (6 cached pages).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
export PDFIUM_LIB="$here/vendor-bin/pdfium/lib/libpdfium.dylib"
width="${WIDTH:-1600}"
keep="${KEEP:-6}"
mkdir -p "$here/out" "$here/results"
out="$here/results/pdf-bench.jsonl"
for f in rt-readme-sample attention irs-f1040 irs-p17; do
  for r in ${RENDERERS:-pdfium hayro mupdf pdfkit}; do
    pdf="$here/fixtures/pdf/$f.pdf"
    if [ "$r" = pdfkit ]; then
      cmd=("$here/pdf/target/pdfkit-bench" "$pdf" "$width" tax "$here/out")
    else
      cmd=("$here/pdf/target/release/bench_$r" "$pdf" "$width" "$keep" "$here/out" tax)
    fi
    line=$(/usr/bin/time -l "${cmd[@]}" 2>"$here/out/time.txt")
    rss=$(awk '/maximum resident set size/ {print $1}' "$here/out/time.txt")
    echo "${line%\}},\"max_rss_mb\":$(echo "scale=1; $rss/1048576" | bc)}" | tee -a "$out"
  done
done
