#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `readonly`-attribute roots fixed in
# v349 (#343, `attr` category):
#   Root A: `readonly -a` indexed-array attribute.
#   Root C: the readonly-variable error is bare (`x: readonly variable`) for a
#           plain re-assignment, but keeps the `readonly:` prefix for an
#           attribute-change attempt (`-a`/`-A` with a non-array-literal RHS).
#   Root D: a QUOTED `name=value` arg reaching readonly/declare/export/local is
#           an assignment (bash re-checks the expanded word for `name=value`).
#   Root B: under `-a`/`-A`, a quoted `(...)` scalar value is coerced to an
#           array literal; without `-a` it stays a literal scalar.
# Each fragment runs through `bash -c` and `huck -c`; stdout+stderr+exit must
# match byte-for-byte.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Normalize the leading program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — that prefix is the invoking binary's argv[0], a
# non-behavioral artifact of piping the fragment on stdin, not a huck<->bash
# difference. Everything after `: line N:` must still match byte-for-byte.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- Root A: readonly -a indexed-array attribute ---
check "readonly -a compound"      'readonly -a x=(1 2); declare -p x'
check "readonly -a single"        'readonly -a r=(7); declare -p r'
check "readonly -A still works"   'readonly -A m=([k]=v); declare -p m'

# --- Root C: readonly-variable error prefix (bare vs attribute-change) ---
check "reassign readonly bare"    'readonly x=1; readonly x=2'
check "already-ro bare"           'x=1; readonly x; x=5'
check "attr-change -a keeps pfx"  'readonly r=(1); readonly -a r=2'
check "-a array-literal is bare"  'readonly r=(1); readonly -a r=(2)'

# --- Root D: quoted `name=value` arg is an assignment ---
check "readonly quoted scalar"    "readonly 'x=hi'; declare -p x"
check "declare quoted scalar"     "declare 'x=hi'; declare -p x"
check "export quoted scalar"      "export 'x=hi'; declare -p x"
check "local quoted scalar"       "f(){ local 'x=hi'; declare -p x; }; f"
check "readonly quoted preexist"  'c=(outside); readonly '"'"'c=(3)'"'"'; declare -p c'
# KEEP: invalid identifier stays rejected.
check "quoted invalid ident"      "readonly '3x=1'"

# --- Root B: `-a`/`-A` coerces a quoted `(...)` scalar to an array ---
check "readonly -a quoted paren"  "d=(outside); readonly -a 'd=(4)'; declare -p d"
check "readonly -a r='(7)'"       "readonly -a r='(7)'; declare -p r"
check "export -a quoted paren"    "export -a r='(7)'; declare -p r"
check "declare -a quoted paren"   "declare -a r='(7)'; declare -p r"
# KEEP: without -a, a quoted `(...)` stays a literal scalar.
check "no -a keeps literal paren" "declare p='(3)'; declare -p p"
check "export no -a literal"      "export r='(5)'; declare -p r"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
