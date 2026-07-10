#!/usr/bin/env bash
# Run every bash-diff harness against its DEFAULT binary. Green = all pass.
# Caller MUST build both binaries first:
#   cargo build --locked --bin huck            # target/debug/huck
#   cargo build --release --locked --bin huck  # target/release/huck
# Does NOT override HUCK_BIN (each harness picks its intended binary) and does
# NOT build or set ulimit — the caller owns that.
set -u
# GitHub Actions (and some init/CI parents) start the job with SIGPIPE set to
# SIG_IGN. A bash child cannot un-ignore an inherited-ignored signal (POSIX), so
# bash builtins (printf/echo) writing to a closed pipe spam "write error: Broken
# pipe" instead of dying silently — diverging from huck, which resets SIGPIPE to
# SIG_DFL at startup. Re-exec the sweep once with SIGPIPE reset to default so
# both shells behave as in a normal terminal. GNU coreutils `env`; on platforms
# without --default-signal this is skipped (best-effort) and a normal-TTY run is
# unaffected since SIGPIPE is already default there.
if [[ -z "${_DIFFCHECK_SIGPIPE_RESET:-}" ]] && env --default-signal=PIPE true 2>/dev/null; then
  export _DIFFCHECK_SIGPIPE_RESET=1
  exec env --default-signal=PIPE bash "$0" "$@"
fi
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
