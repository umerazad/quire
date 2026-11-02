#!/bin/bash
# Runs every merge gate from the repository root and exits non-zero when any gate or test fails,
# so `bash tools/gates.sh && git commit` never commits a red tree.
set -uo pipefail
cd "$(dirname "$0")/.."
zed="${ZED_CHECKOUT:-../zed}"
log="$(mktemp -d)"
fail() {
    echo "GATE FAILED: $1. Fix it, then run tools/gates.sh again. Logs: $log"
    exit 1
}
features=test-support,gpui_platform/runtime_shaders
cargo fmt --check >"$log/fmt" 2>&1 || fail "cargo fmt --check"
cargo clippy -p quire_core -p quire_store -p quire_format -p quire_media --locked --all-targets --no-deps -- -D warnings >"$log/clippy-core" 2>&1 || fail "clippy on the core crates"
cargo clippy -p quire --locked --all-targets --features "$features" --no-deps -- -D warnings >"$log/clippy-quire" 2>&1 || fail "clippy on quire"
cargo build -q -p quire --example fake_ai_cli --features "$features" --locked >"$log/fake-cli" 2>&1 || fail "building the fake AI CLI"
for run in 1 2; do
    cargo test --no-fail-fast -p quire -p quire_core -p quire_store -p quire_format -p quire_media --locked --features "quire/$features" >"$log/test-$run" 2>&1
    status=$?
    grep -E '^test result' "$log/test-$run" | awk -v r="$run" '{p+=$4; f+=$6} END {print "test run " r ": passed " p ", failed " f}'
    grep -E '^    [a-z_:]+$|panicked at' "$log/test-$run" | head -20
    [ "$status" -eq 0 ] || fail "test run $run"
done
node node_modules/vitest/vitest.mjs run tests/reference --root . --exclude '.claude/**' >"$log/vitest" 2>&1 || fail "the reference vitest suite"
grep -E 'Tests ' "$log/vitest"
bun tools/verify-upstream.ts "$zed" >"$log/upstream" 2>&1 || fail "verify-upstream against $zed"
tail -1 "$log/upstream"
echo "ALL GATES PASSED"
