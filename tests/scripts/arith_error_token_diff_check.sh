#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the message and error TOKEN of an
# arithmetic error (#659).
#
# bash has no "unexpected character" diagnostic. A character its tokenizer
# cannot use is reported by the PARSER, and the error token is the whole
# REMAINDER of the expression from that character:
#
#     $((1 + @))    syntax error: operand expected (error token is "@")
#     $((2 @ 3))    syntax error: invalid arithmetic operator (error token is "@ 3")
#
# Which of the two messages appears depends on POSITION — after a complete
# operand an operator was due, anywhere else an operand still was. huck emitted
# its own `unexpected character: '@' (error token is "")` for both.
#
# Separately, an unterminated `name[` subscript is bash's dedicated
# `bad array subscript`, whose error token starts at the IDENTIFIER, not the
# bracket. huck fell through to `operand expected` naming just `[`.
#
# COMPARED: the diagnostic with the leading `$0` and `line N:` stripped, plus
# stdout and the exit status. The `$0` prefix diverges for every builtin
# diagnostic and is not this issue.
#
# NOT compared here:
#   - `$((x[]))`: an EMPTY subscript. bash prints `bad array subscript` TWICE
#     with no error-token clause; huck prints one line (#703).
#   - `$((1 + #))`: bash never reaches the arithmetic parser — it fails to find
#     the closing `))` and reports `bad substitution` (#704).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Diagnostic minus the `$0`/`line N:` prefix, plus stdout and status.
check() {
    local label="$1" frag="$2" b h
    b=$( { printf '%s\n' "$frag" | bash --norc --noprofile 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'; } ; \
         printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$( { printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'; } ; \
         printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- an unusable character in OPERAND position ---
check "operand: after +"        'echo $((1 + @))'
check "operand: at start"       'echo $((@))'
check "operand: padded"         'echo $((  @  ))'
check "operand: after unary ~"  'echo $(( 1 + ~@ ))'
check "operand: token is rest"  'echo $((1 + @ + 2))'
check "operand: backslash"      'echo $((1 + \ 2))'
check "operand: single quote"   "echo \$((1 + '2'))"
check "operand: after ("        'echo $(( ( @ ) ))'
check "operand: after *"        'echo $((3 * @))'

# --- the same character in OPERATOR position ---
check "operator: after number"  'echo $((2 @ 3))'
check "operator: after ident"   'x=1; echo $((x @))'
check "operator: after x++"     'x=1; echo $(( x++ @ ))'
check "operator: after ++x"     'x=1; echo $(( ++x @ ))'
check "operator: after x--"     'x=1; echo $(( x-- @ ))'
check "operand: after bare ++"  'echo $(( ++ @ ))'
check "operand: after bare --"  'echo $(( -- @ ))'
# A closing `)` does NOT complete an operand for this decision, where `]` does.
check "operand: after )"        'echo $(( (1) @ 2 ))'
check "operand: after nested )" 'echo $(( ((1)) @ 2 ))'
check "operand: after (1+2)"    'echo $(( (1+2) @ ))'
check "operator: after ]"       'a=(1 2); echo $(( a[0] @ 2 ))'

# --- an unterminated subscript is `bad array subscript`, named from the ident ---
check "subscript: bare name["   'echo $((x[))'
check "subscript: with index"   'echo $((x[1))'
check "subscript: mid-expr"     'echo $((1 + x[))'
check "subscript: nested"       'echo $((x[y[))'

# --- the other arithmetic diagnostics are unchanged ---
check "regress: trailing +"     'echo $((1 +))'
check "regress: trailing **"    'echo $((1 ** ))'
check "regress: div by zero"    'echo $((1 / 0))'
check "regress: unbalanced ("   'echo $(( (1 + 2 ))'
check "regress: assign to num"  'echo $(( 1 = 2 ))'

# --- valid arithmetic still evaluates ---
check "ok: plain"               'echo $((1 + 2 * 3))'
check "ok: indexed element"     'a=(7 8); echo $((a[1]))'
check "ok: assoc element"       'declare -A m=([k]=5); echo $((m[k]))'
check "ok: nested subscript"    'a=(0 5); b=(1); echo $((a[b[0]]))'
check 'ok: legacy $[ ]'         'echo $[ 2 + 3 ]'

harness_summary
