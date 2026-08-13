#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `-n` / `set -n` (noexec / parse-only).
#
# TWO sections, and the split matters:
#
#   1. The original rows (stdout + exit code, stderr SUPPRESSED). parse-only's
#      surface contract is "did anything reach stdout, and what was the exit
#      code" — a valid script runs nothing and exits 0, a syntax error exits 2.
#      The syntax-error DIAGNOSTIC text legitimately differs (huck's parser
#      messages never byte-match bash's), so stderr is deliberately not compared
#      for those rows.
#   2. The #636 rows, which compare a SIDE EFFECT.
#
# Section 1 passed throughout the bug in section 2 — which is the point. huck RAN
# a backgrounded command when it was the last command in the input:
#
# `-n` parses without running anything. huck RAN a backgrounded command when it
# was the last command in the input:
#
#     printf 'touch MARK &\n' > f.sh
#     bash -n f.sh   # no MARK
#     huck -n f.sh   # MARK created
#
# This is not a cosmetic divergence. `-n` is what you run on a script you do not
# trust, and `tools/parse_sweep.sh` runs it over every shell script on the
# machine on exactly that basis. It reached a USB/IP example whose last line is
# `usbipd --device &` and huck STARTED THE DAEMON, which then held the sweep's
# stderr pipe open and wedged it permanently.
#
# Two paths reach a background launch without passing `run_command`, where the
# noexec gate lived: `execute_with_sink_inner`'s trailing-`&` fast path (which
# ran the command outright) and `execute_sequence_body`'s backgrounded-group
# arm (which forked a child that then skipped its own body, so it left no trace
# but still created a process). The gate now sits at the sequence entry, so
# neither runs and neither forks.
#
# Section 2's rows therefore check a SIDE EFFECT, not just output: each fragment
# tries to create a marker file, and the compared text carries `MARK=yes|no`. A
# text-only comparison scores the bug as a PASS — both shells print nothing and
# exit 0, which is exactly why section 1 never caught it.
#
# The shapes that were already safe are rows too: they are what pins the fix to
# the two bypasses rather than to "background commands" in general.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-noexec.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# Run one fragment under `-n` in a scratch dir and report output, status and
# whether the marker survived. `sleep 0.3` gives a wrongly-launched background
# child time to land before the check.
check_n() {
    local label="$1" frag="$2" b h
    b=$(cd "$TMPROOT" && rm -f MARK && printf '%s\n' "$frag" >f.sh \
        && timeout 10 "$BASH_BIN" --norc --noprofile -n f.sh 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    h=$(cd "$TMPROOT" && rm -f MARK && printf '%s\n' "$frag" >f.sh \
        && timeout 10 "$HUCK_BIN" -n f.sh 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    compare "$label" "$b" "$h"
}

# `-c` and piped stdin are different top-level readers in huck, so `-n` is
# checked through each of them too.
check_n_c() {
    local label="$1" frag="$2" b h
    b=$(cd "$TMPROOT" && rm -f MARK; timeout 10 "$BASH_BIN" --norc --noprofile -n -c "$frag" 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    h=$(cd "$TMPROOT" && rm -f MARK; timeout 10 "$HUCK_BIN" -n -c "$frag" 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    compare "$label" "$b" "$h"
}

check_n_stdin() {
    local label="$1" frag="$2" b h
    b=$(cd "$TMPROOT" && rm -f MARK; printf '%s\n' "$frag" | timeout 10 "$BASH_BIN" --norc --noprofile -n 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    h=$(cd "$TMPROOT" && rm -f MARK; printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" -n 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    compare "$label" "$b" "$h"
}

# `set -n` / `set -o noexec` reached from INSIDE the script, with no `-n` flag.
check_set_n() {
    local label="$1" frag="$2" b h
    b=$(cd "$TMPROOT" && rm -f MARK && printf '%s\n' "$frag" >f.sh \
        && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    h=$(cd "$TMPROOT" && rm -f MARK && printf '%s\n' "$frag" >f.sh \
        && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?"; \
        sleep 0.3; [ -e "$TMPROOT/MARK" ] && echo "MARK=yes" || echo "MARK=no")
    compare "$label" "$b" "$h"
}

# ── Section 1: the original rows — stdout + exit code, stderr suppressed ──────

# With the -n flag (parse-only).
chkn() {
    local l="$1" f="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -n -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -n -c "$f" 2>/dev/null; echo "EXIT:$?")
    compare "$l" "$b" "$h"
}

# Without the flag (exercises `set -n` taking effect mid-script).
chk() {
    local l="$1" f="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    compare "$l" "$b" "$h"
}

# --- valid input: parses clean, runs nothing (empty stdout), rc 0 ---
chkn "valid simple"      'echo SHOULD_NOT_RUN'
chkn "valid for"         'for i in 1 2 3; do echo "$i"; done'
chkn "valid if"          'if true; then echo x; else echo y; fi'
chkn "valid func"        'f(){ local v=1; echo "$v"; }; f'
chkn "valid case"        'case $z in a) echo a;; *) echo def;; esac'
chkn "valid while"       'while read l; do echo "$l"; done'
chkn "valid coproc"      'coproc cat; echo "${COPROC[0]}"'
chkn "valid pipeline"    'echo a | tr a-z A-Z | cat'
chkn "valid subshell"    '( cd /tmp && echo here )'
chkn "valid redirects"   'echo x >/tmp/zz 2>&1; exec 3<&-'

# --- invalid input: syntax error, rc 2 (stderr wording differs, not compared) ---
chkn "err if-then"       'if then'
chkn "err for-no-in"     'for x in'
chkn "err lone done"     'done'
chkn "err unbalanced ("  '( echo unbalanced'
chkn "err case open"     'case x in'
chkn "err unterminated " 'echo "open quote'

# --- set -n taking effect mid-script (no -n flag) ---
chk  "set -n stops after"   'echo a; set -n; echo b; echo c'
chk  "set -n then +n stays" 'set -n; set +n; echo hi'
chk  "no set -n runs all"   'echo one; echo two'

# ── Section 2 (#636): side effects, which section 1 cannot see ────────────────

# --- the bug: a background command ending the input ---
check_n "trailing background"      'touch MARK &'
check_n "background pipeline"      'touch MARK | cat &'
check_n "background after another line" 'echo hi
touch MARK &'
check_n "background then a comment" 'touch MARK &
# trailing comment'
check_n "background then a blank line" 'touch MARK &
'
check_n "two backgrounds"          'true &
touch MARK &'
check_n "background with a redirect" 'touch MARK >/dev/null &'
check_n "background assignment+cmd" 'V=1 touch MARK &'

# --- the same through the other readers ---
check_n_c     "-c trailing background"    'touch MARK &'
check_n_c     "-c background pipeline"    'touch MARK | cat &'
check_n_stdin "stdin trailing background" 'touch MARK &'
check_n_stdin "stdin background pipeline" 'touch MARK | cat &'

# --- noexec turned on from inside the script ---
check_set_n "set -n then background"        'set -n
touch MARK &'
check_set_n "set -o noexec then background" 'set -o noexec
touch MARK &'
check_set_n "set -n then background pipeline" 'set -n
touch MARK | cat &'
check_set_n "set -n same line as background"  'set -n; touch MARK &'

# --- shapes that were already safe: they must stay safe ---
check_n "background then a command" 'touch MARK &
echo done'
check_n "background then cmd, same line" 'touch MARK & true'
check_n "command then background, same line" 'echo hi; touch MARK &'
check_n "backgrounded subshell"    '( touch MARK ) &'
check_n "backgrounded brace group" '{ touch MARK ; } &'
check_n "backgrounded and-or"      'true && touch MARK &'
check_n "backgrounded loop"        'for i in 1; do touch MARK; done &'
check_n "backgrounded if"          'if true; then touch MARK; fi &'
check_n "foreground command"       'touch MARK'
check_n "foreground pipeline"      'touch MARK | cat'

# --- noexec still reports syntax errors, and still parses the whole input ---
check_n "syntax error after background" 'touch MARK &
if true'
check_n "syntax error only"        'if true'
check_n "clean script"             'echo hi
if true; then echo t; fi'

# --- without -n the command MUST run: the fix must not disable backgrounding ---
check_set_n "no noexec, background runs" 'touch MARK &
wait'
check_set_n "no noexec, pipeline runs"   'touch MARK | cat &
wait'

harness_summary
