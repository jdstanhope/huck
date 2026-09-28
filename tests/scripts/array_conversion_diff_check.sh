#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #347/#697/#734 (hub 2B of #765):
# ONE conversion table for `declare -a`/`-A`/`local -a`/`-A` on a name that
# already exists. bash's rules, all measured against 5.2.21:
#
#   * a materialised SCALAR is PROMOTED to element 0 (`x=v; declare -a x` ->
#     `declare -a x=([0]="v")`; `declare -A x` -> `([0]="v")` with key "0");
#   * indexed <-> associative is REFUSED in both directions, with the data
#     left untouched — it is never silently discarded;
#   * an UNSET declaration carries its shape, so the refusal applies to it
#     too (`declare -a y; declare -A y`);
#   * readonly does NOT block a reshape — readonly guards the VALUE, not the
#     shape (`readonly R=1; declare -a R` -> `declare -ar R=([0]="1")`), and
#     a refusal's message wins over the readonly message;
#   * inside a function, a shape change on a name ALREADY local in this frame
#     DISCARDS the value (`local v=s; local -a v` -> `declare -a v=()`), while
#     a fresh local over an outer scalar starts from nothing and a TOP-LEVEL
#     reshape promotes.
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

# --- promotion: a materialised scalar becomes element 0 (#697) ---------------
check "scalar -> indexed"      'x=v; declare -a x; declare -p x'
check "scalar -> assoc"        'x=v; declare -A x; declare -p x'
check "scalar -> indexed -i"   'x=v; declare -ai x; declare -p x'
check "scalar -> assoc -i"     'x=v; declare -Ai x; declare -p x'
check "declared scalar -> idx" 'declare x; declare -a x; declare -p x'
check "integer -> indexed"     'declare -i n=5; declare -a n; declare -p n'

# --- refusal in both directions, data untouched (#347) -----------------------
check "unset idx -> assoc"     'declare -a x; declare -A x; declare -p x; echo rc=$?'
check "unset assoc -> idx"     'declare -A x; declare -a x; declare -p x'
check "assoc -> idx keeps"     'declare -A x=([k]=1); declare -a x; echo rc=$?; declare -p x'
check "idx -> assoc keeps"     'declare -a x=(1 2); declare -A x; echo rc=$?; declare -p x'
check "promote then refuse"    'x=v; declare -a x; declare -p x; declare -A x; echo rc=$?'
check "refused then element"   'declare -A x; x[k]=1; declare -a x; echo rc=$?; x[j]=2; declare -p x'
check "refused, subscript 0"   'declare -A x; declare -a x; x[0]=z; declare -p x'
check "refuse with -g"         'declare -A x; declare -ga x; echo rc=$?; declare -p x'
check "refuse with -i"         'declare -a y; declare -Ai y; echo rc=$?; declare -p y'
check "unset after refusal"    'declare -A x; unset x; declare -a x; declare -p x'
check "declare -g then reshape" 'f(){ declare -g -A g; }; f; declare -a g; echo rc=$?; declare -p g'

# --- idempotent: same shape twice is a no-op --------------------------------
check "idx -> idx"             'declare -a x; declare -a x; declare -p x'
check "assoc -> assoc"         'declare -A x; declare -A x; declare -p x'
check "idx -> idx with value"  'declare -a y=(1 2); declare -a y; declare -p y'
check "assoc -> assoc w/value" 'declare -A x=([k]=1); declare -A x; declare -p x'

# --- readonly guards the VALUE, not the SHAPE (#734) ------------------------
check "readonly -> indexed"    'readonly R=1; declare -a R; declare -p R; echo rc=$?'
check "readonly -> assoc"      'readonly R=1; declare -A R; declare -p R; echo rc=$?'
# The conversion refusal is reported INSTEAD of the readonly message: bash
# checks the shape before it checks whether the value may change.
check "refusal beats readonly" 'declare -A x; readonly x; declare -a x; echo rc=$?; declare -p x'
check "readonly scalar -> idx" 'x=v; readonly x; declare -a x; echo rc=$?; declare -p x'
check "readonly scalar -> asc" 'x=v; readonly x; declare -A x; echo rc=$?; declare -p x'

# --- inside a function ------------------------------------------------------
# Already local in THIS frame: the shape change discards the value.
check "in-frame discard idx"   'f(){ local v=s; local -a v; declare -p v; }; f'
check "in-frame discard assoc" 'f(){ local v=s; local -A v; declare -p v; }; f'
check "in-frame idx -> assoc"  'f(){ local -a v; local -A v; echo rc=$?; declare -p v; }; f'
check "in-frame assoc -> idx"  'f(){ local -A v; local -a v; echo rc=$?; declare -p v; }; f'
check "reassign after reshape" 'f(){ local -a v; local v=s; declare -p v; }; f'
check "reassign after assoc"   'f(){ local -A v; local v=s; declare -p v; }; f'
# A FRESH local over an outer binding starts from nothing — it does not
# promote the shadowed value, and it does not inherit the outer SHAPE.
check "fresh local over scalar" 'x=v; f(){ local -a x; declare -p x; }; f'
check "declare over scalar"     'x=v; f(){ declare -a x; declare -p x; }; f'
check "local shadows assoc"     'declare -A x; f(){ local -a x; declare -p x; echo rc=$?; }; f; declare -p x'
check "local shadows indexed"   'declare -a y=(1); f(){ local -A y; declare -p y; echo rc=$?; }; f; declare -p y'
# `declare` without -g inside a function is `local`, so the same fresh rule.
check "declare shadows assoc"   'declare -A x; f(){ declare -a x; declare -p x; echo rc=$?; }; f; declare -p x'
check "local then scalar local" 'x=v; f(){ local x=w; local -a x; declare -p x; }; f'

harness_summary
