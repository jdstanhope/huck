#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `return` where it is not legal (#678).
#
# bash allows `return` only inside a function or a SOURCED script. At the top
# level of an EXECUTED script it prints a diagnostic, sets `$?` to 2, and RUNS
# THE NEXT COMMAND. huck returned a function-return outcome, which the top-level
# driver read as "stop" — so a stray `return` silently swallowed the entire rest
# of the script, with no message to say why.
#
# Found by the runtime sweep: `initramfs-tools/scripts/local-top/cryptroot` has
# such a `return` in a guard near the top, and huck ran one line of it where bash
# ran the whole thing.
#
# A SCRIPT FILE is the driver for most rows: "does the rest of the script still
# run" is the assertion, and that needs a file with lines after the `return`.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
# ⚠️ Status captured BEFORE any pipe — `cmd | sed; echo $?` reports sed's status.
# ⚠️ The script PATH is normalised out of the diagnostic prologue: both shells
# print the path they were given, and the temp dir differs per run.
check() {
    local label="$1" body="$2" b h out rc
    printf '%s\n' "$body" >"$T/s.sh"
    out=$(cd "$T" && bash s.sh 2>&1); rc=$?
    b=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    out=$(cd "$T" && "$HUCK_BIN" s.sh 2>&1); rc=$?
    h=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── the bug: the rest of the script must still run ────────────────────────────
check 'top level continues' 'echo one
return 5
echo "st=$?"
echo two'
check 'status is 2'         'return 5
echo "st=$?"'
check 'bare return'         'echo one
return
echo "st=$?"'
check 'return --'           'echo one
return -- 5
echo "st=$?"'
check 'inside if'           'if true; then return 1; fi
echo "st=$?"'
check 'inside a loop'       'for i in 1 2; do return 1; done
echo "st=$?"'
check 'in a subshell'       '( return 1 ); echo "st=$?"'
check 'twice'               'return 1
return 2
echo "st=$?"'

# ── argument errors, and their ORDER relative to the context error ────────────
# ⚠️ Measured, not chosen: `return 1 2` reports ONLY "too many arguments", while
# `return abc` reports BOTH "numeric argument required" AND the context error,
# in that order. (A first reading of this said bash printed only the context
# message — that was a `tail -2` cutting the first line off the measurement.)
check 'bad number + context' 'echo one
return abc
echo "st=$?"'
#
# ⚠️ NOT compared: `return 1 2` (too many arguments) in a SCRIPT FILE. bash
# continues there with `$?` = 1 and stops under `-c`; huck stops in both,
# because that check predates v358's error-fatality classifier and hardcodes a
# fatal outcome. Different root from this issue — the message and status are
# already right, only the per-driver fatality is wrong — so it is #683 rather
# than a row here. huck and bash DO agree under `-c`, which is what makes the
# driver the whole difference.

# ── controls: where `return` IS legal, nothing may change ─────────────────────
check 'in a function'        'f() { echo in; return 5; echo NOPE; }
f
echo "fn=$?"'
check 'nested function'      'f() { g() { return 3; }; g; echo "g=$?"; }
f'
check 'function bare return' 'f() { false; return; }
f
echo "fn=$?"'
check 'sourced script'       'echo body > inner.sh
echo "return 7" >> inner.sh
echo "echo NOPE" >> inner.sh
. ./inner.sh
echo "src=$?"'
check 'sourced bad number'   'echo "return abc" > inner.sh
. ./inner.sh
echo "src=$?"'
check 'function in sourced'  'printf "f(){ return 4; }\nf\necho \"in=\$?\"\n" > inner.sh
. ./inner.sh
echo "src=$?"'
check 'errexit + return'     'set -e
echo one
return 5
echo two'

harness_summary
