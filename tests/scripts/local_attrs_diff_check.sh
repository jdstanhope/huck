#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `local` attribute flags (-i / -r and
# their clusters). Each fragment runs through `bash -c` and `huck -c`;
# stdout+stderr+exit must match.
#
# SCOPE: huck's `local` now accepts the two attributes `declare` implements,
# `-i` (integer) and `-r` (readonly), applied with local scope. Bare/multiple/
# clustered forms and the -a/-A regress cases are all byte-comparable to bash.
#
# NOT byte-compared (excluded here):
#   - `-l`/`-u`/`-n`: huck prints a `not yet implemented` message (declare
#     doesn't implement them either); bash supports them — divergent by design.
#   - readonly REASSIGNMENT (`local -r x=1; x=2`): hits the known L-42/L-43
#     readonly-abort divergence — not byte-comparable.
#   - `local` outside a function: error-message PREFIX (`bash:` vs `huck:`)
#     diverges as for every builtin diagnostic.
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

# --- -i (integer attribute) ---
check "i: arith on ((++))"      'f(){ local -i c=41; (( c++ )); echo "$c"; }; f'
check "i: RHS arith-evaluated"  'f(){ local -i x=3+4; echo "$x"; }; f'
check "i: assign coerces arith" 'f(){ local -i x; x=2+3; echo "$x"; }; f'
check "i: multiple names"       'f(){ local -i a=1 b=2 c=3; echo "$a$b$c"; }; f'
check "i: does not leak global" 'f(){ local -i c=5; }; f; echo "out=${c-unset}"'
check "i: outer global intact"  'c=99; f(){ local -i c=5; echo "in=$c"; }; f; echo "after=$c"'

# --- -r (readonly attribute) ---
check "r: value set + readable"  'f(){ local -r ro=fixed; echo "$ro"; }; f'

# --- combined / clustered ---
check "ri: both attrs"           'f(){ local -ri n=5+5; echo "$n"; }; f'
check "ir: order-independent"    'f(){ local -ir n=5+5; echo "$n"; }; f'

# --- regress: no flags ---
check "plain: NAME=val pairs"    'f(){ local x=1 y=2; echo "$x$y"; }; f'

# --- regress: -a / -A still work ---
check "a: indexed array"         'f(){ local -a arr=(p q r); echo "${#arr[@]}"; }; f'
check "A: associative array"     'f(){ local -A m=([k]=v); echo "${m[k]}"; }; f'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
