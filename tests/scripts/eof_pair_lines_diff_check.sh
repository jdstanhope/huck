#!/usr/bin/env bash
# What the 813-cell matrix cannot see (v362, #643).
#
# `eof_delimiter_matrix_diff_check.sh` places every fragment on ONE line, so it
# proves which delimiter is named and nothing about WHICH LINE — the two halves
# of bash's answer. It also runs a script file only, and compares text only.
# This harness covers the rest:
#
#   1. multi-line inputs, one per pair type — is the reported line the one the
#      pair OPENED on (quotes, `${`, `$((`, `$[`, `v=(`) or the EOF line (`$(`)?
#   2. #629 — `$((1+2)`, which huck re-reads as `$(` and must still report at the
#      arithmetic's line.
#   3. #633 — the EXIT STATUS of an unterminated compound assignment (1, not 2),
#      on every driver, and that a `source`d file's caller survives it.
#   4. the piped-stdin driver, a different top-level reader from a script file.
#   5. Shape 2 controls — the constructs that must NOT become
#      `unexpected EOF while looking for matching X`.
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

# ── driver: a script FILE ─────────────────────────────────────────────────────
# Its own driver lines, per the note in lib/harness.sh. `s.sh` is used unchanged
# by both shells so the program-name prefix is identical and does not have to be
# stripped (stripping it is how a diff harness starts passing for the wrong
# reason).
script_case() { # <label> <body>
    printf '%s\n' "$2" >"$T/s.sh"
    local b h
    b=$(cd "$T" && (ulimit -v 500000; timeout 5 bash --norc --noprofile s.sh) 2>&1; echo "rc=$?")
    h=$(cd "$T" && (ulimit -v 500000; timeout 5 "$HUCK_BIN" s.sh) 2>&1; echo "rc=$?")
    compare "script: $1" "$b" "$h"
}

# ── driver: piped stdin ───────────────────────────────────────────────────────
stdin_case() { # <label> <body>
    local b h
    b=$(printf '%s\n' "$2" | (ulimit -v 500000; timeout 5 bash --norc --noprofile) 2>&1 \
        | sed 's|^bash: ||'; echo "rc=$?")
    h=$(printf '%s\n' "$2" | (ulimit -v 500000; timeout 5 "$HUCK_BIN") 2>&1 \
        | sed "s|^$HUCK_BIN: ||"; echo "rc=$?")
    compare "stdin: $1" "$b" "$h"
}

# ── driver: `-c` ──────────────────────────────────────────────────────────────
dashc_case() { # <label> <body>
    local b h
    b=$( (ulimit -v 500000; timeout 5 bash --norc --noprofile -c "$2") 2>&1 \
        | sed 's|^bash: ||'; echo "rc=$?")
    h=$( (ulimit -v 500000; timeout 5 "$HUCK_BIN" -c "$2") 2>&1 \
        | sed "s|^$HUCK_BIN: ||"; echo "rc=$?")
    compare "-c: $1" "$b" "$h"
}

