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
# v341 (#44) Root 1: bash brace-expands textually BEFORE variable expansion, so
# `x=p; echo $x{a,b}` -> $xa $xb -> (both unset) -> empty line.
check "var-adjacent unset"  'x=p; echo $x{a,b}'
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

# v341 (#44) Root 1: bare $var{x,y} merges the brace suffix into the name;
# braced ${var} does NOT (structurally distinct: Var vs ParamExpansion).
check "bare name merge"    'var=baz; varx=vx; vary=vy; echo $var{x,y}'
check "braced no merge"    'var=baz; varx=vx; vary=vy; echo ${var}{x,y}'
check "quoted braced"      'var=baz; varx=vx; vary=vy; echo "${var}"{x,y}'
check "merge non-namechar" 'var=baz; echo $var{-,+}'
check "merge digits"       'v1=one; v2=two; var=baz; echo $var{1,2}'
# v341 (#44, final review) Root 1 fix-round-2: positional/special params are
# NOT identifiers — bash's greedy $name read never absorbs the brace suffix
# for $1/$$/$#, unlike a real bare $name.
check "no merge positional 1" 'set -- foo; echo $1{a,b}'
check "no merge positional 2" 'set -- a b c d e f g h i j k; echo $1{0,1}'
check "identifier merge"      'foo=baz; fooa=vx; foob=vy; echo $foo{a,b}'

# v341 (#318) Root 5: unmatched outer `{` is literal but a LATER balanced
# brace still expands.
check "unmatched then balanced" 'echo a-{bdef-{g,i}-c'
check "unmatched wraps balanced" 'echo {a{b,c}'
check "double unmatched"   'echo {{a,b}'
check "unmatched no inner"  'echo x{y,z'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
