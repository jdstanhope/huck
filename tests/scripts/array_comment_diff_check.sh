#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v183: `#` comments inside an array
# literal `name=( … )`. A comment at element-start runs to end-of-line; a `)`/`(`
# inside it must NOT be read as elements or close the array (huck used to
# mis-parse → "expected a command": kernel ioam6.sh / sysctl.sh commented rows).
# Mid-word `#` and `$#` stay literal. rc 0 in bash → compare full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "comment with paren and close" 'a=(
  1
  # note (paren) and ) here
  2
); echo "${a[@]}"'
check "comment right after open"     'a=(  # lead comment )
  x y
); echo "${a[@]}"'
check "trailing comment after elem"  'a=(
  p q  # trailing ) brace
  r
); echo "${a[@]}"'
check "multiple comment lines"       'a=(
  # first ) comment
  alpha
  # second (paren) comment
  beta
); echo "${a[@]}"'
check "midword hash literal"         'a=(x#y a#b); echo "${a[@]}"'
check "dollar-hash count"            'set -- a b c; a=($#); echo "${a[@]}"'
check "plain array no comment"       'a=(one two three); echo "${a[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
