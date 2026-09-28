#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #784: `declare +a` / `+A` and the
# "cannot destroy array variables in this way" refusal.
#
# The rule is PER NAME and shape-matched (`builtins/declare.def:854`):
#
#   * `+a` objects only to an INDEXED array, `+A` only to an ASSOCIATIVE one —
#     the shapes do not cross, and a scalar or an absent name is accepted
#     silently;
#   * the diagnostic names the VARIABLE, not the flag;
#   * a refused name is skipped entirely (no attribute and no assignment is
#     applied to it) while the REMAINING names are still processed, rc 1;
#   * it sits between the readonly-assignment refusal and the shape-conversion
#     table, so `declare -ar y=(1); declare +a y=2` says `readonly variable`
#     while `declare -A x; declare +a -a x` says `cannot convert …`;
#   * `local` enforces it too.
#
# huck used to reject `+a`/`+A` in the OPTION loop, unconditionally, naming the
# flag — so `declare +a v` on a plain scalar errored, and `local +a` did not
# error at all.
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

# --- accepted silently: no matching shape ------------------------------------
check "+a on absent"        'declare +a v; echo rc=$?; declare -p v'
check "+A on absent"        'declare +A v; echo rc=$?; declare -p v'
check "+a on scalar"        'v=s; declare +a v; echo rc=$?; declare -p v'
check "+A on scalar"        'v=s; declare +A v; echo rc=$?; declare -p v'
check "+a on unset scalar"  'declare v; declare +a v; echo rc=$?; declare -p v'
# The shapes do not cross.
check "+a on associative"   'declare -A x; declare +a x; echo rc=$?; declare -p x'
check "+A on indexed"       'declare -a y; declare +A y; echo rc=$?; declare -p y'
check "+a on scalar assign" 'declare +a v=1; echo rc=$?; declare -p v'

# --- refused: the matching shape ---------------------------------------------
check "+a on indexed"       'declare -a y; declare +a y; echo rc=$?; declare -p y'
check "+A on associative"   'declare -A x; declare +A x; echo rc=$?; declare -p x'
check "+a on valued idx"    'declare -a y=(1 2); declare +a y; echo rc=$?; declare -p y'
check "+A on valued assoc"  'declare -A x=([k]=v); declare +A x; echo rc=$?; declare -p x'
check "+a with an assign"   'declare -a y=(1); declare +a y=2; echo rc=$?; declare -p y'
check "+ax on indexed"      'declare -ax y; declare +ax y; echo rc=$?; declare -p y'
check "+Ai on associative"  'declare -Ai x; declare +Ai x; echo rc=$?; declare -p x'

# --- the name is skipped, the rest are not -----------------------------------
check "later name survives" 'declare -a y; declare +a y newname; echo rc=$?; declare -p newname'
check "earlier name first"  'declare -a y; declare +a newname y; echo rc=$?; declare -p newname'
check "later assign runs"   'declare -a y; declare +a y v=3; echo rc=$?; declare -p v'
check "two refusals"        'declare -a y; declare -a z; declare +a y z; echo rc=$?; declare -p y; declare -p z'

# --- ordering against the neighbouring refusals ------------------------------
# readonly-assignment wins over the destroy refusal …
check "readonly assign wins" 'declare -ar y=(1); declare +a y=2; echo rc=$?; declare -p y'
# … and the destroy refusal wins over the conversion refusal, but only when the
# shape it names matches.
check "destroy beats convert" 'declare -A x; declare +A -a x; echo rc=$?; declare -p x'
check "convert when no match" 'declare -A x; declare +a -a x; echo rc=$?; declare -p x'
check "destroy beats convert2" 'declare -a y; declare +a -A y; echo rc=$?; declare -p y'

# NOT here: `typeset +a y` — bash prefixes the diagnostic with `typeset:` and
# huck says `declare:` for every message in this builtin, which is its own
# divergence (#788). The row belongs to that fix, not this one.

# --- local enforces it too ---------------------------------------------------
check "local +a scalar"     'f(){ local +a v; echo rc=$?; declare -p v; }; f'
check "local +a on local a" 'f(){ local -a y; local +a y; echo rc=$?; declare -p y; }; f'
check "local +A on local A" 'f(){ local -A x; local +A x; echo rc=$?; declare -p x; }; f'
check "local +a with assign" 'f(){ local -a y=(1); local +a y=2; echo rc=$?; declare -p y; }; f'
check "local +a crosses"    'f(){ local -A x; local +a x; echo rc=$?; declare -p x; }; f'
check "local +a over global" 'declare -a g=(1); f(){ local +a g; echo rc=$?; declare -p g; }; f; declare -p g'
check "local +A over global" 'declare -A gm=([k]=v); f(){ local +A gm; echo rc=$?; declare -p gm; }; f; declare -p gm'

# --- readonly / export take -a/-A as SELECTORS (#698), so `+a` is inert -----
check "readonly +a"         'declare -a y; readonly +a y; echo rc=$?; declare -p y'
check "export +a"           'declare -a y; export +a y; echo rc=$?; declare -p y'

harness_summary
