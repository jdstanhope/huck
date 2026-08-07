#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v190: bare `declare`/`typeset` (no args)
# variable listing format. Each case sets z* vars and greps `declare` to ^z to
# filter out the inherited environment. `declare -p` is included as a guard.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# scalar quoting battery (bare declare)
check "scalar bare"   'zq=hello; declare 2>/dev/null | grep "^zq="'
check "scalar empty"  'zq=; declare 2>/dev/null | grep "^zq="'
check "scalar space"  'zq="a b"; declare 2>/dev/null | grep "^zq="'
check "scalar semi"   'zq="x;y"; declare 2>/dev/null | grep "^zq="'
check "scalar glob"   'zq="gl*ob"; declare 2>/dev/null | grep "^zq="'
check "scalar dollar" 'zq="d\$ollar"; declare 2>/dev/null | grep "^zq="'
check "scalar bang"   'zq="bang!x"; declare 2>/dev/null | grep "^zq="'
check "scalar angle"  'zq="lt<gt>"; declare 2>/dev/null | grep "^zq="'
check "scalar quote"  "zq=\"qu'ote\"; declare 2>/dev/null | grep '^zq='"
check "scalar tilde"  'zq="ti~lde"; declare 2>/dev/null | grep "^zq="'
check "scalar eq"     'zq="eq=ual"; declare 2>/dev/null | grep "^zq="'
check "scalar tab"    'zq=$'"'"'ta\tb'"'"'; declare 2>/dev/null | grep "^zq="'
# integer / exported / readonly (bare: no attribute flag)
check "integer"       'declare -i zi=42; declare 2>/dev/null | grep "^zi="'
check "exported"      'export ze=world; declare 2>/dev/null | grep "^ze="'
check "readonly"      'readonly zr=const; declare 2>/dev/null | grep "^zr="'
# indexed array
check "array"         'za=(p "q r" ""); declare 2>/dev/null | grep "^za="'
# typeset parity
check "typeset"       'zt=hi; typeset 2>/dev/null | grep "^zt="'
# function listing (structural: huck normalizes bodies (M-121), so just confirm
# the function is listed at all — count is byte-identical "1")
check "lists fn"      'zf(){ echo hi; }; declare 2>/dev/null | grep -c "^zf"'
# REGRESSION GUARD: declare -p must stay byte-identical (the -p path is unchanged)
check "declare -p"    'zq="a b"; zi=42; declare -i zi; za=(p "q r"); declare -p zq zi za 2>/dev/null'

harness_summary
