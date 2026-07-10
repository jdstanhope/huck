#!/usr/bin/env bash
# Run every bash-diff harness against its DEFAULT binary. Green = all pass.
# Caller MUST build both binaries first:
#   cargo build --locked --bin huck            # target/debug/huck
#   cargo build --release --locked --bin huck  # target/release/huck
# Does NOT override HUCK_BIN (each harness picks its intended binary) and does
# NOT build or set ulimit — the caller owns that.
set -u
cd "$(dirname "$0")/../.." || exit 1   # repo root
for b in target/debug/huck target/release/huck; do
  [[ -x "$b" ]] || { echo "missing binary: $b — build it first" >&2; exit 1; }
done
pass=0; fail=0; failed=()
log=$(mktemp)
trap 'rm -f "$log"' EXIT
for h in tests/scripts/*_diff_check.sh; do
  name=$(basename "$h")
  case "$name" in
    run_diff_checks.sh|bash_test_suite_runner_diff_check.sh) continue ;;
  esac
  if timeout 120 bash "$h" >"$log" 2>&1; then
    pass=$((pass+1)); echo "PASS $name"
  else
    fail=$((fail+1)); failed+=("$name"); echo "FAIL $name"
    # Surface the failing harness's own output so a CI failure is debuggable.
    echo "----- $name output -----"; sed 's/^/    /' "$log"; echo "-----"
  fi
done
echo
echo "Diff-check sweep: $pass passed, $fail failed"
(( fail == 0 )) || { echo "Failed: ${failed[*]}" >&2; exit 1; }
