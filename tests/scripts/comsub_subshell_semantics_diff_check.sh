#!/usr/bin/env bash
# Stage 0 (#197): baseline for the SUBSHELL SEMANTICS of `$( )`, a safety net
# before command substitution is reworked to fork a real subshell (today huck
# runs the body in-process against a cloned Shell). These cases pin the parent-
# visible effects a real fork would have to reproduce EXACTLY: $? propagation,
# state isolation (vars/cwd/positional params), inherited stdin, and trap
# inheritance/listing. `check` = must match bash now; `check_pin` = pins huck's
# CURRENT (diverging) output as a STAGE-1 TARGET so this file is green today and
# a deliberate re-match (not an accident) flips the pin later.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
check() {
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(timeout 10 bash -c "$frag" 2>/tmp/s0s_be); br=$?; be=$(cat /tmp/s0s_be)
  ho=$(timeout 10 "$HUCK" -c "$frag" 2>/tmp/s0s_he); hr=$?; he=$(cat /tmp/s0s_he)
  if [ "$br" = 124 ]; then echo "FAIL [$label] (bash TIMED OUT)"; FAIL=1; return; fi
  if [ "$hr" = 124 ]; then echo "FAIL [$label] (huck TIMED OUT — deadlock?)"; FAIL=1; return; fi
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_pin() {
  local label=$1 frag=$2 xo=$3 xe=$4 xr=$5 ho he hr
  ho=$(timeout 10 "$HUCK" -c "$frag" 2>/tmp/s0s_he); hr=$?; he=$(cat /tmp/s0s_he)
  if [ "$hr" = 124 ]; then echo "FAIL [$label] (huck TIMED OUT — deadlock?)"; FAIL=1; return; fi
  if [ "$ho" != "$xo" ] || [ "$he" != "$xe" ] || [ "$hr" != "$xr" ]; then
    echo "FAIL [$label] (pin drifted)"; echo "  want: out=[$xo] err=[$xe] rc=$xr"; echo "  got : out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PIN  [$label]"; fi
}

# --- $? propagation: the comsub's exit status is the parent's $? ---
check 'dollar-? from exit' 'x=$(exit 7); echo $?'
check 'dollar-? success'   'x=$(true); echo $?'
check 'dollar-? false'     'x=$(false); echo $?'

# --- State isolation: mutations inside `$( )` MUST NOT leak to the parent ---
check 'state isolation var' 'v=outer; x=$(v=inner; echo x); echo "$v"'
check 'state isolation cd'  'cd /; x=$(cd /tmp; :); echo "$PWD"'
check 'state isolation set' 'set -- a b; x=$(set -- x y z; :); echo $#'
check 'nested var isolation' 'v=1; echo $(v=2; echo $(v=3; echo $v)-$v)-$v'

# --- Inherited stdin: the comsub reads the surrounding command's stdin ---
# `printf | { ... }` feeds the pipe so `$(cat)` never blocks on a tty.
check 'stdin read in comsub' 'printf "hi\n" | { x=$(cat); echo "[$x]"; }'

# --- `exit N` inside the body terminates the substitution with status N ---
check 'exit propagates' 'x=$( echo a; exit 3; echo b ); printf "<%s> rc=%s" "$x" "$?"'

# --- FUNCNAME visible in a comsub run inside a function ---
check 'funcname in comsub' 'f(){ echo "$FUNCNAME"; }; x=$(f); echo "[$x]"'

# --- Trap inheritance/listing inside a comsub ---
# Both shells INHERIT the parent EXIT trap into the comsub (so `trap` with no
# args lists `echo T` EXIT), and the EXIT trap fires once at the parent's end.
# DIVERGENCE: bash additionally lists a default `trap -- '' SIGTSTP` entry (its
# non-interactive job-control default disposition); huck lists only the EXIT
# trap. This is the exact trap-table view a forked comsub must reproduce, so the
# pin records what huck emits today.
# STAGE-1 TARGET (#197 — comsub trap-table view; bash adds a SIGTSTP default row)
check_pin 'trap list in comsub' 'trap "echo T" EXIT; x=$(trap); echo "list=[$x]"' \
  "list=[trap -- 'echo T' EXIT]
T" '' '0'

rm -f /tmp/s0s_be /tmp/s0s_he
echo ""; [ "$FAIL" -eq 0 ] && echo "comsub_subshell_semantics OK" || { echo "comsub_subshell_semantics FAILED" >&2; exit 1; }
