#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `fg`/`bg` WITHOUT job control
# (#518, #416).
#
# bash's `fg_bg` refuses outright when the shell has no job control, before it
# parses options and before it resolves a job spec — so every form collapses to
# one line, `fg: no job control`, status 1. huck used to parse first and answer
# whatever the parse found: `-Q: invalid option` plus a usage line at status 2,
# or `%1: no such job` at status 1.
#
# A shell has no job control when it is non-interactive without `set -m` — and
# also inside a SUBSHELL, even one whose parent has `set -m`. That second half
# is why a `bg` piped into anything reports "no job control" in bash: the pipe
# stage is a subshell. It is also what made resuming a stopped job untestable
# in bg_current_job_diff_check.sh.
#
# Both shells run with an EXPLICIT $0 ("huck5") so the error prologue matches
# byte for byte.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

# DRIVER: `-c` with an explicit $0.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- no job control: every form is the same one line ---
check "fg bare"            'fg'
check "bg bare"            'bg'
check "fg %1"              'fg %1'
check "bg %1"              'bg %1'
check "fg bad option"      'fg -Q'
check "bg bad option"      'bg -Q'
check "fg two bad options" 'fg -Q -R'
check "fg --"              'fg --'
check "fg lone dash"       'fg -'
check "bg lone dash"       'bg -'
check "fg many operands"   'fg a b c'
check "bg many operands"   'bg a b c'
check "fg nonexistent pid" 'fg 999999'
check "bg nonexistent pid" 'bg 999999'

# --- a SUBSHELL has no job control even when the parent enabled it (#416) ---
check "fg in subshell"     '( fg %1 )'
check "bg in subshell"     '( bg %1 )'
check "set -m subshell fg" 'set -m; ( fg %1 )'
check "set -m subshell bg" 'set -m; ( bg %1 )'
check "bg into a pipe"     'set -m; sleep 0.2 & { bg %1; } | cat'
check "fg into a pipe"     'set -m; { fg %1; } | cat'

# --- with `set -m` the ordinary parse/resolve path is back ---
check "set -m fg bare"     'set -m; fg'
check "set -m bg bare"     'set -m; bg'
check "set -m fg %1"       'set -m; fg %1'
check "set -m bg %1"       'set -m; bg %1'
check "set -m fg bad opt"  'set -m; fg -Q'
check "set -m bg bad opt"  'set -m; bg -Q'
check "set -m live job"    'set -m; sleep 0.2 & bg %1'

# --- controls: the neighbouring job builtins are NOT gated on job control ---
check "jobs no jobcontrol"   'jobs; echo "rc=$?"'
check "disown no jobcontrol" 'disown; echo "rc=$?"'
check "wait no jobcontrol"   'wait; echo "rc=$?"'
check "jobs bad option"      'jobs -Q'
check "disown bad option"    'disown -Q'

harness_summary
