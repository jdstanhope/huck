#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v119: POSIX bracket character classes
# in glob patterns (M-54). Spread across the 12 classes + negation + mixed +
# pathname. File-arg execution (L-27: huck history-expands piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "subst digit"   's="a1 b2"; echo "${s//[[:digit:]]/_}"'
check "subst alpha"   's="a1 b2"; echo "${s//[[:alpha:]]/_}"'
check "subst alnum"   's="a1_b2"; echo "${s//[[:alnum:]]/X}"'
check "subst space"   's=$'\''a b\tc'\''; echo "${s//[[:space:]]/_}"'
check "subst upper"   's="aXbY"; echo "${s//[[:upper:]]/_}"'
check "subst lower"   's="aXbY"; echo "${s//[[:lower:]]/_}"'
check "subst punct"   's="a.b!c]"; echo "${s//[[:punct:]]/_}"'
check "subst xdigit"  's="9fg"; echo "${s//[[:xdigit:]]/_}"'
check "subst blank"   's=$'\''a b\tc'\''; echo "${s//[[:blank:]]/_}"'
check "subst cntrl"   's=$'\''a\tb'\''; echo "${s//[[:cntrl:]]/_}"'
check "subst print"   's="ab cd"; echo "${s//[[:print:]]/_}"'
check "subst graph"   's="a b c"; echo "${s//[[:graph:]]/_}"'
check "case space"    'case " " in [[:space:]]) echo SP;; *) echo no;; esac'
check "case digit no" 'case "x" in [[:digit:]]) echo D;; *) echo no;; esac'
check "dbracket alpha" '[[ "x" == [[:alpha:]] ]] && echo Y || echo N'
check "dbracket neg"   '[[ "x" == [^[:digit:]] ]] && echo Y || echo N'
check "mixed class"    's="a5_b"; echo "${s//[[:digit:]_]/X}"'
check "extglob off"    'shopt -u extglob; case "5" in [[:digit:]]) echo D;; *) echo no;; esac'
check "pathname upper" 'd=$(mktemp -d); touch "$d"/Af "$d"/bf "$d"/Cf; cd "$d"; echo [[:upper:]]*; rm -rf "$d"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
