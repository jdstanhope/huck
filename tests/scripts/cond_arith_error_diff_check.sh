#!/usr/bin/env bash
# Byte-identical bash<->huck harness for an ARITHMETIC failure inside `[[ … ]]`
# or a `for (( … ))` header (#711).
#
# bash renders the same diagnostic everywhere arithmetic runs: the echoed
# EXPRESSION, the message, and the `(error token is "…")` clause. `$(( ))`,
# `(( ))` and `let` already did that in huck; these two call sites printed the
# bare message with neither, because they rendered the legacy `Display` impl
# instead of `arith::render_error_body`:
#
#     [[ @ -eq 5 ]]   bash: [[: @: syntax error: operand expected (error token is "@")
#                     huck: [[: syntax error: operand expected
#
# `[[ ]]` also used one exit status for every failure. bash does not: an
# ARITHMETIC failure is a false-with-error and exits 1, while a bad regex is a
# usage error and exits 2. huck exited 2 for both.
#
# COMPARED: the diagnostic with the `$0`/`line N:` prefix stripped, plus stdout
# and status.
#
# NOT compared here:
#   - an unmatched `[` in a `[[ == ]]` PATTERN: bash treats it as a literal
#     character; huck reports `bad pattern` and exits 2 — and four of huck's
#     five pattern consumers get that rule wrong (#717).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

check() {
    local label="$1" frag="$2" b h
    b=$( bash --norc --noprofile -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         bash --norc --noprofile -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         "$HUCK_BIN" -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    compare "$label" "$b" "$h"
}

# --- `[[ ]]` arithmetic comparisons ---
check '[[: bad lhs'            '[[ @ -eq 5 ]]; echo "rc=$?"'
check '[[: bad rhs'            '[[ 5 -eq @ ]]; echo "rc=$?"'
check '[[: trailing +'         '[[ 1+ -eq 5 ]]; echo "rc=$?"'
check '[[: division by 0'      '[[ 1/0 -eq 5 ]]; echo "rc=$?"'
check '[[: modulo by 0'        '[[ 1%0 -eq 5 ]]; echo "rc=$?"'
check '[[: -ne operator'       '[[ @ -ne 5 ]]; echo "rc=$?"'
check '[[: -lt operator'       '[[ @ -lt 5 ]]; echo "rc=$?"'
check '[[: -ge operator'       '[[ @ -ge 5 ]]; echo "rc=$?"'
check '[[: rhs of an &&'       '[[ 1 -eq 1 && @ -eq 5 ]]; echo "rc=$?"'
check '[[: rhs of an ||'       '[[ 1 -eq 2 || @ -eq 5 ]]; echo "rc=$?"'

# --- a failed operand is a VALUE, so composition carries on (#718): `!` inverts
#     its non-zero, `||` still evaluates the right operand, `&&` short-circuits
#     and keeps it. The diagnostic is emitted once, at the operand, even when the
#     result is later inverted or discarded. ---
check 'compose: ! a failure'   '[[ ! @ -eq 5 ]]; echo "rc=$?"'
check 'compose: ! ! a failure' '[[ ! ! @ -eq 5 ]]; echo "rc=$?"'
check 'compose: ! bad regex'   '[[ ! a =~ [ ]]; echo "rc=$?"'
check 'compose: || rhs true'   '[[ @ -eq 5 || 1 -eq 1 ]]; echo "rc=$?"'
check 'compose: || rhs false'  '[[ @ -eq 5 || 1 -eq 2 ]]; echo "rc=$?"'
check 'compose: || bad regex'  '[[ a =~ [ || 1 -eq 1 ]]; echo "rc=$?"'
check 'compose: && keeps 2'    '[[ a =~ [ && 1 -eq 1 ]]; echo "rc=$?"'
check 'compose: && lhs failed' '[[ @ -eq 5 && 1 -eq 1 ]]; echo "rc=$?"'
check 'compose: parenthesised' '[[ ( @ -eq 5 ) || 1 -eq 1 ]]; echo "rc=$?"'
check 'compose: not evaluated' '[[ 1 -eq 1 || a =~ [ ]]; echo "rc=$?"'
check 'compose: reported once' '[[ ! @ -eq 5 ]] 2>&1 | wc -l'

# --- a `for (( … ))` header, in each of its three sections ---
check 'for: bad init'          'for (( i=@; i<1; i++ )); do :; done; echo "rc=$?"'
check 'for: bad cond'          'for (( i=0; @; i++ )); do :; done; echo "rc=$?"'
check 'for: bad step'          'for (( i=0; i<1; @ )); do :; done; echo "rc=$?"'
check 'for: division by 0'     'for (( i=1/0; i<1; i++ )); do :; done; echo "rc=$?"'
check 'for: cond fails later'  'for (( i=0; i<3; i++ )); do echo $i; (( i==1 )) && break; done; echo "rc=$?"'

# --- an ERE bash cannot compile: exit 2 and say NOTHING (#716) ---
check 'regex: unclosed class' '[[ a =~ [ ]]; echo "rc=$?"'
check 'regex: leading *'      '[[ a =~ *x ]]; echo "rc=$?"'
check 'regex: unclosed brace' '[[ a =~ "a{" ]]; echo "rc=$?"'

# --- the sites that already agreed must keep agreeing ---
check 'regress: $(( ))'        'echo $(( @ )); echo "rc=$?"'
check 'regress: (( ))'         '(( @ )); echo "rc=$?"'
check 'regress: let'           'let "@"; echo "rc=$?"'

# --- valid conditionals and loops are unaffected ---
check 'ok: true comparison'    '[[ 5 -eq 5 ]]; echo "rc=$?"'
check 'ok: false comparison'   '[[ 5 -eq 6 ]]; echo "rc=$?"'
check 'ok: arith operands'     '[[ 2+3 -eq 5 ]]; echo "rc=$?"'
check 'ok: variable operand'   'x=4; [[ x -lt 5 ]]; echo "rc=$?"'
check 'ok: unset is zero'      '[[ nosuchvar -eq 0 ]]; echo "rc=$?"'
check 'ok: for loop runs'      'for (( i=0; i<3; i++ )); do echo $i; done; echo "rc=$?"'
check 'ok: for empty cond'     'for (( i=0; ; i++ )); do echo $i; (( i==1 )) && break; done'
check 'ok: string compare'     '[[ abc == abc ]]; echo "rc=$?"'
check 'ok: regex match'        '[[ abc =~ ^a ]]; echo "rc=$?"'

harness_summary
