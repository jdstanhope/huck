#!/usr/bin/env bash
# Differential fd/redirect audit: huck vs bash 5.2. For each (context, redirect)
# case, run both shells identically in a fresh temp dir and compare ALL
# observables: combined stdout+stderr stream, rc, resulting file contents, and
# $v (the {var} fd number). Reports only DIVERGENCES.
set -u
HUCK="${HUCK:-/home/john/projects/huck/target/debug/huck}"
PL="$(cd "$(dirname "$0")" && pwd)/redirect_audit_pl.sh"           # external payload (writes O to fd1, E to fd2)
PASS=0; DIV=0
declare -a DIVERGENCES=()

# In-process payload (compound), and the standard observable suffix.
IN='{ printf O"\n"; printf E"\n" >&2; }'
SUF='; __r=$?; printf "RC=%s V=%s\n" "$__r" "${v-unset}"'

norm() { sed -E 's#(/[^ ]*/)?(bash|huck)(\[[0-9]+\])?: ##g; s#line [0-9]+: ##g'; }

run_one() {   # $1=shell $2=fragment  -> prints "STREAM||rc||FILES"
    local sh="$1" frag="$2" T out files
    T=$(mktemp -d); printf 'F1\n' > "$T/f1"
    out=$(cd "$T" && timeout 5 "$sh" -c "$frag" </dev/null 2>&1; printf '\nrc=%s' "$?")
    out=$(printf '%s' "$out" | norm)
    files=$(cd "$T" && for x in f a b c d g x y z out; do [ -f "$x" ] && printf '%s=[%s] ' "$x" "$(tr "\n" ',' < "$x")"; done)
    rm -rf "$T"
    printf '%s ||FILES: %s' "$out" "$files"
}

check() {     # $1=label $2=fragment
    local label="$1" frag="$2" b h
    b=$(run_one bash   "$frag")
    h=$(run_one "$HUCK" "$frag")
    if [[ "$b" == "$h" ]]; then PASS=$((PASS+1))
    else
        DIV=$((DIV+1))
        DIVERGENCES+=("DIVERGE: $label
    frag:  $frag
    bash:  $b
    huck:  $h")
    fi
}

# Build a case in each of 3 contexts for a redirect string R and label L.
tri() {       # $1=label $2=redirects
    local L="$1" R="$2"
    check "inproc  $L" "$IN $R$SUF"
    check "exec    $L" "exec $R; $IN$SUF"
    check "extern  $L" "$PL $R$SUF"
}

# ---- source the case list ----
source "$(dirname "$0")/redirect_audit_cases.sh"

echo "=================================================================="
printf 'AUDIT: %d cases, %d agree, %d DIVERGE\n' "$((PASS+DIV))" "$PASS" "$DIV"
echo "=================================================================="
for d in "${DIVERGENCES[@]}"; do printf '%s\n\n' "$d"; done
