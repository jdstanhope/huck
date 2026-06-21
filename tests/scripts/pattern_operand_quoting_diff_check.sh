#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v201 (L-54a): a QUOTED glob metacharacter
# in a `${x#pat}` / `${x%pat}` / `${x/pat/repl}` PATTERN matches LITERALLY (the
# quote escapes `*`/`?`/`[`), while an unquoted one stays an active glob. This is
# the `${...}`-pattern analogue of `case`/`[[ == ]]` quoting (which already work)
# and of v199's `=~` regex fix.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# --- remove-prefix # with quoted metachars (literal) ---
check "rm# quoted * no-match" 'x=axb; echo "${x#'\''a*'\''}"'   # literal a* -> no match -> axb
check "rm# quoted * match"    'x="a*b"; echo "${x#'\''a*'\''}"' # literal a* matches -> b
check "rm# quoted ? no-match" 'x=axc; echo "${x#'\''a?'\''}"'   # literal a? -> no match -> axc
check "rm# unquoted * active" 'x=axb; echo "${x#a*}"'          # active -> match -> ""(empty)
# --- remove-suffix % ---
check "rm% quoted * no-match" 'x=bxa; echo "${x%'\''*a'\''}"'   # literal *a -> no match
check "rm% quoted * match"    'x="b*a"; echo "${x%'\''*a'\''}"' # literal *a matches -> b
# --- substitute / and // ---
check "sub/ quoted * "        'x=star; echo "${x/'\''*'\''/_}"'        # literal * -> no match -> star
check "sub/ quoted ? "        'x=ab; echo "${x/'\''?'\''/_}"'          # literal ? -> no match -> ab
check "sub// quoted [ "       'x="a[b]c"; echo "${x//'\''[b]'\''/_}"' # literal [b] -> a_c
check "sub/ unquoted * active" 'x=star; echo "${x/*/_}"'            # active -> _
check "sub/ quoted via var"   'p="*"; x=star; echo "${x/"$p"/_}"'   # quoted var -> literal
# --- no-metachar controls (must be unchanged) ---
check "rm# plain"             'x=aXb; echo "${x#a}"'
check "sub/ plain"            'x=ab; echo "${x/a/Z}"'
check "sub// plain literal"   'x=aXbXc; echo "${x//'\''X'\''/_}"'
# --- case-modification ${x^^pat} / ${x,,pat} patterns ---
check "case^^ quoted * lit"  'x=axb; echo "${x^^'\''a*'\''}"'   # literal a* -> no match -> axb
check "case^^ unquoted * "   'x=axb; echo "${x^^a*}"'          # active -> AXB
check "case^^ quoted [ lit"  'x=cat; echo "${x^^'\''[ac]'\''}"' # literal [ac] -> no match -> cat
check "case^^ unquoted [ "   'x=cat; echo "${x^^[ac]}"'        # active -> CAt
# --- extglob: unquoted active, quoted literal ---
check "extglob unquoted"      'shopt -s extglob; x=abc; echo "${x/@(a|z)/_}"'   # active -> _bc
check "extglob quoted literal" 'shopt -s extglob; x="@(a)bc"; echo "${x/'\''@(a)'\''/_}"' # literal -> _bc
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
