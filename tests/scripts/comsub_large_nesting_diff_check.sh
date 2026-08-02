#!/usr/bin/env bash
# Stage 0 (#197): baseline for LARGE-OUTPUT and NESTED `$( )` behavior, a safety
# net before command substitution is reworked to fork+pipe-drain. `check` = must
# match bash; `check_pin` = pins huck's CURRENT (diverging) output, a STAGE-1
# TARGET, so this file is green now.
#
# The large-output cases are DEADLOCK GUARDS: a future fork+pipe implementation
# must drain >64 KB of producer output without blocking. Current huck buffers
# in memory so these should NOT hang today; each large case is wrapped in
# `timeout 10` so a future regression surfaces as a FAIL, not a stuck sweep.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
check() {
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(timeout 10 bash -c "$frag" 2>/tmp/s0n_be); br=$?; be=$(cat /tmp/s0n_be)
  ho=$(timeout 10 "$HUCK" -c "$frag" 2>/tmp/s0n_he); hr=$?; he=$(cat /tmp/s0n_he)
  if [ "$br" = 124 ]; then echo "FAIL [$label] (bash TIMED OUT)"; FAIL=1; return; fi
  if [ "$hr" = 124 ]; then echo "FAIL [$label] (huck TIMED OUT — deadlock?)"; FAIL=1; return; fi
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_pin() {
  local label=$1 frag=$2 xo=$3 xe=$4 xr=$5 ho he hr
  ho=$(timeout 10 "$HUCK" -c "$frag" 2>/tmp/s0n_he); hr=$?; he=$(cat /tmp/s0n_he)
  if [ "$hr" = 124 ]; then echo "FAIL [$label] (huck TIMED OUT — deadlock?)"; FAIL=1; return; fi
  if [ "$ho" != "$xo" ] || [ "$he" != "$xe" ] || [ "$hr" != "$xr" ]; then
    echo "FAIL [$label] (pin drifted)"; echo "  want: out=[$xo] err=[$xe] rc=$xr"; echo "  got : out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PIN  [$label]"; fi
}

# --- Large output (deadlock guards; each MUST match bash) ---
# THE in-process-builtin >64 KB deadlock guard: a single `printf` builtin emits
# 70000 bytes straight into the comsub capture buffer (no external command, no
# brace expansion, no real pipe). This is the exact path a future fork+pipe-drain
# must handle without blocking — if that regresses, this case hangs and the
# `timeout 10` in `check` turns it into a FAIL.
check 'large 70k builtin printf' 'x=$(printf "%70000s" ""); echo ${#x}'
# ~108 KB of many small in-process writes (12000 echoes) — pushes the in-process
# capture well past a single 64 KB pipe buffer via repeated builtin writes.
check 'large with newlines' 'x=$(for i in {1..12000}; do echo line$i; done); echo "${#x} $(printf %s "$x" | wc -l)"'
# External >64 KB producer through a REAL per-command pipe (not the in-process
# capture buffer) — complements the builtin guard above on the other plumbing path.
check 'large 70k external'  'x=$(head -c 70000 /dev/zero | tr "\0" X); echo ${#x}'
# NOTE: this `{1..70000}` case is a BRACE-EXPANSION-CAP divergence, ORTHOGONAL to
# comsub — NOT a large-output/deadlock guard. huck caps brace expansion at
# MAX_ELEMENTS=65536 (crates/huck-syntax/src/brace_expand.rs), so `{1..70000}` is
# rejected at parse time before any output is produced; bash 5.2 has no such cap.
# A future fork+pipe change never touches this path, so the pin should stay put.
# STAGE-1 TARGET (#387 — brace-cap divergence, orthogonal to comsub)
check_pin 'brace cap 70000 (not a large-output guard)' 'x=$(printf "%0.sX" {1..70000}); echo ${#x}' \
  '' 'target/debug/huck: -c: line 1: syntax error: brace expansion: too many elements' '2'

# --- Nesting ---
check 'nested comsub'      'echo $(echo $(echo deep))'
check 'comsub in pipeline' 'echo $(echo hi) | cat'
check 'comsub in subshell' '( x=$(echo a); echo "$x" )'
check 'comsub in lastpipe' 'shopt -s lastpipe; echo hi | while read l; do echo "$(echo $l)"; done'
check 'comsub arg splits'  'set -- $(echo a b c); echo $#'
check 'comsub in for'      'for w in $(echo x y z); do printf "[%s]" "$w"; done'
check 'backtick nesting'   'echo `echo hi`'

rm -f /tmp/s0n_be /tmp/s0n_he
echo ""; [ "$FAIL" -eq 0 ] && echo "comsub_large_nesting OK" || { echo "comsub_large_nesting FAILED" >&2; exit 1; }
