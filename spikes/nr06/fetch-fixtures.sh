#!/usr/bin/env bash
# Downloads the public PDFs and the prebuilt pdfium used by the NR06 spike. None are committed.
# The IRS files are revised yearly; a hash mismatch means the measured file is no longer served.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$here/fixtures/pdf" "$here/vendor-bin/pdfium"
cd "$here/fixtures/pdf"
curl -sSLf -o attention.pdf https://arxiv.org/pdf/1706.03762v7
curl -sSLf -o irs-p17.pdf https://www.irs.gov/pub/irs-pdf/p17.pdf
curl -sSLf -o irs-f1040.pdf https://www.irs.gov/pub/irs-pdf/f1040.pdf
# The one-page sample embedded in rt's README (Helvetica, not embedded: exercises standard-14 fallback).
grep -o 'data:application/pdf;base64,[A-Za-z0-9+/=]*' /Users/umer/code/rt/src/content/README.md | head -1 | cut -d, -f2 > sample.b64
base64 -d -i sample.b64 -o rt-readme-sample.pdf && rm sample.b64
nix-shell -p qpdf --run "qpdf --encrypt '' ownerpw 256 -- irs-f1040.pdf irs-f1040-aes256-empty-user.pdf"
shasum -a 256 -c <<'EOF' || echo "warning: fixture hashes differ from the measured files"
bdfaa68d8984f0dc02beaca527b76f207d99b666d31d1da728ee0728182df697  attention.pdf
3d31c226df0d189ced80e039d01cf0f8820c1019681a0f0ca6264de277b7e982  irs-f1040.pdf
2d2381d62c7c77ed4b64b83ac23c28ed85b7c8304eb580c09bd9fd660be0840b  irs-p17.pdf
9ba0796362224e3092b198787763ec555d4195e3fd5088cc0c79e75196e2786c  rt-readme-sample.pdf
EOF
cd "$here/vendor-bin/pdfium"
# chromium/8125, published 2026-10-26; sha256 4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d
curl -sSLf -o pdfium-mac-arm64.tgz https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8125/pdfium-mac-arm64.tgz
tar xzf pdfium-mac-arm64.tgz
