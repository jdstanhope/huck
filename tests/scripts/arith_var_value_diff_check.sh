#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a variable's VALUE being an arithmetic
# EXPRESSION, not an integer literal (#677).
#
# In bash, a name referenced from arithmetic has its value recursively
# evaluated: `x=010` is octal 8, `x=0x10` is 16, `x=1+1` is 2, and `x=y`
# resolves `y` in turn. huck parsed the value with a decimal integer parse, so
# `010` came out as **10** — silently wrong, no error — and every other form was
# a syntax error. Direct literals were always fine, which is why nothing caught
# it until a real script (`/usr/bin/usb-devices`, masking a hex field out of
# /sys) printed the wrong number.
#
# ⚠️ The recursion guard is part of the same fix and is pinned here too: before
# it, `x=x; echo $((x))` recursed until the process died with
# `thread 'main' has overflowed its stack`, taking the whole shell with it.
# Adding recursion without a cap would have SHIPPED that crash to scalars, where
# a self-reference is far likelier to be typed than in an array element.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ⚠️ Status captured BEFORE any pipe — `cmd | sed; echo $?` reports sed's status.
#
# ⚠️ The program-name prefix is normalised away: under `-c` bash says `bash:`
# while huck says its own argv[0], which is the absolute path this harness
# invoked. That is a difference about how the binary was found, not about the
# message, and it would fail every error row for the wrong reason.
norm() { sed -E 's#^[^:]*: line #SH: line #'; }
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    out=$("$HUCK_BIN" --norc -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── the base prefixes, through a variable ─────────────────────────────────────
check 'octal via var'        'x=010; echo $((x))'
check 'octal arithmetic'     'x=010; echo $((x+1))'
check 'hex via var'          'x=0x10; echo $((x+1))'
check 'hex zero-padded'      'x=0x0002; echo $(( x & 0x7ff ))'
check 'explicit base'        'x=8#7; echo $((x))'
check 'base 10 explicit'     'x=10#08; echo $((x+1))'
check 'negative octal'       'x=-010; echo $((x))'

# ── a value that is an EXPRESSION, or another name ────────────────────────────
check 'expression value'     'x=1+1; echo $((x))'
check 'name chain'           'a=b; b=c; c=7; echo $((a))'
check 'surrounding spaces'   'x=" 0x10 "; echo $((x+1))'
check 'parenthesised'        'x="(2+3)*4"; echo $((x))'

# ── every arithmetic CONTEXT reads values the same way ────────────────────────
check 'declare -i'           'x=010; declare -i y=x; echo $y'
check 'dparen command'       'x=0x10; (( y = x + 1 )); echo $y'
check 'let'                  'x=010; let "y = x"; echo $y'
check 'increment'            'x=0x10; echo $((x++)); echo $x'
check 'indexed element'      'declare -a a=(010 0x10); echo $((a[0])) $((a[1]))'
check 'assoc element'        'declare -A m=([k]=0x10); echo $((m[k]))'
check 'ternary'              'x=010; echo $(( x > 8 ? 1 : 0 ))'
check 'array subscript'      'declare -a a=(9 8 7); i=0x1; echo ${a[i]}'

# ── the guard: a value that refers to itself ──────────────────────────────────
# Errors, and the shell SURVIVES to run the next command — which is the whole
# point. `echo AFTER` is the row's real assertion.
check 'self reference'       'x=x; echo $((x)); echo AFTER'
check 'self ref element'     'declare -a a; a[0]="a[0]"; echo $((a[0])); echo AFTER'
#
# ⚠️ NOT compared: a MUTUAL chain (`x=y; y=x`). Both shells report the recursion
# and survive, but bash names the INNERMOST variable (`y: … error token is "y"`)
# and huck names the one the expression started from (`x`). Matching that means
# carrying the inner expression's source out through the error, which is a
# message-fidelity change, not part of getting values right. The single-name
# case above — the one anybody actually types — matches bash exactly.
#
# ⚠️ Nor is a chain DEEPER THAN 64 compared: bash's cap is 1024 and huck's is
# 64. A debug build overflows between depth 350 and 400 on the main thread, so a
# 1024 cap would never be reached — and a libtest thread gets 2 MiB where the
# main thread gets 8 MB, so even 128 overflowed there. Recorded in
# docs/bash-divergences.md.

# ── controls: direct literals were never broken and must not move ─────────────
check 'literal octal'        'echo $((010+1))'
check 'literal hex'          'echo $((0x10+1))'
check 'literal base'         'echo $((8#7))'
check 'plain decimal'        'i=5; echo $((i+1))'
check 'empty is zero'        'x=; echo $((x))'
check 'unset is zero'        'unset x; echo $((x))'
check 'name of unset'        'x=abc; echo $((x))'
check 'leading plus'         'x=+5; echo $((x))'

harness_summary
