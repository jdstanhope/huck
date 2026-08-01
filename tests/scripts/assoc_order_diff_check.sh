#!/usr/bin/env bash
# Byte-identical bash<->huck harness for L-44 (#32): associative-array
# ITERATION ORDER. bash walks its internal hash-table bucket order, not
# insertion order; `${m[@]}`/`${!m[@]}`/`${m[@]<op>}`/`${m[@]:o:l}` must
# enumerate elements in that same bash hash order.
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
S='declare -A a=([one]=1 [two]=2 [three]=3 [foo]=f [bar]=b [qux]=q)'
check "assoc values @"   "$S; printf '<%s>' \"\${a[@]}\"; echo"
check "assoc keys !@"     "$S; printf '<%s>' \"\${!a[@]}\"; echo"
check "assoc values *"    "$S; printf '<%s>' \"\${a[*]}\"; echo"
check "assoc for-in"      "$S"$'\n'"for k in \"\${!a[@]}\"; do printf '[%s=%s]' \"\$k\" \"\${a[\$k]}\"; done; echo"
check "assoc transform ^^" "$S; printf '<%s>' \"\${a[@]^^}\"; echo"
check "assoc transform #"  "$S; printf '<%s>' \"\${a[@]#?}\"; echo"
# #322 (FIXED): assoc `${m[@]:o:l}` slicing has a bash-only off-by-one —
# a non-negative offset o collapses to max(o-1, 0) (so o=0 and o=1 both
# start at element 0), unlike indexed-array/positional slicing. Negative
# offsets are unaffected.
check "assoc slice 1:3"   "$S; printf '<%s>' \"\${a[@]:1:3}\"; echo"
check "assoc slice 0:3"   "$S; printf '<%s>' \"\${a[@]:0:3}\"; echo"
check "assoc slice 2:2"   "$S; printf '<%s>' \"\${a[@]:2:2}\"; echo"
check "assoc slice 3:2"   "$S; printf '<%s>' \"\${a[@]:3:2}\"; echo"
check "assoc slice 2"     "$S; printf '<%s>' \"\${a[@]:2}\"; echo"
check "assoc slice 6:1"   "$S; printf '<%s>' \"\${a[@]:6:1}\"; echo"
check "assoc slice neg2"  "$S; printf '<%s>' \"\${a[@]: -2}\"; echo"
check "assoc slice neg21" "$S; printf '<%s>' \"\${a[@]: -2:1}\"; echo"
check "assoc slice star"  "$S; printf '<%s>' \"\${a[*]:2:2}\"; echo"
# collision + update + unset
C='declare -A a; a[dup0]=1; a[dup1]=2; a[x]=3; a[dup0]=9; unset "a[x]"'
check "assoc upd/unset !@" "$C; printf '<%s>' \"\${!a[@]}\"; echo"

# Task 3: render/transform sites — declare -p / bare declare / @A / @K / @k.
check "assoc declare -p"  "$S; declare -p a"
check "assoc bare declare" "$S; declare -A | grep '^declare -A a='"
# Note: @A whole-array uses `echo`, not `printf '%s\n'` — matches the
# array_transforms_diff_check.sh precedent. bash's `[@]`+`@A` combo has an
# orthogonal, pre-existing field-splitting quirk (the declare string is
# split into words at top-level spaces even though quoted) that `printf`
# would expose; `echo` re-joins with single spaces and hides it, isolating
# this case to what this task targets: iteration order.
check "assoc @A"           "$S; echo \"\${a[@]@A}\""
check "assoc @K"           "$S; printf '%s\n' \"\${a[@]@K}\""
check "assoc @k"           "$S; printf '<%s>' \"\${a[@]@k}\"; echo"


# Task 4: `declare -c` (capitalize-first attribute, L-44's sibling casemod
# blocker). bash uppercases the first char AND lowercases the rest of the
# value on assignment -- NOT the same transform as `${v@u}` (which leaves
# the untouched chars' case alone). Verified on bash 5.2.21.
check "declare -c basic"     'declare -c x="hello world"; echo "$x"'
check "declare -c mixed rest" 'declare -c x="foo BAR"; echo "$x"'
check "declare -c reassign"  'declare -c x; x="foo BAR"; echo "$x"; x="BAZ qux"; echo "$x"'
check "declare -c decl-p"    'declare -c x="hello"; declare -p x'
check "declare -lc last-wins (both cancel)" 'declare -l -c x="HELLO"; echo "$x"; declare -p x'
check "declare -uc both cancel"  'declare -u -c v="hello"; declare -p v'
check "declare -u then -c (sequential, last wins)" 'declare -u v="hello"; declare -c v; declare -p v; echo "$v"'
check "declare +c clears"   'declare -c v="hello"; declare +c v; v="second try"; echo "$v"; declare -p v'
check "declare -c +c same-invocation cancel" 'declare -c +c v="hello"; declare -p v; echo "$v"'
check "declare -c empty value" 'declare -c x=""; echo "$x"; declare -p x'
check "local -c"            'f(){ local -c x="hello world"; echo "$x"; }; f'

# casemod's actual remaining blocker after -c (found via re-measurement
# against the real bash-test-suite `casemod` category, not part of the
# original brief): `${v^pattern}`/`${v,pattern}` (all=false, singular form)
# tested EVERY char for the first match, but bash only tests the STRING'S
# FIRST char against pattern -- no forward scan. Fixed in
# param_expansion.rs::case_modify. Verified on bash 5.2.21.
check "casemod ^pattern no match at index 0 (no scan)" 'S=hello; echo "${S^[aeiou]}"'
check "casemod ^pattern match at index 0"              'S=ello; echo "${S^[aeiou]}"'
check "casemod ,pattern no match at index 0 (no scan)" 'S=HELLO; echo "${S,[AEIOU]}"'
check "casemod @ positional ^pattern no scan"          'set -- hello ello; echo "${@^[aeiou]}"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
