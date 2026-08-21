#!/usr/bin/env bash
# Byte-identical bash<->huck harness for what a function-local SHADOW inherits
# (#539 for `local`, #694 for `declare`/`typeset`).
#
# bash's `local NAME` creates a FRESH variable: it carries only the attributes
# its own flags ask for, plus the export attribute of the variable it shadows.
# Nothing else crosses the boundary — not the outer value, not its shape
# (indexed/associative/nameref), not `-i`/`-l`/`-u`. `declare` without `-g`
# inside a function creates the same kind of binding and follows the same rule;
# `declare -g` writes the global and is not a shadow at all.
#
# NOT byte-compared here:
#   - bare `local V` shadowing an EXPORTED outer variable: bash keeps the
#     variable declared-but-unset AND exported; huck has no declared-but-unset
#     state (#600), so it cannot represent "exported and unset" at once (#691).
#   - `-r`: shadowing a readonly outer is an error in both, but the message
#     PREFIX diverges as for every builtin diagnostic.
#   - what a CHILD PROCESS sees after `local +x V=2` over an exported outer:
#     bash still exports the outer `V=1` there, huck exports nothing (#692).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- attributes are NOT inherited by the local ---
check "shadow: -i dropped"      'declare -i N=5; f(){ local N=abc; declare -p N; }; f'
check "shadow: -l dropped"      'declare -l L=AB; f(){ local L=CD; declare -p L; }; f'
check "shadow: -u dropped"      'declare -u U=ab; f(){ local U=cd; declare -p U; }; f'
check "shadow: -n dropped"      'declare -n P=zz; f(){ local P=2; declare -p P; }; f'

# --- the outer SHAPE and VALUE are not inherited either ---
check "shadow: indexed->scalar" 'declare -a A=(1 2); f(){ local A=x; declare -p A; }; f'
check "shadow: assoc->scalar"   'declare -A M=([k]=v); f(){ local M=x; declare -p M; }; f'
check "shadow: assoc->indexed"  'declare -A M=([k]=v); f(){ local M=([z]=9); declare -p M; }; f'
check "shadow: indexed literal" 'declare -a A=(1 2); f(){ local A=(9); declare -p A; }; f'

# --- bare `local -a/-A` starts empty, it does not adopt the outer value ---
#
# Compared by CONTENTS, not by `declare -p`: bash renders a declared-but-unset
# array as `declare -a N` where huck renders `declare -a N=()`, which is #600
# (no declared-but-unset state), a different divergence from this one.
check "bare -a over scalar"     'N=5; f(){ local -a N; echo "${#N[@]}"; N+=(z); echo "${N[*]}"; }; f'
check "bare -a over indexed"    'declare -a A=(1 2); f(){ local -a A; echo "${#A[@]}"; A+=(z); echo "${A[*]}"; }; f'
check "bare -a over assoc"      'declare -A M=([k]=v); f(){ local -a M; echo "${#M[@]}"; M+=(z); echo "${M[*]}"; }; f'
check "bare -A over indexed"    'declare -a A=(1 2); f(){ local -A A; echo "${#A[@]}"; A[q]=z; echo "${A[q]}"; }; f'

# --- export IS inherited (the one attribute that crosses) ---
check "shadow: -x kept"         'declare -x V=1; f(){ local V=2; declare -p V; }; f'
check "shadow: -x kept, -i not" 'declare -ix N=5; f(){ local N=abc; declare -p N; }; f'
check "shadow: -x in child env" 'declare -x V=1; f(){ local V=2; env | grep "^V="; }; f'
check "shadow: -x onto -n"      'declare -x V=1; f(){ local -n V=t; declare -p V; }; f'
check "shadow: -x onto -a"      'declare -x V=1; f(){ local -a V=(1); declare -p V; }; f'
check "shadow: +x cancels -x"   'declare -x V=1; f(){ local +x V=2; declare -p V; }; f'
check "shadow: +i is a no-op"   'declare -x V=1; f(){ local +i V=2; declare -p V; }; f'

# --- a nested function's local shadows the CALLER's local, also freshly ---
check "nested: fresh over local" 'g(){ local N=inner; declare -p N; }; f(){ local -i N=5; g; }; f'

