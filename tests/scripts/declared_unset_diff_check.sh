#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #600/#225/#33/#691 (hub 2A of #765):
# a variable is attributes plus an OPTIONAL value. A valueless declaration
# records the attributes and leaves the value NULL; any assignment sets it;
# `unset` removes the entry outright.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$tmp" && bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [-c]" "$b" "$h"
    printf '%s\n' "$frag" >"$tmp/f.sh"
    b=$(cd "$tmp" && bash --norc --noprofile f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [file]" "$b" "$h"
}

# --- #600's table: a valueless declaration has no value ---------------------
check "declare -A"          'declare -A x; declare -p x'
check "declare -a"          'declare -a y; declare -p y'
check "declare -i"          'declare -i n; declare -p n'
check "declare -l/-u"       'declare -l lo; declare -p lo; declare -u up; declare -p up'
check "declare -n"          'declare -n r; declare -p r'
check "declare -r (#225)"   'declare -r r; declare -p r; readonly q; declare -p q'
check "declare bare"        'declare v; declare -p v'
check "@A transform"        'declare -a y; echo "${y[@]@A}"; declare -i n; echo "${n@A}"'
check "[[ -v ]]"            'declare -a y; [[ -v y ]] && echo v || echo nv; declare -A x; [[ -v x ]] && echo v || echo nv'
check "set -u scalar"       'set -u; declare -i n; echo "$n"'
check "set -u array reads"  'set -u; declare -a y; echo "[${y[@]}]"; echo "[${y[*]}]"; echo "${#y[@]}"'
check "export no value"     'declare -x E; env | grep -c "^E="; declare -p E; export -p | grep -c "^declare -x E$"'
check "local bare"          'f(){ local v; declare -p v; echo "[${v+set}${v-unset}]"; }; f'
check "local -a/-i"         'f(){ local -a a; declare -p a; local -i i; declare -p i; }; f'

# --- becoming set -----------------------------------------------------------
check "scalar assign"       'declare v; echo "[${v+set}]"; v=; declare -p v; echo "[${v+set}]"'
check "indexed element"     'declare -a y; y[2]=x; declare -p y; [[ -v y ]] && echo v || echo nv'
check "indexed append"      'declare -a y; y+=(a); declare -p y'
check "assoc element"       'declare -A x; x[k]=v; declare -p x; [[ -v x ]] && echo v || echo nv; [[ -v x[k] ]] && echo vk || echo nvk'
check "integer append"      'declare -i n; n+=1; declare -p n'
check "export then assign"  'declare -x E; env | grep -c "^E="; E=1; env | grep "^E="; declare -p E'
check "case fold on assign" 'declare -l lo; lo=ABC; declare -p lo'
check "nameref on assign"   'declare -n r; r=x; declare -p r'

# --- still unset ------------------------------------------------------------
check "attribute churn"     'declare -i n; declare -p n; declare +i n; declare -p n; declare -x n; declare -p n'
check "shape survives"      'declare -a y; declare -A y; echo rc=$?; declare -p y'
# fix round 1 (#600 regression): on readonly/export with NO value, -a/-A is
# a SELECTOR — it must never change the shape of an existing variable, set
# or unset (bash: `declare -A x; readonly -a x` -> `declare -Ar x`, shape
# unchanged). The `readonly -A` row on an existing INDEXED array is left out:
# it hits #698 (unrelated, out of scope) on unpatched shape-conversion
# refusal — this row happens to pass here because the SAME fix that
# resolves the selector regression also stops readonly -a/-A from ever
# attempting a shape conversion in the no-value path, so add it too.
check "readonly -a selector" 'declare -A x; readonly -a x; declare -p x'
check "readonly -A selector" 'declare -a y; readonly -A y; declare -p y; echo rc=$?'
check "export -a selector"   'declare -A x; export -a x; declare -p x'
check "readonly guard"      'declare -r r; r=1; echo rc=$?; declare -p r'
check "unset removes"       'declare -a y; unset y; declare -p y; echo rc=$?'
check "listings skip it"    'declare -a y; compgen -v y; echo ---; set | grep -c "^y="; declare -p | grep -c "^declare -a y$"'
check "empty-ish reads"     'declare -A x; echo "[${x[@]:-D}] [${x-D}] [${#x}]"; declare -a y; echo "[${!y[@]}] [${y[@]:0}]"'

