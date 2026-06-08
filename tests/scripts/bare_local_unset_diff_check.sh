#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v115: a bare `local NAME` (no value)
# declares an UNSET local (M-111). `local NAME=`/`=val`/`-a`/`-A` unchanged; a
# bare re-`local` of an already-local name preserves its value.
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

check "bare local -v unset"   'f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }; f'
check "bare local -default"   'f(){ local x; echo "[${x-DEF}]"; }; f'
check "bare local +alt"       'f(){ local x; echo "[${x+ALT}]"; }; f'
check "explicit empty is set" 'f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }; f'
check "local with value"      'f(){ local x=v; echo "$x"; }; f'
check "assign-then-local"     'x=outer; f(){ local x; x=5; echo "in=$x"; }; f; echo "out=$x"'
check "shadow outer as unset" 'x=outer; f(){ local x; echo "[${x-DEF}]"; }; f'
check "colon-default matches" 'f(){ local x; echo "[${x:-d}]"; }; f'
check "multiple bare locals"  'f(){ local a b; [[ -v a ]] && echo aS || echo aU; [[ -v b ]] && echo bS || echo bU; }; f'
check "re-local preserves"    'f(){ local x=v; local x; [[ -v x ]] && echo "SET=[$x]" || echo UNSET; }; f'
check "upvars v-gate shape"   'f(){ local up=() vcur vcword; vcur=cur; [[ -v vcur ]] && up+=("$vcur"); [[ -v vcword ]] && up+=("$vcword"); echo "n=${#up[@]}"; }; f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
