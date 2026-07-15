#!/usr/bin/env bash
# v300 (#159): huck must accept `-o <option>` / `+o <option>` at the command
# line and apply it like `set -o`, matching bash. Compares stdout+stderr+rc
# byte-identically (shell-name prologue normalized to SH:).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Strips a leading shell-name prologue, REPEATEDLY (bash's own CLI-level `-o
# <badname>` diagnostic doubles argv[0]: `bash: line 0: bash: badname:
# invalid option name` — a genuine bash 5.2.21 quirk, not a huck bug), so both
# sides reduce to the same shell-agnostic residual (e.g. `badname: invalid
# option name`).
norm() { sed -E ":a; s#^(bash|huck|$HUCK)(: line [0-9]+)?: ##; ta"; }
# $@ = argv to pass to each shell (already split); compares combined out+err+rc.
check() {
  local label=$1; shift
  local b h
  b=$( { bash    "$@"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { "$HUCK" "$@"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# -o applies the option (errexit takes effect -> no "after", rc 1)
check 'o-errexit'  -o errexit -c 'false; echo after'
# -o posix is accepted and runs
check 'o-posix'    -o posix -c 'echo ok'
# +o syntax accepted and runs
check 'plus-o'     +o errexit -c 'echo ok'
# bad option name -> "<name>: invalid option name", rc 2
check 'o-badname'  -o badname -c 'echo x'

if [ $FAIL -ne 0 ]; then echo "cli_o_flag_diff_check FAILED" >&2; exit 1; fi
echo "cli_o_flag_diff_check OK"
