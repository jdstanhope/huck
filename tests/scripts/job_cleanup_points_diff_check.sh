#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #475/#758 (and the rows #476 flaked
# on): WHEN a dead background job is reported and dropped from the table.
#
# bash reaps a child whenever SIGCHLD arrives but reports and prunes at exactly
# five points — the end of a foreground wait, the `jobs` builtin, each loop
# iteration, `wait`, and each new input line the parser reads (`shell_getc`)
# — and what it reports depends on how the shell was
# started: a script file (or piped stdin) reports a background job only when
# a signal killed it, while `-c` marks a normal exit reported silently. So every
# row runs under BOTH drivers: the same fragment as a `-c` string and as a
# script file, each compared to bash under the same driver.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Normalize the program name and the pid in the signal form. The LINE NUMBER
# is deliberately left alone — it is asserted.
norm() { sed -E 's|^[^:]*: line |PROG: line |; s|^(PROG: line [0-9]+: )[0-9]+ |\1PID |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [-c]" "$b" "$h"
    printf '%s\n' "$frag" >"$tmp/f.sh"
    b=$(bash --norc --noprofile "$tmp/f.sh" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" "$tmp/f.sh" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [file]" "$b" "$h"
}

# --- a normal exit: reported at a foreground wait under -c, never in a file --
check "jobs after fg wait"        'sleep 0.1 & sleep 0.3; jobs; echo END'
check "jobs after builtins only"  'sleep 0.1 & sleep 0.3; :; :; jobs; echo END'
check "jobs -p after fg wait"     'sleep 0.1 & sleep 0.3; jobs -p | wc -l'
check "jobs twice"                'sleep 0.1 & sleep 0.3; jobs; echo MID; jobs; echo END'
check "two jobs, one loop"        'sleep 0.1 & sleep 0.2 & sleep 0.4; for i in 1; do :; done; jobs; echo END'
check "listed then loop then wait" 'sleep 0.1 & sleep 0.3; jobs >/dev/null; for i in 1; do :; done; wait %1; echo rc=$?'
check "listed then wait"          'sleep 0.1 & sleep 0.3; jobs >/dev/null; wait %1; echo rc=$?'

# --- wait: the $! job outlives a bare wait in a file ------------------------
check "bare wait then jobs"       'sleep 0.1 & sleep 0.3; wait; jobs; echo END'
check "wait %1 then jobs"         'sleep 0.1 & sleep 0.2 & sleep 0.4; wait %1; jobs; echo END'
check "wait %1 twice"             'sleep 0.1 & wait %1; wait %1; echo rc=$?'
check "wait \$pid twice"          'sleep 0.1 & p=$!; wait $p; wait $p; echo rc=$?'
check "wait \$pid then jobs"      'sleep 0.1 & p=$!; wait $p; jobs; echo END'
check "wait \$pid after fg wait"  'sleep 0.1 & p=$!; sleep 0.3; wait $p; echo rc=$?'
check "wait \$pid after loop"     'sleep 0.1 & p=$!; sleep 0.3; for i in 1; do :; done; wait $p; echo rc=$?'
check "wait -n then jobs"         'sleep 0.1 & sleep 0.2 & wait -n; jobs | wc -l; wait'

# --- #475: a trap interrupts wait; no cleanup point before the pipeline -----
check "interrupted wait"          'trap "echo caught" USR1; sleep 0.6 & ( sleep 0.2; kill -USR1 $$ ) & wait; jobs | wc -l; wait'

# --- #758: %+ keeps pointing at the job after it dies -----------------------
check "wait % after KILL (-m)"    'set -m; sleep 5 & kill -KILL %+ ; wait % 2>/dev/null; echo rc=$?'
check "wait % after KILL"         'sleep 5 & kill -KILL %+ ; wait % 2>/dev/null; echo rc=$?'
check "kill %+ after exit"        'sleep 0.1 & sleep 0.3; kill -0 %+ 2>/dev/null; echo rc=$?'

# --- the signal line needs no job control; the job line does ----------------
check "KILL without -m"           'sleep 5 & kill -KILL %1; sleep 0.3; echo n'
check "HUP without -m"            'sleep 5 & kill -HUP %1; sleep 0.3; echo n'
check "TERM without -m"           'sleep 5 & kill -TERM %1; sleep 0.3; echo n'
# (Killed DURING the wait: a `kill; wait` is racy in bash — a SIGCHLD that
# lands before `wait` starts leaves it nothing to block on, and no report.)
check "KILL during bare wait"     'sleep 5 & p=$!; ( sleep 0.2; kill -KILL $p ) & wait; echo n'
# (`exit` before EOF: reading EOF is itself a line read, so a script that
# simply ends would report the death there — or not, depending on whether the
# SIGCHLD had landed. bash is racy on that; ending explicitly is not.)
check "KILL, builtins only"       'sleep 5 & kill -KILL %1; echo A; echo B; exit'
check "trapped USR1 without -m"   'trap "echo t" USR1; sleep 5 & kill -USR1 %1; sleep 0.3; echo n'
# (Whether bash then reports the KILL of a STOPPED child is a race in bash
# itself — 2 of 4 runs — so that notice is hidden; the `Running` is the row.)
check "STOP without -m"           'sleep 5 & kill -STOP %1; sleep 0.3; jobs; kill -9 %1; wait %1 2>/dev/null; echo rc=$?'

# --- set -m: wait announces the job it collected ----------------------------
check "wait %1 announces (-m)"    'set -m; sleep 0.1 & wait %1; echo x'
check "wait \$pid announces (-m)" 'set -m; sleep 0.1 & wait $!; echo x'
check "wait -n announces (-m)"    'set -m; sleep 0.1 & sleep 0.2 & wait -n; echo x; sleep 0.3; echo y'
check "wait after TERM (-m)"      'set -m; sleep 3 & kill -TERM %1; wait; echo x'
check "jobs marks it (-m)"        'set -m; sleep 0.1 & sleep 0.3; jobs; sleep 0.1; echo y'

# --- a multi-line script: the notice carries the line of the point that saw it
# A death reaped while a foreground child ran is reported at that line; one
# reaped while builtins ran is reported when the parser reads the NEXT line.
# (A `kill` followed by builtins is racy in bash itself — its SIGCHLD lands
# whenever — so the kill happens INSIDE the busy-wait, which pins the death
# to the loop and the report to the line after it.)
check "notice line (-m)"          $'set -m\nsleep 3 &\nkill -TERM %1\nsleep 0.2\necho A\necho B\necho C'
check "notice line, KILL"         $'sleep 3 &\nkill -KILL %1\nsleep 0.2\necho A\necho B\necho C'
check "next-line point (-m)"      $'set -m\nsleep 3 & p=$!\nwhile kill -0 $p 2>/dev/null; do kill -TERM $p 2>/dev/null; done\necho A\necho B'
check "next-line point, KILL"     $'sleep 3 & p=$!\nwhile kill -0 $p 2>/dev/null; do kill -KILL $p 2>/dev/null; done\necho A\necho B'

harness_summary
