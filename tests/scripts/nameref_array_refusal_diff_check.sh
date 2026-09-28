#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #790: an EXISTING array cannot become
# a nameref, and the nameref checks run before the readonly refusal.
#
# bash validates a nameref declaration at two points, both AHEAD of the
# readonly-assignment refusal at `builtins/declare.def:845`:
#
#   :515  lexical checks on the word — self-reference, then an invalid target
#   :801  the existing variable's shape — `reference variable cannot be an array`
#
# so the order is self-ref -> invalid target -> existing array -> readonly ->
# destroy-array (#784) -> shape conversion (#697). huck had the array check on
# the value-LESS form only (#227), reported `readonly variable` ahead of all
# three nameref checks, and on an associative array leaked an INTERNAL message
# (`internal: install_scalar_value on associative array`) to the user.
#
# The shape is read as an ATTRIBUTE, matching bash's `array_p`, so a valueless
# `declare -a r` is refused as well as a populated one.
#
# NOT here, each its own divergence: what bash does when the SAME command
# creates the array (it silently drops the `-n` — #793); a self-reference
# inside a function (a warning, not an error — #792); and a shape flag applied
# to an existing nameref (#786).
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

# --- an existing array refuses, in every declaration form -------------------
check "=target on indexed"   'declare -a r=(1); declare -n r=t; echo rc=$?; declare -p r'
check "=target on assoc"     'declare -A r=([k]=v); declare -n r=t; echo rc=$?; declare -p r'
check "=target unset idx"    'declare -a r; declare -n r=t; echo rc=$?; declare -p r'
check "=target unset assoc"  'declare -A r; declare -n r=t; echo rc=$?; declare -p r'
check "no value on indexed"  'declare -a r=(1); declare -n r; echo rc=$?; declare -p r'
check "no value on assoc"    'declare -A r=([k]=v); declare -n r; echo rc=$?; declare -p r'
check "no value unset idx"   'declare -a r; declare -n r; echo rc=$?; declare -p r'
check "no value unset assoc" 'declare -A r; declare -n r; echo rc=$?; declare -p r'
check "-an on indexed"       'declare -a r=(1); declare -an r=t; echo rc=$?; declare -p r'
check "typeset -n on indexed" 'declare -a r=(1); typeset -n r=t; echo rc=$?; declare -p r'
# The refusal abandons only this name; the line carries on.
check "line carries on"      'declare -a r=(1); declare -n r=t; echo after'

# --- the order against readonly --------------------------------------------
check "self-ref beats readonly" 'readonly r=1; declare -n r=r; echo rc=$?'
check "bad target beats ro"     'readonly r=1; declare -n r="a b"; echo rc=$?'
check "array beats readonly"    'declare -a r=(1); readonly r; declare -n r=t; echo rc=$?'
check "readonly still fires"    'readonly r=1; declare -n r=t; echo rc=$?; declare -p r'
check "self-ref beats array"    'declare -a r=(1); declare -n r=r; echo rc=$?'
check "bad target beats array"  'declare -a r=(1); declare -n r="a b"; echo rc=$?'

# --- a scalar, or an absent name, still binds normally ----------------------
check "scalar binds"         'declare -n r=t; t=5; echo "[$r]"; declare -p r'
check "valid target checked" 'declare -n r="a b"; echo rc=$?'
check "self reference"       'declare -n r=r; echo rc=$?'
check "subscripted target"   'declare -a arr=(1 2); declare -n r="arr[1]"; echo "[$r]"'

# --- local: only an ALREADY-local array refuses ------------------------------
# A fresh local is a plain scalar (#539), so an outer array is not inherited
# and the bind succeeds.
check "local over outer idx"   'declare -a g=(1); f(){ local -n g=t; echo rc=$?; declare -p g; }; f'
check "local over outer assoc" 'declare -A gm=([k]=v); f(){ local -n gm=t; echo rc=$?; declare -p gm; }; f'
check "local -n on local idx"  'f(){ local -a r=(1); local -n r=t; echo rc=$?; declare -p r; }; f'
check "local -n on local assoc" 'f(){ local -A r; local -n r=t; echo rc=$?; declare -p r; }; f'
check "local -n bare on idx"   'f(){ local -a r=(1); local -n r; echo rc=$?; declare -p r; }; f'
check "local -n bare on assoc" 'f(){ local -A r; local -n r; echo rc=$?; declare -p r; }; f'
check "local -n unset idx"     'f(){ local -a r; local -n r=t; echo rc=$?; declare -p r; }; f'
check "local -n over scalar"   'f(){ local v=s; local -n v=t; echo rc=$?; declare -p v; }; f'
check "local bad target"       'f(){ local -n r="a b"; echo rc=$?; }; f'

# --- the target word is expanded EXACTLY once (#220) ------------------------
# A command substitution in the target must run one time, not twice — the
# refusal paths above return before the bind, so the expansion had to move.
#
# ⚠️ Each row TRUNCATES its own counter first. Both shells run in the same $tmp,
# so a counter that merely APPENDS is shared between them: the first draft of
# these rows had bash write 1 line and huck append a second, and reported a
# double expansion that did not exist.
check "one expansion"        ': >count; declare -n r=$(echo tgt >>count; echo tgt); tgt=7; echo "[$r]"; wc -l <count'
check "one expansion refused" ': >c2; declare -a r=(1); declare -n r=$(echo x >>c2; echo t); echo rc=$?; wc -l <c2'

harness_summary