# --- #691: a bare local inherits ONLY the export attribute ------------------
check "local over exported" 'declare -x V=1; f(){ local V; declare -p V; V=9; declare -p V; }; f; declare -p V'
check "local over plain"    'V=1; f(){ local V; declare -p V; }; f'
check "local over integer"  'declare -i N=1; f(){ local N; declare -p N; N=2+2; declare -p N; }; f'

# --- #692: the export set answers TWO different questions -------------------
# `export -p` reports the innermost VISIBLE binding if exported; a CHILD's
# environment takes the innermost binding that is exported AND has a value,
# walking past an unexported (or valueless) local to the outer binding.
check "unexported local"    'declare -x V=1; f(){ local +x V=2; env | grep "^V="; echo "[$V]"; declare -p V; }; f; env | grep "^V="'
check "export -p vs child"  'declare -x V=1; f(){ local +x V=2; export -p | grep -c "^declare -x V="; env | grep -c "^V=1$"; }; f'
check "bare local, valueless" 'declare -x V=1; f(){ local V; env | grep "^V="; declare -p V; }; f'
check "assign to unexported" 'declare -x V=1; f(){ local +x V=2; V=3; env | grep "^V="; declare -p V; }; f'
check "nested local"        'declare -x V=1; f(){ local +x V=2; g(){ local V=3; env | grep "^V="; declare -p V; }; g; }; f'
check "export inside"       'declare -x V=1; f(){ local +x V=2; export V; env | grep "^V="; }; f'
check "unexported outer"    'V=1; f(){ local +x V=2; env | grep "^V=" || echo none; }; f'
check "sibling unaffected"  'declare -x V=1 W=9; f(){ local +x V=2; env | grep -E "^(V|W)=" | sort; }; f'
check "local assign exports" 'declare -x A=1; f(){ local A; A=2; env | grep "^A="; declare -p A; }; f'

# --- #777: `local -x` -------------------------------------------------------
check "local -x"            'f(){ local -x L=5; declare -p L; env | grep "^L="; }; f; env | grep -c "^L="'
check "local -x over outer" 'declare -x V=1; f(){ local -x V=2; env | grep "^V="; declare -p V; }; f; env | grep "^V="'
check "local -x no value"   'declare -x V=1; f(){ local -x V; env | grep "^V="; declare -p V; }; f'

# --- #698 (remaining cell): export -A is a selector too, same as export -a --
check "export -A selector"     'declare -a i=(1); export -A i; echo rc=$?; declare -p i'
check "export -a selector"     'declare -A m=([k]=v); export -a m; echo rc=$?; declare -p m'
check "readonly -A selector"   'declare -a i=(1); readonly -A i; echo rc=$?; declare -p i'

# --- #777: exported_env must STOP at an exported ARRAY, not walk past it ---
check "exported array shadow (indexed)"     'export V=1; f(){ local -a V=(x); env | grep "^V=" || echo noenv; }; f'
check "exported array shadow (assoc)"       'export V=1; f(){ local -A V=([k]=q); env | grep "^V=" || echo noenv; }; f'
check "exported array shadow valueless"     'export V=1; f(){ local -a V; env | grep "^V=" || echo noenv; }; f'
check "exported array shadow at snapshot"   'export V=1; f(){ local -a V=(x); g; }; g(){ local +x V=2; env | grep "^V=" || echo noenv; }; f'

# --- #777: `export -A NAME=(...)` must build an ASSOCIATIVE array, not an
# indexed one that silently drops every key but the last -----------------
check "export -A with value"        'export -A x=([k]=v [j]=w); declare -p x'
check "export -a with value"        'export -a x=(1 2); declare -p x'
check "export -A value then key"    'export -A x=([k]=v); x[j]=w; declare -p x'

harness_summary
