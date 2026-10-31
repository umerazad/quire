#!/usr/bin/env bash
# Peak resident memory of a formatter process handling all corpus snippets once.
# Usage: rss.sh helper <binary> | rss.sh quickjs <binary>
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
mode="$1"; bin="$2"
tmp="$(mktemp)"
if [ "$mode" = helper ]; then
  bun -e '
    const p = {js:"babel",javascript:"babel",jsx:"babel",ts:"typescript",typescript:"typescript",tsx:"typescript",json:"json",css:"css",html:"html",xml:"html",yaml:"yaml",yml:"yaml",markdown:"markdown",md:"markdown"};
    const fs = require("node:fs"); let id = 0;
    for (const f of fs.readdirSync(process.argv[1]).sort())
      console.log(JSON.stringify({id: id++, parser: p[f.split(".").pop()], text: fs.readFileSync(process.argv[1] + "/" + f, "utf8")}));
  ' "$here/corpus" > "$tmp.in"
  /usr/bin/time -l "$bin" < "$tmp.in" > /dev/null 2> "$tmp"
else
  /usr/bin/time -l "$bin" "$here/corpus" "$here/out-quickjs" 1 > /dev/null 2> "$tmp"
fi
awk '/maximum resident set size/ {printf "{\"mode\":\"%s\",\"max_rss_mb\":%.1f}\n", "'"$mode"'", $1/1048576}' "$tmp"
rm -f "$tmp" "$tmp.in"