# ── 1. which LINE each pair reports ───────────────────────────────────────────
# Every body puts the opener on line 2 of a 3-line file, so "the opening line",
# "the first line" and "the EOF line" are three different answers.
script_case 'dquote opens on line 2'   'echo a
echo "abc
echo c'
script_case 'squote opens on line 2'   'echo a
echo '"'"'abc
echo c'
script_case 'brace opens on line 2'    'echo a
echo ${x
echo c'
script_case 'arith opens on line 2'    'echo a
echo $((1+
echo c'
script_case 'legacy opens on line 2'   'echo a
echo $[1+
echo c'
script_case 'backtick opens on line 2' 'echo a
echo `echo
echo c'
script_case 'array literal opens on line 2' 'echo a
v=(p q
echo c'
# The exception: `$(` is reported where input RAN OUT, not where it opened.
script_case 'comsub reports the EOF line' 'echo a
echo $(cmd
echo c'
# A quote INSIDE an arithmetic body reports the quote'"'"'s own line (#621).
script_case 'quote inside arith reports the quote line' 'echo a
echo $((1+
"abc'
# ...but an ESCAPED quote there opens nothing, so the arithmetic answers (#624).
script_case 'escaped quote inside arith names the arith' 'echo a
echo $((1+\"
echo c'

# ── 2. #629 — the re-read `$((` keeps the arithmetic'"'"'s line ────────────────────
script_case 'reread arith, one line'  'echo $((1+2)'
script_case 'reread arith, line 2'    'echo a
echo $((1+2)
echo c'
script_case 'reread arith with a tail' 'echo $((1+2)+3'
# Controls: a real `$(` containing a subshell, and a real `$((`.
script_case 'real comsub with subshell' 'echo a
echo $( (1+2)
echo c'
script_case 'real arith unterminated'   'echo a
echo $(( (1+2)
echo c'

# ── 3. #633 — an unterminated compound assignment ─────────────────────────────
# Status 1, not 2 — and the same on every driver, unlike most of huck'"'"'s fatality
# codes. The message and line are checked here too.
for body in 'v=(' 'v=(a b' 'v=(""' 'v=([0]=x' 'declare -a v=(a' 'v+=(a' \
            'v=('"'"'abc' 'v=("abc' 'v=($((1+' 'v=(${x' '(v=(a' 'f() { v=(a'; do
    script_case "compound assign: $body" "$body"
done
dashc_case 'compound assign status' 'v=(a'
stdin_case 'compound assign status' 'v=(a'
# ...and an ordinary syntax error still exits 2 on the same three drivers.
script_case 'ordinary syntax error status' 'echo "a'
dashc_case  'ordinary syntax error status' 'echo "a'
stdin_case  'ordinary syntax error status' 'echo "a'
# A sourced file'"'"'s caller must SURVIVE it, with the file'"'"'s status.
printf 'v=(a\n' >"$T/bad.sh"
printf 'echo "a\n' >"$T/badq.sh"
for inner in bad.sh badq.sh; do
    b=$(cd "$T" && (ulimit -v 500000; timeout 5 bash --norc --noprofile \
        -c ". ./$inner; echo OUTER=\$?") 2>&1 | sed 's|^bash: ||'; echo "rc=$?")
    h=$(cd "$T" && (ulimit -v 500000; timeout 5 "$HUCK_BIN" \
        -c ". ./$inner; echo OUTER=\$?") 2>&1 | sed "s|^$HUCK_BIN: ||"; echo "rc=$?")
    compare "source: caller survives $inner" "$b" "$h"
done

# ── 4. #634 — a `$(` in `${` name position swallows the `}` ───────────────────
script_case 'name-position comsub, EOF'      'echo ${$(echo x'
script_case 'name-position comsub eats the brace' 'echo ${$(echo x}'
script_case 'name-position arith, EOF'       'echo ${$((1+'
script_case 'name-position arith eats the brace'  'echo ${$((1+1)}'
# Controls: terminated forms stay bad substitutions, and the `$` name survives.
script_case 'name-position comsub terminated' 'echo ${$(echo x)}; echo after'
script_case 'name-position arith terminated'  'echo ${$((1+1))}; echo after'
script_case 'dollar as a name'                'echo ${$x}'

# ── 5. Shape 2 controls ───────────────────────────────────────────────────────
# These must stay `syntax error: unexpected end of file`. The `(` pair is the
# point: a SUBSHELL `(` is Shape 2 while an array literal'"'"'s `(` is Shape 3, and
# `Delim::Paren`/`Delim::ArrayParen` exist to keep them apart.
for body in 'if true' 'while :' 'until :' 'case x in' '{ echo hi' '( echo hi' \
            'f() {' 'for i in a' 'if true; then'; do
    script_case "shape 2: $body" "$body"
done

harness_summary
