#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v106 (M-101): extglob patterns
# (`!(…)`/`@(…)`/…) inside command substitutions `$(…)` / `` `…` `` and array
# literals, plus the two bundled lexer fixes (a `=~` regex operand on a `\`-newline
# continuation, and a bare `{` in a `${var%%pat}` operand). `shopt -s extglob`
# MUST be on a PRIOR line — same-line shopt is not active at parse time in either
# shell. Globs are kept single-match (`!(skip)` excludes the only other file) so
# the result is one word, independent of array word-splitting.
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

# 1: extglob !(…) inside $() (extglob enabled on a prior line)
check "extglob in \$()" $'d=/tmp/hkv106_1; rm -rf "$d"; mkdir -p "$d"; cd "$d"; : > keep; : > skip\nshopt -s extglob\necho $(printf \'%s\\n\' !(skip))\ncd /; rm -rf "$d"'

# 2: extglob !(…) inside backticks
check "extglob in backticks" $'d=/tmp/hkv106_2; rm -rf "$d"; mkdir -p "$d"; cd "$d"; : > keep; : > skip\nshopt -s extglob\necho `printf \'%s\\n\' !(skip)`\ncd /; rm -rf "$d"'

# 3: extglob !(…) inside a $() that is an array-literal element
check "extglob in array \$()" $'d=/tmp/hkv106_3; rm -rf "$d"; mkdir -p "$d"; cd "$d"; : > keep; : > skip\nshopt -s extglob\na=($(printf \'%s\\n\' !(skip))); echo "${a[0]}"\ncd /; rm -rf "$d"'

# 4: [[ =~ ]] regex operand on a backslash-newline continuation line
check "=~ operand continuation" $'[[ abc =~ \\\n  (a|x)bc ]] && echo M || echo N'

# 5: bare `{` inside a ${var%%pattern} operand (must not nest the ${...})
check "bare brace in \${} operand" "x='abc{def'; echo \${x%%[<{(]*}"

# 6: plain command sub (extglob off) — control, byte-unchanged
check "plain command sub" 'echo $(echo plain)'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