# --- the local's OWN flags still win ---
check "own -i over outer plain"  'N=5; f(){ local -i N=3+4; declare -p N; }; f'
check "own -i over outer -i"     'declare -i N=5; f(){ local -i N=3+4; declare -p N; }; f'

# --- a repeat `local` in the SAME frame is not a fresh shadow ---
check "repeat: keeps -i"         'f(){ local -i x=1; local x=2+3; declare -p x; }; f'
check "repeat: bare keeps value" 'f(){ local -i x=1; local x; declare -p x; }; f'

# --- the outer binding is restored on return ---
check "restore: -i scalar"       'declare -i N=5; f(){ local N=abc; }; f; declare -p N'
check "restore: indexed array"   'declare -a A=(1 2); f(){ local A=x; }; f; declare -p A'
check "restore: assoc array"     'declare -A M=([k]=v); f(){ local M=x; }; f; declare -p M'
check "restore: nameref"         'declare -n P=zz; f(){ local P=2; }; f; declare -p P'

# --- `declare` inside a function is the same fresh shadow (#694) ---
check "decl: -i dropped"        'declare -i N=5; f(){ declare N=abc; declare -p N; }; f'
check "decl: -l dropped"        'declare -l L=AB; f(){ declare L=CD; declare -p L; }; f'
check "decl: -n dropped"        'declare -n P=zz; f(){ declare P=2; declare -p P; }; f'
check "decl: indexed->scalar"   'declare -a A=(1 2); f(){ declare A=x; declare -p A; }; f'
check "decl: assoc->scalar"     'declare -A M=([k]=v); f(){ declare M=x; declare -p M; }; f'
check "decl: assoc->indexed"    'declare -A M=([k]=v); f(){ declare M=([z]=9); declare -p M; }; f'
check "decl: bare hides value"  'N=5; f(){ declare N; echo "[${N-UNSET}]"; }; f'
check "decl: -a starts empty"   'declare -A m=([k]=v); f(){ declare -a m; echo "n=${#m[@]}"; m+=(z); echo "${m[*]}"; }; f'
check "decl: -x kept"           'declare -x V=1; f(){ declare V=2; declare -p V; }; f'
check "decl: -x in child env"   'declare -x V=1; f(){ declare V=2; env | grep "^V="; }; f'
check "decl: +x cancels -x"     'declare -x V=1; f(){ declare +x V=2; declare -p V; }; f'
check "decl: typeset too"       'declare -i N=5; f(){ typeset N=abc; declare -p N; }; f'
check "decl: nested fresh"      'declare -a A=(1 2); g(){ declare A=inner; declare -p A; }; f(){ declare A=mid; g; }; f'
check "decl: repeat keeps -i"   'f(){ declare -i x=1; declare x=2+3; declare -p x; }; f'
check "decl: after local -i"    'f(){ local -i x=1; declare x=2+3; declare -p x; }; f'
check "decl: restore on return" 'declare -i N=5; f(){ declare N=abc; }; f; declare -p N'

# --- `declare -g` is NOT a shadow: it writes the global, attributes and all ---
check "decl -g: keeps outer -i" 'declare -i N=5; f(){ declare -g N=abc; declare -p N; }; f'
check "decl -g: persists out"   'declare -i N=5; f(){ declare -g N=7; }; f; declare -p N'

# --- a READONLY name is refused, never cleared (the clear must not disarm
#     `declare`'s own readonly guards) ---
#
# stderr is dropped on these three: both shells diagnose, but the `$0` PREFIX
# of a builtin diagnostic diverges (`main:` vs `environment:`) as it does for
# every builtin. What is compared is the status and the surviving value —
# which is what a bypassed guard would change.
check "decl -r: assign refused"  'readonly R=1; f(){ declare R=2 2>/dev/null; echo "rc=$?"; declare -p R; }; f'
check "decl -r: -i refused"      'readonly R=1; f(){ declare -i R 2>/dev/null; echo "rc=$?"; declare -p R; }; f'
check "local -r: assign refused" 'readonly R=1; f(){ local R=2 2>/dev/null; echo "rc=$?"; declare -p R; }; f'

# --- top level is not a shadow either: `declare -a` promotes the scalar ---
check "top: -a promotes scalar" 'x=hello; declare -a x; declare -p x'
check "top: -i then plain"      'declare -i N=5; declare N=abc; declare -p N'

harness_summary
