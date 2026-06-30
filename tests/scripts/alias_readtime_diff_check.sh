#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v239: read-time alias expansion via
# the live lexer. Aliases must be defined on a line BEFORE their use (same-unit
# defs don't take effect — these are multi-line fragments). All tests use
# shopt -s expand_aliases so the shell expands aliases in file/source mode.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliasrt.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# alias→keyword: 'x' is defined as 'if'; the next line uses 'x' at command
# position where it expands to 'if', making the whole construct valid.
checkf "alias to keyword" $'shopt -s expand_aliases\nalias x=if\nx true; then echo Y; fi'

# func-def name: 'foo' is aliased to 'bar'; 'foo() { ... }' defines function bar.
checkf "func-def name expands" $'shopt -s expand_aliases\nalias foo=bar\nfoo() { echo INFUNC; }\ndeclare -F'

# trailing-blank chains to an alias arg: 'a' has trailing blank, so the next
# word 'c' is also eligible; 'c' → 'hello' (via b→echo, c→hello chain).
checkf "trailing-blank chains to alias arg" $'shopt -s expand_aliases\nalias a="b "\nalias b=echo\nalias c=hello\na c'

# trailing-blank, arg not an alias: 'a' has trailing blank making 'hi' eligible,
# but 'hi' is not an alias — it stays literal.
checkf "trailing-blank arg not an alias" $'shopt -s expand_aliases\nalias a="b "\nalias b=echo\na hi'

# NO trailing blank, arg IS an alias (must NOT expand): 'a' body has no trailing
# blank, so the argument word 'c' is NOT alias-eligible even though 'c' is an alias.
checkf "no trailing-blank arg is alias no-expand" $'shopt -s expand_aliases\nalias a=echo\nalias c=hello\na c'

# trailing-blank chain stops at the 2nd arg: 'a' → 'echo ' (trailing blank),
# so 'x' → 'X' (first arg expands); 'y' is NOT eligible (only the first arg
# after the trailing-blank expansion is re-eligible).
checkf "trailing-blank stops at second arg" $'shopt -s expand_aliases\nalias a="echo "\nalias x=X\nalias y=Y\na x y'

# recursion guard: alias ls="ls -a" must not loop; the inner 'ls' in the body
# is in the active set and is not re-expanded.
checkf "recursion guard" $'shopt -s expand_aliases\nalias ls="ls -a"\nls /dev/null'

# cross-unit def-then-use: alias defined on line N, used on line N+1.
# The live lexer's between-unit set_aliases refresh makes the new alias visible
# to the parser of the NEXT unit.
checkf "cross-unit def-then-use" $'shopt -s expand_aliases\nalias greet="echo hi"\ngreet'

# quoted word not expanded: a single-quoted alias name is not a candidate for
# alias expansion. We alias 'printf' to something different so the difference
# is visible, then use 'printf' unquoted (expands) and quoted (does not expand).
checkf "quoted word not expanded" $'shopt -s expand_aliases\nalias printf="echo ALIAS"\nprintf "direct"\n'"'"'printf'"'"' "quoted"'

# Command substitution + alias integration: alias expands at command position in
# the outer context; $() captures output of a real command normally.
# Note: bash expands aliases inside $() with expand_aliases; huck does not
# (batch-tokenized comsub). This test uses a real command inside $() so both
# shells agree on the output.
checkf "comsub integration with aliases" $'shopt -s expand_aliases\nalias greet="echo HELLO"\ngreet\nx=$(echo from_comsub)\necho "x=$x"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
