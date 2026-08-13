#!/usr/bin/env bash
# Byte-identical bash<->huck harness for input that ends inside a QUOTE that is
# itself inside an arithmetic body (#621).
#
# bash names the innermost open delimiter, which for `echo $((1+"` is the quote,
# and reports it at the line the QUOTE opened on:
#
#     echo $((1 +
#     2 +
#     "abc
#     bash: line 3: unexpected EOF while looking for matching `"'
#     huck: line 1: unexpected EOF while looking for matching `)'
#
# Both the delimiter and the line were wrong: huck's arith scanner raised its own
# unterminated-arith error at end of input whether or not a quote span was open
# inside it, so the diagnostic pointed at the `$((` instead of the quote.
#
# `$((`, `$[` and `((` all share that scanner, so all three are rows here. The
# rows where the quote opens on a LATER line than the arith are the ones that
# pin the line, and "quote spans a $x" pins that the span survives a sub-parse
# (a `$`/backtick inside the body returns mid-scan, so the opening offset has to
# ride on the mode frame, not on the current scan call).
#
# Controls: a CLOSED quote followed by an unterminated arith must still name the
# arith delimiter (`)` / `]`), which is what stops a fix from simply always
# blaming a quote.
#
# NOT here, pre-existing and each its own issue: a BACKSLASH-escaped quote,
# where huck keeps the `\` and opens a span bash never opens — `echo $((1+\"a"`
# leaves huck naming `)` where bash names the second quote (#624; the rows below
# do cover the escaped-quote shapes huck gets right, since the fix must not
# regress them); and an unterminated `for (( … ))` header, which reports huck's
# generic wording at the last line (#625).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-aeq.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# The fragment goes THIRD in a 4-line file, so a line number that is right for
# the wrong reason (first line, last line, one-past-EOF) still shows up.
check_file() {
    local label="$1" frag="$2" b h
    printf 'echo a\necho b\n%s\necho c\n' "$frag" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# A whole file verbatim, for the multi-line bodies where the arith and the quote
# open on DIFFERENT lines.
check_raw() {
    local label="$1" src="$2" b h
    printf '%s\n' "$src" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Piped stdin is a different top-level reader (it re-lexes the buffer to classify
# the incompleteness), so the same shapes run through it too. It has no argv[0]
# to set, so each shell names itself — normalise the program name only.
check_stdin() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | timeout 10 "$BASH_BIN" --norc --noprofile 2>&1 \
        | sed "s|^$BASH_BIN: |SHELL: |"; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 \
        | sed "s|^$HUCK_BIN: |SHELL: |"; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the quote is the innermost open delimiter: bash names it, not the arith ---
check_file "dquote in arith"      'echo $((1+"'
check_file "squote in arith"      "echo \$((1+'"
check_file "dquote in legacy"     'echo $[1+"'
check_file "squote in legacy"     "echo \$[1+'"
check_file "dquote in arith cmd"  '((1+"'
check_file "squote in arith cmd"  "((1+'"
# The frame starts OUTSIDE the word's own quote (every push site seeds
# `in_dquote: false`), so a span opened inside the body wins even when the whole
# word is already quoted — bash agrees, naming the inner quote in both.
check_file "dquote inside a dquoted word" 'echo "$((1+"'
check_file "squote inside a dquoted word" "echo \"\$((1+'"
check_file "dquote in legacy in dquote"   'echo "$[1+"'
check_file "in an assignment"     'x=$((1+"'
check_file "quote spans a paren"  'echo $((1+"a(b'
check_file "squote literal in a dquote span" 'echo $((1+"a'"'"'b'
check_file "dquote literal in a squote span" "echo \$((1+'a\"b"
check_file "quote in a nested arith" 'echo $(($((1+"'

# --- escaped quotes: bash keeps these on the ARITH delimiter (#624) ---
check_file "escaped dquote"       'echo $((1+\"'
check_file "escaped squote"       "echo \$((1+\\'"
check_file "escaped dquote legacy" 'echo $[1+\"'
# An EVEN backslash run escapes the BACKSLASH, so the quote is live again.
check_file "double backslash then dquote" 'echo $((1+\\"'
check_file "escaped dquote inside a span" 'echo $((1+"a\"'

# --- the line: the quote opens LATER than the arith ---
check_raw "arith L2, quote L4" 'echo a
echo $((1 +
2 +
"abc
3 +'
check_raw "legacy L2, quote L4" 'echo a
echo $[1 +
2 +
"abc'
check_raw "arith cmd L2, quote L3" 'echo a
((1 +
"abc'
# A `$x` inside the span returns mid-scan, so the opening offset must survive on
# the mode frame — with a per-call offset this reports the `$((` line instead.
check_raw "quote spans a \$x sub-parse" 'echo a
echo $((1 +
"abc
$x def'
check_raw "quote spans a comsub" 'echo a
echo $((1 +
"abc
$(echo hi) def'
# The SECOND span is the open one: the offset tracks the current span, not the
# first quote ever seen in the body.
check_raw "second span opens later" 'echo a
echo $((1 + "a" +
"b'
check_raw "second span, legacy" 'echo a
echo $[1 + "a" +
"b'

# --- piped stdin: same shapes through the other reader ---
check_stdin "stdin dquote in arith"  'echo $((1+"'
check_stdin "stdin squote in arith"  "echo \$((1+'"
check_stdin "stdin dquote in legacy" 'echo $[1+"'
check_stdin "stdin arith cmd"        '((1+"'
check_stdin "stdin quote opens later" 'echo a
echo $((1 +
"abc'

# --- controls: a CLOSED quote leaves the arith delimiter to be named ---
check_file "closed dquote, arith open"  'echo $((1+"x"'
check_file "closed squote, arith open"  "echo \$((1+'x'"
check_file "closed dquote, legacy open" 'echo $[1+"x"'
check_file "no quote at all"            'echo $((1+'
check_file "no quote, legacy"           'echo $[1+'
check_file "no quote, arith cmd"        '((1+'
check_file "quote and arith both closed" 'echo $((1+"2"))'
check_file "legacy both closed"          'echo $[1+"2"]'
check_stdin "stdin closed quote, arith open" 'echo $((1+"x"'
check_stdin "stdin complete arith"           'echo $((1+"2"))'

harness_summary
