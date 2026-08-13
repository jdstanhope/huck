#!/usr/bin/env bash
# Byte-identical bash<->huck harness for input that ends inside an open
# delimiter (#385).
#
# Two divergences, one per driver:
#
#   * a SCRIPT FILE reported the line the scan gave up on rather than the line
#     the delimiter opened on — `echo "a` on line 3 of a 4-line file came out as
#     `line 5`. Single quotes and backticks happened to be right (they are
#     scanned as one atom); `"`, `${` and `$((` were not.
#
#   * PIPED STDIN printed huck's own `syntax error: unexpected end of input`,
#     which bash never says. bash reports the same two shapes here as
#     everywhere else — `unexpected EOF while looking for matching `X'` for a
#     delimiter, `syntax error: unexpected end of file` for an open compound.
#
# `$(` is the delimiter that reports the EOF line rather than its opening line
# in bash (it keeps scanning to end of input before giving up), so it is a row
# in both drivers — a fix that reported "where it opened" for everything would
# break it.
#
# The stdin rows normalise the program NAME only: piped stdin has no argv[0] to
# set, so each shell names itself.
#
# NOT here, all pre-existing and each its own issue: `$[1+`, where bash says
# matching `]` and huck says `)` (#618); an unterminated `case`, which bash
# reports at the NEXT line's token where huck reports at the `case` (#617); and
# `echo ;;`, where huck runs the leading `echo` before rejecting the line
# (#575).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-eof.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# A 4-line file with the offending line THIRD, so a line number that is right
# for the wrong reason (first line, last line, one-past-EOF) still shows up.
check_file() {
    local label="$1" frag="$2" b h
    printf 'echo a\necho b\n%s\necho c\n' "$frag" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check_stdin() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | timeout 10 "$BASH_BIN" --norc --noprofile 2>&1 \
        | sed "s|^$BASH_BIN: |SHELL: |"; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 \
        | sed "s|^$HUCK_BIN: |SHELL: |"; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- a script file: the line the delimiter OPENED on ---
check_file "double quote"   'echo "open'
check_file "single quote"   "echo 'open"
check_file "backtick"       'echo `open'
check_file "param brace"    'echo ${x'
check_file "arith"          'echo $((1+'
check_file "comsub"         'echo $(open'
check_file "quote in comsub" 'echo $(echo "'
check_file "comsub in quote" 'echo "$(echo'
check_file "subscript"      'echo ${x[0'
check_file "quote with space" 'echo "a b'
check_file "compound if"    'if true'
check_file "compound brace" '{ echo hi'
check_file "compound loop"  'while true; do :'
check_file "function body"  'f() {'

# --- piped stdin: the same shapes, not huck's own wording ---
check_stdin "stdin quote"   'echo "open'
check_stdin "stdin squote"  "echo 'open"
check_stdin "stdin backtick" 'echo `open'
check_stdin "stdin brace"   'echo ${x'
check_stdin "stdin arith"   'echo $((1+'
check_stdin "stdin comsub"  'echo $(open'
check_stdin "stdin if"      'if true'
check_stdin "stdin group"   '{ echo hi'
check_stdin "stdin loop"    'while true; do :'
check_stdin "stdin case"    'case x in'
check_stdin "stdin function" 'f() {'
check_stdin "stdin after a command" 'echo a
echo "open'
check_stdin "stdin two before" 'echo a
echo b
echo ${x'
check_stdin "stdin quote spans lines" 'echo a
echo "x
y'

# --- controls: complete input, and errors that are not EOF ---
check_file  "closed quote"  'echo "closed"'
check_file  "closed compound" 'if true; then echo t; fi'
check_stdin "stdin complete" 'echo done'
check_file  "near token"    'if; then'

harness_summary
