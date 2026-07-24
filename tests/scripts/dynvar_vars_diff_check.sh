#!/usr/bin/env bash
# v332 (#286): dynamic (computed) shell variables match bash 5.2.21.
# Task 1 covers BASH_ARGV0 (read/write $0) + EPOCHREALTIME (<secs>.<6-digit-micros>).
# Task 2 appends BASH_COMMAND cases below — keep this `check` helper reusable.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash --norc --noprofile -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

check "argv0 assign"     'BASH_ARGV0=hello; echo "$0 $BASH_ARGV0"'
check "argv0 in fn"      'setarg0(){ BASH_ARGV0="$1"; }; setarg0 arg0; echo "$0"'
check "epochrealtime fmt" '[[ $EPOCHREALTIME =~ ^[0-9]+\.[0-9]{6}$ ]] && echo shape-ok'
check "epochrealtime pos" '(( ${EPOCHREALTIME%.*} > 0 )) && echo pos-ok'

check "bashcmd simple"   'echo $BASH_COMMAND'                          # echo $BASH_COMMAND
check "bashcmd in fn"    'f(){ echo $BASH_COMMAND; }; f'               # echo $BASH_COMMAND
check "bashcmd after asn" 'x=1; echo $BASH_COMMAND'                    # echo $BASH_COMMAND
check "bashcmd in debug"  'set -T; trap "echo D:\$BASH_COMMAND" DEBUG; :; true'  # match bash

if [ $FAIL -ne 0 ]; then echo "dynvar_vars_diff_check FAILED" >&2; exit 1; fi
echo "dynvar_vars_diff_check OK"
