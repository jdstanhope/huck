#!/usr/bin/env bash
# Byte-identical bash<->huck harness for command-position brace expansion.
# (The array-literal path is covered by array_brace_expansion_diff_check.sh;
# this exercises braces in ordinary command words.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# basic comma lists
check "simple list"     'echo {a,b,c}'
check "prefix"          'echo x{a,b}y'
check "affix words"     'echo pre{1,2}post'
check "trailing"        'echo a{1,2,3}z'
# nesting and cross product
check "nested"          'echo {a,{b,c}}'
check "cross product"   'echo {a,b}{1,2}'
# var-adjacent brace expansion
# DIVERGENCE (reported): `x=p; echo $x{a,b}` — bash does brace expansion BEFORE
# variable expansion, so $x{a,b} -> $xa $xb -> (unset) -> empty line. huck expands
# the variable first, yielding `pa pb`. Excluded until the ordering is fixed.
# quoting must NOT expand
check "dquote literal"  'echo "{a,b}"'
check "squote literal"  "echo '{a,b}'"
check "bslash literal"  'echo \{a,b\}'
# sentinel: brace precedes var expansion, so a var VALUE with braces is inert
check "value not reexpanded" 'x='"'"'{a,b}'"'"'; echo $x'

# v341 (#318) Root 4: negative step (sign ignored; direction from endpoints).
check "neg step int desc"  'echo {10..1..-2}'
check "neg step int asc"   'echo {-1..-10..-2}'
check "neg step big"       'echo {100..0..-5}'
check "neg step char"      'echo {z..a..-2}'
check "pos step desc"      'echo {10..1..2}'
# v341 (#318) Root 2: backslash char range → empty element.
check "backslash range Aa" 'echo {A..a}'
check "backslash range Za" 'echo {Z..a}'
# v341 (#318) fix-round-1: i64::MIN step must not panic (checked_abs guard);
# bash also leaves this literal (step magnitude has no i64 representation).
check "step i64::MIN int"  'echo {1..2..-9223372036854775808}'
check "step i64::MIN char" 'echo {a..z..-9223372036854775808}'

# v341 (#318) Root 3: nested non-comma brace — inner still expands.
check "nested non-comma"   'echo a-{b{d,e}}-c'
check "nested deeper"      'echo a-{b{c{d,e}}}-f'
check "nested plain body"  'echo x-{foo}-y'
check "brace spaces body"  'echo {a b}'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
