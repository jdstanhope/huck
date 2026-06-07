#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v105: `[[ … =~ … ]]` regex operands.
# Each fragment exercises the regex right-hand operand of `=~` (unquoted ERE
# with parens/alternation/bracket-exprs/anchors, a $var-supplied regex, and a
# `( )` grouped test). check feeds each fragment as a whole script to both
# shells and compares combined stdout+stderr+EXIT to assert byte-identity.
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

# 1: parenthesised group with a space matches a quoted operand containing a space
check "group with space match" '[[ "a b" =~ (a b) ]] && echo yes || echo no'

# 2: same group does NOT match when the space is absent
check "group with space no-match" '[[ ab =~ (a b) ]] && echo yes || echo no'

# 3: nested groups + escaped brackets + alternation + trailing dot
check "nested groups escaped brackets" '[[ "[no-]x" =~ (\[((no|dont)-?)\]). ]] && echo yes || echo no'

# 4: anchored ERE with character classes and capture groups
check "anchored char-classes" 'c=foo=bar; [[ $c =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] && echo yes || echo no'

# 5: leading dash + negated bracket expression
check "negated bracket expr" '[[ "-abc" =~ (-[^]]+) ]] && echo yes || echo no'

# 6: escaped metachars + alternation across the whole pattern
check "escaped meta alternation" '[[ /etc =~ ^\~.*|^\/.* ]] && echo yes || echo no'

# 7: regex supplied via a variable (unquoted $var stays an ERE)
check "regex from variable" "re='(a|b)'; [[ a =~ \$re ]] && echo yes || echo no"

# 8: grouped `( )` test combinator (no =~, guards the parser path)
check "grouped test combinator" '[[ -n a && ( -z "" || -n b ) ]] && echo yes || echo no'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
