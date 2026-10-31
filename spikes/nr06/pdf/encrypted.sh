#!/usr/bin/env bash
# Opens an AES-256 encrypted PDF with an empty user password (common for "protected" downloads).
# The fixture comes from fetch-fixtures.sh (qpdf --encrypt '' ownerpw 256).
here="$(cd "$(dirname "$0")/.." && pwd)"
export PDFIUM_LIB="$here/vendor-bin/pdfium/lib/libpdfium.dylib"
pdf="$here/fixtures/pdf/irs-f1040-aes256-empty-user.pdf"
out="$here/results/pdf-encrypted.txt"
: > "$out"
for r in pdfium hayro mupdf; do
  echo "== $r" | tee -a "$out"
  "$here/pdf/target/release/bench_$r" "$pdf" 1600 1 - tax 2>&1 | sed "s#$here/##" | cut -c1-400 | tee -a "$out"
done
echo "== pdfkit" | tee -a "$out"
"$here/pdf/target/pdfkit-bench" "$pdf" 1600 tax 2>&1 | sed "s#$here/##" | cut -c1-400 | tee -a "$out"
