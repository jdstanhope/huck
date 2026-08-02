#!/usr/bin/env bash
# Stage 0 (#197): baseline for stderr/stdout routing through `$( )` capture that
# a later fork-comsub change will move. `check` = must match bash; `check_pin` =
# pins huck's CURRENT (diverging) output, a STAGE-1 TARGET, so this file is green now.
#
# Deliberately NON-overlapping with comsub_merge_stderr_diff_check.sh: that file
# covers compound `{ …; } 2>&1` / subshell `2>&1` groups inside $() and its own
# #195 `2>&1 >file` pin. This file covers the bare-simple-command + `$(<file)` +
# readonly-arith(#353) + external-producer family.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
check() {
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/s0m_be); br=$?; be=$(cat /tmp/s0m_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/s0m_he); hr=$?; he=$(cat /tmp/s0m_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_pin() {
  local label=$1 frag=$2 xo=$3 xe=$4 xr=$5 ho he hr
  ho=$("$HUCK" -c "$frag" 2>/tmp/s0m_he); hr=$?; he=$(cat /tmp/s0m_he)
  if [ "$ho" != "$xo" ] || [ "$he" != "$xe" ] || [ "$hr" != "$xr" ]; then
    echo "FAIL [$label] (pin drifted)"; echo "  want: out=[$xo] err=[$xe] rc=$xr"; echo "  got : out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PIN  [$label]"; fi
}

# 1. plain capture: only stdout is captured, no leaked stderr.
check 'plain-builtin'  'x=$(echo o); printf "<%s>" "$x"'
check 'plain-external' 'x=$(/bin/echo o); printf "<%s>" "$x"'

# 2. stderr escapes the capture: only `o` captured, `e` on terminal stderr.
check 'stderr-escapes-builtin'  'x=$(echo o; echo e >&2); printf "<%s>" "$x"'
check 'stderr-escapes-external' 'x=$(/bin/echo o; /bin/echo e >&2); printf "<%s>" "$x"'

# 3. bare-simple-command 2>&1 inside $(): stderr merges into capture.
check 'simple-2>&1-builtin'  'x=$(echo hi 2>&1); printf "<%s>" "$x"'
check 'simple-2>&1-external' 'x=$(/bin/echo hi 2>&1); printf "<%s>" "$x"'
check 'simple-2>&1-err-only' 'x=$(echo hi >&2 2>&1); printf "<%s>" "$x"'

# 4. >file 2>&1 inside $(): both streams to the file, capture empty.
check 'redir-file-then-dup-builtin'  'x=$(echo hi >/tmp/s0m_o 2>&1); printf "<%s>[%s]" "$x" "$(cat /tmp/s0m_o)"'
check 'redir-file-then-dup-external' 'x=$(/bin/echo hi >/tmp/s0m_o 2>&1); printf "<%s>[%s]" "$x" "$(cat /tmp/s0m_o)"'

# 5. 2>&1 >file inside $(): stderr to capture, stdout to file (reverse ordering).
check 'dup-then-redir-file-builtin'  'x=$(sh -c "echo out; echo err >&2" 2>&1 >/tmp/s0m_o); printf "<%s>[%s]" "$x" "$(cat /tmp/s0m_o)"'
check 'dup-then-redir-file-external' 'x=$(/bin/sh -c "/bin/echo out; /bin/echo err >&2" 2>&1 >/tmp/s0m_o); printf "<%s>[%s]" "$x" "$(cat /tmp/s0m_o)"'

# 6. $(<file) file read.
check 'comsub-file-read' 'printf "abc\n" >/tmp/s0m_f; x=$(</tmp/s0m_f); printf "<%s>" "$x"'

# 7. readonly-arith error captured under $(cmd 2>&1) (the #353 shape).
# bash merges the readonly error into the capture (x="bash: ...: readonly variable").
# huck currently leaks the error onto its own real stdout AHEAD of the printf and
# captures nothing (x=""). Pin huck's CURRENT output; a fork-comsub change should
# either match bash or update this pin consciously.
# STAGE-1 TARGET (#353)
check_pin 'readonly-arith-2>&1' 'x=$(readonly r=1; (( r++ )) 2>&1); printf "<%s>" "$x"' \
  'target/debug/huck: line 1: r: readonly variable
<>' '' '0'

# 8. extra same-family cases.
check 'plain-group'          'x=$( { echo a; } ); printf "<%s>" "$x"'
check 'external-err-escapes' 'x=$(/bin/ls /nonexistent-xyz 2>&1 1>/dev/null); printf "<%s>" "$x"'
check 'multiline-capture'    'x=$(printf "a\nb\n"); printf "<%s>" "$x"'

rm -f /tmp/s0m_be /tmp/s0m_he /tmp/s0m_o /tmp/s0m_f
echo ""; [ "$FAIL" -eq 0 ] && echo "comsub_capture_matrix OK" || { echo "comsub_capture_matrix FAILED" >&2; exit 1; }
