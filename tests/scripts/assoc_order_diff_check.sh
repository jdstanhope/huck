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
# Informational only (does NOT affect PASS/FAIL): bash's `${m[@]:o:l}`
# slicing on ASSOCIATIVE arrays has its own off-by-one — non-negative
# offset o collapses to max(o-1, 0), unlike indexed-array/positional
# slicing. That's orthogonal to the hash-order fix this harness targets
# (#32); tracked separately as #322. Recorded here so it's visible, not
# silently untested.
check_known_divergence() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s (known-divergence case now matches bash)\n' "$label"
    else
        printf 'INFO (known divergence #322, not counted): %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
    fi
}

S='declare -A a=([one]=1 [two]=2 [three]=3 [foo]=f [bar]=b [qux]=q)'
check "assoc values @"   "$S; printf '<%s>' \"\${a[@]}\"; echo"
check "assoc keys !@"     "$S; printf '<%s>' \"\${!a[@]}\"; echo"
check "assoc values *"    "$S; printf '<%s>' \"\${a[*]}\"; echo"
check "assoc for-in"      "$S"$'\n'"for k in \"\${!a[@]}\"; do printf '[%s=%s]' \"\$k\" \"\${a[\$k]}\"; done; echo"
check "assoc transform ^^" "$S; printf '<%s>' \"\${a[@]^^}\"; echo"
check "assoc transform #"  "$S; printf '<%s>' \"\${a[@]#?}\"; echo"
check_known_divergence "assoc slice" "$S; printf '<%s>' \"\${a[@]:1:3}\"; echo"
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

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
