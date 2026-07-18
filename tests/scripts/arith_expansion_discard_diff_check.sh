#!/usr/bin/env bash
# v312 (#3/#49): a `$(( ))` arithmetic EXPANSION error discards the current
# top-level command (unwinds out of loops/functions, rc 1) WITHOUT exiting the
# shell — bash's jump_to_top_level(DISCARD). huck swallowed it (ran the command
# with an empty value, rc 0). Distinct from set -u/${x?} (which EXIT the shell).
# A comsub boundary CONTAINS the discard (outer command continues).
#
# The arithmetic ERROR MESSAGE wording is out of scope (#60): each shell's
# arith-error diagnostic line is normalized to `ARITH_ERR` so only the abort
# behavior (which commands run) and the rc are compared.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Normalize each shell's arith-error diagnostic line to a fixed token; everything
# else (command output, ordering) is compared raw. The prog/path prefix varies by
# context — `bash:` / `target/debug/huck:` for `-c`, the script path for a file,
# `environment:` inside a function — so the prefix is matched generically
# (`^[^:]*:`), leaving only the abort behavior + rc to be compared.
norm() {
  sed -E -e 's#^[^:]*: line [0-9]+: .*(arithmetic|unexpected character|division by 0|operand expected|syntax error).*#ARITH_ERR#' \
         -e 's#^[^:]*: .*(arithmetic|unexpected character|division by 0|operand expected).*#ARITH_ERR#'
}
check() {  # compares merged stdout+stderr (normalized) AND rc
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_script() {  # same, but runs a multi-line SCRIPT FILE (tests non-exit)
  local label=$1; shift
  local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" "$f" 2>&1 | norm); hr=${PIPESTATUS[0]}
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

# --- The fix (red->green): discard the current command.
check 'cmd-word'       'echo $((3.5)); echo done'
check 'before-after'   'echo BEFORE; echo $((3.5)); echo AFTER'
check 'bare-arith'     '$((3.5)); echo done'
check 'div-by-zero'    'echo $((1/0)); echo done'
check 'assignment'     'x=$((3.5)); echo AFTER'
check 'subscript'      'a[$((3.5))]=1; echo AFTER'
check 'loop-unwind'    'for i in 1 2 3; do echo i$i; echo $((3.5)); echo t$i; done; echo END'
check 'func-unwind'    'f(){ echo in; echo $((3.5)); echo after_in; }; f; echo AFTER_F'

# --- Multi-line SCRIPT: the discard must NOT exit the shell (later lines run).
check_script 'script-continues' 'echo $((3.5))' 'echo L2' 'echo L3'

# --- Comsub boundary CONTAINS the discard (outer command continues).
check 'comsub-contained' 'x=$( echo $((3.5)) ); echo "[$x] after"'
check 'comsub-inline'    'echo pre $( echo $((3.5)) ) post; echo NEXT'

# --- set -e: a discarded rc-1 command aborts like any rc-1 command.
check 'set-e'          'set -e; echo $((3.5)); echo done'

# --- Controls: must stay NON-fatal (different code path).
check 'arith-cmd'      '(( 3.5 )); echo done'
check 'cstyle-for'     'for ((i=3.5; i<1; i++)); do :; done; echo done'
check 'let-builtin'    'let "3.5"; echo done'
check 'valid-arith'    'echo $((1+1)); echo done'

if [ $FAIL -ne 0 ]; then echo "arith_expansion_discard_diff_check FAILED" >&2; exit 1; fi
echo "arith_expansion_discard_diff_check OK"
