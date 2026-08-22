#!/usr/bin/env bash
# Byte-identical bash<->huck harness for BACKSLASH ESCAPES inside an arithmetic
# body (#624, #700).
#
# bash expands a `$(( … ))` body under DOUBLE-QUOTE rules, and it does so for the
# whole body regardless of any quote span inside it. So ONE table applies
# everywhere:
#
#   \" \\ \$ \`   drop the backslash, keep the character as a PLAIN character
#   \( \)         keep both — the backslash protects the arith delimiter
#   \'            keep both, but the quote is still consumed, so no span opens
#                 (except INSIDE a `'…'` span, where the backslash is literal and
#                 the quote closes the span)
#   anything else keep both
#
# huck pushed the backslash verbatim and let the quote through as a span OPENER,
# so the expression text differed from bash's AND a later live quote toggled that
# span closed instead of opening its own — which surfaced as the wrong EOF
# diagnostic.
#
# COMPARED: the diagnostic with the `$0`/`line N:` prefix stripped, plus stdout
# and status. Driven from a SCRIPT FILE — huck's piped-stdin reader rewrites a
# `\`+newline before the lexer sees it (#701).
#
# NOT compared here:
#   - `$(( 1+\$x ))`: bash's body `1+$x` is right in huck too, but huck's
#     arithmetic tokenizer then accepts `$name` as a variable reference where
#     bash's rejects a bare `$` (#707).
#   - `$(( '))' ))` and `$(( '1\'2' ))`: huck's scan for the closing `))` is
#     quote-blind where bash's honours the span (#708). Pre-existing, unchanged
#     by this work.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# `$1` label, `$2` the arith BODY. Wrapped in `echo $(( … ))` in a script file.
check() {
    local label="$1" b h tmp
    tmp=$(mktemp)
    printf 'echo $(( %s ))\n' "$2" > "$tmp"
    b=$( bash --norc --noprofile "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //'
         bash --norc --noprofile "$tmp" 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //'
         "$HUCK_BIN" "$tmp" 2>/dev/null; echo "EXIT:$?" )
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

# --- outside any quote span ---
check 'bare: \" both ends'  '1+\"2\"'
check 'bare: \" leading'    '\"3\"'
check 'bare: \\ collapses'  '1+\\2'
check 'bare: \` kept plain' '1+\`'
check 'bare: \a verbatim'   '1+\a'
check 'bare: \% verbatim'   '1+\%'
check 'bare: \) protected'  '1+\)'
check 'bare: \( protected'  '1+\('
check 'bare: \\\) is \\ +)' '1+\\\)'
check "bare: \\' verbatim"  "1+\\'2\\'"

# --- inside a double-quoted span ---
check 'dq: \\ collapses'    '1+"\\2"'
check 'dq: \" kept plain'   '1+"\""'
check 'dq: \` kept plain'   '1+"\`"'
check 'dq: \a verbatim'     '1+"\a"'
check 'dq: \) protected'    '1+"\)"'
check "dq: \\' verbatim"    "1+\"\\'\""

# --- inside a single-quoted span: the SAME table applies (#700) ---
check 'sq: \\ collapses'    "1+'\\\\2'"
check 'sq: \" kept plain'   "1+'\\\"'"
check 'sq: \` kept plain'   "1+'\\\`'"
check 'sq: \a verbatim'     "1+'\\a'"
check 'sq: \) protected'    "1+'\\)'"

# --- span PARITY: an escaped quote must not open a span, so a later live quote
#     opens its own and the EOF names IT (#624's second table) ---
check_eof() {
    local label="$1" b h tmp
    tmp=$(mktemp)
    printf 'x=1\ny=2\n%s\n' "$2" > "$tmp"
    b=$(bash --norc --noprofile "$tmp" 2>&1 | sed -z 's/^[^ ]*: //')
    h=$("$HUCK_BIN" "$tmp" 2>&1 | sed -z 's/^[^ ]*: //')
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}
check_eof 'eof: escaped then live "' 'echo $((1+\"a"'
check_eof 'eof: escaped " alone'     'echo $((1+\"'
check_eof "eof: escaped ' alone"     "echo \$((1+\\'"
check_eof 'eof: \\ leaves " live'    'echo $((1+\\"'
check_eof 'eof: live " alone'        'echo $((1+"'

# --- the legacy `$[ … ]` form uses the SAME table (#709); its delimiter cells
#     are `\[` and `\]` rather than `\(` and `\)` ---
lcheck() {
    local label="$1" b h tmp
    tmp=$(mktemp)
    printf 'echo $[ %s ]\n' "$2" > "$tmp"
    b=$( bash --norc --noprofile "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //'
         bash --norc --noprofile "$tmp" 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //'
         "$HUCK_BIN" "$tmp" 2>/dev/null; echo "EXIT:$?" )
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}
lcheck 'legacy: \" both ends' '1+\"2\"'
lcheck 'legacy: \\ collapses'  '1+\\2'
lcheck 'legacy: \$ kept plain' '1+\$x'
lcheck 'legacy: \` kept plain' '1+\`'
lcheck 'legacy: \a verbatim'   '1+\a'
lcheck 'legacy: \% verbatim'   '1+\%'
lcheck 'legacy: \] protected'  '1+\]'
lcheck 'legacy: \[ protected'  '1+\['
lcheck "legacy: \\' verbatim"  "1+\\'2\\'"
lcheck 'legacy: escaped sq'    "\\'"
lcheck 'legacy: escaped dq'    '\"'
lcheck 'legacy: ok plain'      '2+3'
lcheck 'legacy: ok dquoted'    '"2"+1'
lcheck 'legacy: ok hex'        '0x1f'

# --- valid arithmetic is unaffected ---
check 'ok: plain'           '1+2'
check 'ok: dquoted operand' '"2"+1'
check 'ok: parens'          '(1+2)*3'
check 'ok: var'             'x+1'

harness_summary
