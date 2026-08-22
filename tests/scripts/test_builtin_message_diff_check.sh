#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `test`/`[` USAGE MESSAGES (#679).
#
# `test` decides what an expression means — and what its error says — from the
# ARGUMENT COUNT, not by parsing left to right. Two arguments mean "unary
# operator and operand", so a bad one names ARG 0; three mean "operand, binary
# operator, operand", so a bad one names ARG 1; four or more are parsed as an
# expression, and leftovers are `too many arguments` with no argument named.
#
# huck had the right messages already — `evaluate_short_form` computed them —
# and then THREW THEM AWAY: the caller matched `if let Ok(b) = …`, so on an error
# it fell through to the grammar parser, whose own message named a different
# token. `[ $x -eq 1 ]` with `$x` empty is `[ -eq 1 ]`: bash says
# `-eq: unary operator expected`, huck said `1: unexpected argument`.
#
# Four corpus scripts print one of these messages when their inputs are empty
# (open-iscsi's activate-storage.sh and umountiscsi.sh, landscape-sysinfo.wrapper,
# and a docker-entrypoint), which is how the runtime sweep found it.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ⚠️ Status captured BEFORE the normalising pipe — `cmd | sed; echo $?` reports
# sed's status, which would make every rc assertion here vacuous.
# ⚠️ The program-name prefix is normalised: under `-c` bash says `bash:` where
# huck says its own argv[0], an absolute path here.
norm() { sed -E 's#^[^:]*: line #SH: line #'; }
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    out=$("$HUCK_BIN" --norc -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── two arguments: the message names ARG 0 ───────────────────────────────────
check 'empty var, -eq'    'x=; [ $x -eq 1 ]'
check 'empty var, ='      'x=; [ $x = 1 ]'
check 'empty var, =='     'x=; [ $x == 1 ]'
check 'empty var, -lt'    'x=; [ $x -lt 1 ]'
check 'two plain words'   '[ a b ]'
check 'operator first'    '[ -eq 1 ]'
check 'operator second'   '[ a -eq ]'
check 'number first'      '[ 1 -eq ]'
check 'combinator second' '[ a -a ]'
check 'bang then two'     '[ ! a b ]'
check 'open paren alone'  "[ '(' a ]"
check 'both parens'       "[ '(' ')' ]"
check 'test not bracket'  'test a b'

# ── three arguments: the message names ARG 1 ─────────────────────────────────
check 'three plain words' '[ a b c ]'
check 'trailing -a'       '[ -n a -a ]'
check 'unary in middle'   "[ '(' -n a ]"

# ── four or more: parsed, and leftovers are unnamed ──────────────────────────
check 'four words'        '[ a b c d ]'
check 'five words'        '[ a b c d e ]'
check 'parens plus one'   "[ '(' a ')' x ]"
check 'ran out after -a'  '[ a = b -a ]'

# ── the parenthesised readings, which the counts own too ─────────────────────
# ⚠️ `[ ( a b ) ]` reports `a: unary operator expected` — the FOUR-argument
# reading applies the TWO-argument rule to the inside, so the message is about
# `a`, not about parentheses. Getting this wrong is how a naive fix regresses.
check 'paren wraps one'   "[ '(' a ')' ]"
check 'paren wraps two'   "[ '(' a b ')' ]"
check 'paren wraps unary' "[ '(' -n a ')' ]"
check 'nested parens'     "[ '(' '(' a ')' ')' ]"
check 'nested unclosed'   "[ '(' '(' a ')' ]"

# An unclosed `(` in a 4+-argument expression (#688). bash's `, found X` clause
# names the token its parser stopped on, which under `[` is the closing `]` that
# huck strips before evaluating — so `[` carries the clause and `test`, whose
# arguments simply ran out, does not.
check 'unclosed paren, ['  "[ '(' -n a -a -n b ]"
check 'unclosed paren, 6'  "[ '(' -n a -a -n b -a -n c ]"
check 'unclosed paren, eq' "[ '(' 1 -eq 1 ]"
check 'unclosed, nested ('  "[ '(' '(' -n a -a -n b ]"
check  'unclosed paren, test'  "test '(' -n a -a -n b"
check  'unclosed paren, test6' "test '(' -n a -o -n b"
check  'unclosed, test -eq'    "test '(' 1 -eq 1"
# Neighbours that must NOT gain the clause: these end before the `)` is due.
check 'combinator at end'  "[ '(' -n a -a ]"
check  'combinator, test'  "test '(' -n a -a"

# A `[` whose last argument is not `]` (#731). bash quotes the bracket with a
# backtick-then-apostrophe pair, as it does for every other such diagnostic.
check 'no closing bracket'  "[ -n a"
check 'bracket not last'    "[ '(' -n a -a -n b ] extra"
check 'bare word, no ]'     "[ a"
check 'comparison, no ]'    "[ a = a"

# ── controls: valid expressions must not move ────────────────────────────────
check 'one word true'     '[ a ]; echo "st=$?"'
check 'empty is false'    '[ ]; echo "st=$?"'
check 'bare -f is true'   '[ -f ]; echo "st=$?"'
check 'bare -n is true'   '[ -n ]; echo "st=$?"'
check 'negated word'      '[ ! a ]; echo "st=$?"'
check 'double negation'   '[ ! ! a ]; echo "st=$?"'
check 'string equality'   '[ a = a ]; echo "st=$?"'
check 'integer compare'   '[ 1 -eq 1 ]; echo "st=$?"'
check 'and combinator'    '[ -n a -a -n b ]; echo "st=$?"'
check 'or combinator'     '[ a = b -o c = c ]; echo "st=$?"'
check 'long conjunction'  '[ a = b -a c = d ]; echo "st=$?"'
check 'file test'         '[ -f /etc/passwd ]; echo "st=$?"'
check 'dbracket unmoved'  '[[ -n a && 1 -eq 1 ]]; echo "st=$?"'
check 'dbracket pattern'  '[[ abc == a* ]]; echo "st=$?"'

harness_summary
