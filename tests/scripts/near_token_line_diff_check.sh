#!/usr/bin/env bash
# Byte-identical bash<->huck harness for WHICH LINE a near-token syntax error
# names (#617).
#
# bash reports the offending token's OWN line and echoes that line. huck used
# the line the enclosing command STARTED on, which agreed only while the token
# sat on the compound's first line:
#
#     echo a
#     if true
#     then
#     ;;
#     fi
#     bash: line 4: syntax error near unexpected token `;;'   line 4: `;;'
#     huck: line 2: syntax error near unexpected token `;;'   line 2: `if true'
#
# The token is named correctly in both — only the line and the echoed source
# line were wrong, which is exactly the pair a reader uses to find the mistake.
#
# Filed as a `case`-only divergence; measuring showed every multi-line compound
# has it, so the rows below cover `if`, `while`, `until`, `for`, `case`, a brace
# group, a subshell and a function body.
#
# Single-line rows are the controls: they agreed before this change and must
# still agree, since they are the shape where the unit's start and the token's
# line are the same.
#
# NOT here, both pre-existing: `echo ;;` on a line of its own, where huck runs
# the leading `echo` before rejecting the line (#575); and a bare `()`, where
# huck raises its own "empty subshell" variant instead of bash's near-token
# shape (#574's family).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-ntl.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

check_file() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the token is on a LATER line than the compound it sits in ---
check_file "if body"        'echo a
if true
then
;;
fi'
check_file "while body"     'echo a
while true; do
;;
done'
check_file "until body"     'echo a
until false; do
;;
done'
check_file "for body"       'echo a
for i in 1 2; do
;;
done'
check_file "case pattern"   'echo a
echo b
case x in
echo c'
check_file "case body"      'echo a
case x in
  p) echo p;;
  ;;
esac'
check_file "brace group"    'echo a
{
;;
}'
check_file "subshell"       'echo a
(
;;
)'
check_file "function body"  'echo a
f() {
;;
}'
check_file "nested compound" 'echo a
if true; then
  while true; do
  ;;
  done
fi'
check_file "deep in a file" 'echo 1
echo 2
echo 3
echo 4
if true
then
)
fi'

# --- controls: the single-line shapes that already agreed ---
check_file "bare rparen"    'echo )'
check_file "after a command" 'echo a
echo )'
check_file "double pipe"    'echo a
echo | | b'
check_file "unterminated if" 'echo a
if true
then'
check_file "good script"    'echo a
if true; then echo t; fi
case x in p) echo p;; esac'

harness_summary
