#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `readonly`-attribute roots fixed in
# v349 (#343, `attr` category):
#   Root A: `readonly -a` indexed-array attribute.
#   Root C: the readonly-variable error is bare (`x: readonly variable`) for a
#           plain re-assignment, but keeps the `readonly:` prefix for an
#           attribute-change attempt (`-a`/`-A` with a non-array-literal RHS).
#   Root D: a QUOTED `name=value` arg reaching readonly/declare/export/local is
#           an assignment (bash re-checks the expanded word for `name=value`).
#   Root B: under `-a`/`-A`, a quoted `(...)` scalar value is coerced to an
#           array literal; without `-a` it stays a literal scalar.
# Each fragment runs through `bash -c` and `huck -c`; stdout+stderr+exit must
# match byte-for-byte.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# Normalize the leading program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — that prefix is the invoking binary's argv[0], a
# non-behavioral artifact of piping the fragment on stdin, not a huck<->bash
# difference. Everything after `: line N:` must still match byte-for-byte.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1 | norm; echo "EXIT:${PIPESTATUS[1]}")
    compare "$label" "$b" "$h"
}

# --- Root A: readonly -a indexed-array attribute ---
check "readonly -a compound"      'readonly -a x=(1 2); declare -p x'
check "readonly -a single"        'readonly -a r=(7); declare -p r'
check "readonly -A still works"   'readonly -A m=([k]=v); declare -p m'

# --- Root C: readonly-variable error prefix (bare vs attribute-change) ---
check "reassign readonly bare"    'readonly x=1; readonly x=2'
check "already-ro bare"           'x=1; readonly x; x=5'
check "attr-change -a keeps pfx"  'readonly r=(1); readonly -a r=2'
check "-a array-literal is bare"  'readonly r=(1); readonly -a r=(2)'

# --- Root D: quoted `name=value` arg is an assignment ---
check "readonly quoted scalar"    "readonly 'x=hi'; declare -p x"
check "declare quoted scalar"     "declare 'x=hi'; declare -p x"
check "export quoted scalar"      "export 'x=hi'; declare -p x"
check "local quoted scalar"       "f(){ local 'x=hi'; declare -p x; }; f"
check "readonly quoted preexist"  'c=(outside); readonly '"'"'c=(3)'"'"'; declare -p c'
# KEEP: invalid identifier stays rejected.
check "quoted invalid ident"      "readonly '3x=1'"

# --- Root B: `-a`/`-A` coerces a quoted `(...)` scalar to an array ---
check "readonly -a quoted paren"  "d=(outside); readonly -a 'd=(4)'; declare -p d"
check "readonly -a r='(7)'"       "readonly -a r='(7)'; declare -p r"
check "export -a quoted paren"    "export -a r='(7)'; declare -p r"
check "declare -a quoted paren"   "declare -a r='(7)'; declare -p r"
# KEEP: without -a, a quoted `(...)` stays a literal scalar.
check "no -a keeps literal paren" "declare p='(3)'; declare -p p"
check "export no -a literal"      "export r='(5)'; declare -p r"

# --- what `readonly` actually protects (#695, #734) ---
#
# bash refuses to create a function-LOCAL SHADOW of a readonly name, whatever
# flags are given, but permits ATTRIBUTE changes to the existing binding — the
# readonly flag guards the VALUE, not the attributes.
#
# NOT compared: `readonly R=1; declare -a R`, where bash reshapes the readonly
# scalar to `([0]="1")`. That is a shape change behind `Shell::assign`'s readonly
# check, which every writer in the shell shares; noted on #734.
check "ro: local shadow refused"   'readonly R=1; f(){ declare R; echo "rc=$?"; }; f'
check "ro: shadow with -x"         'readonly R=1; f(){ declare -x R; echo "rc=$?"; }; f'
check "ro: shadow with -i"         'readonly R=1; f(){ declare -i R; echo "rc=$?"; }; f'
check "ro: shadow with a value"    'readonly R=1; f(){ declare R=2; echo "rc=$?"; }; f'
check "ro: -g is not a shadow"     'readonly R=1; f(){ declare -g R; echo "rc=$?"; }; f'
check "ro: non-readonly shadows"   'V=1; f(){ declare V; echo "rc=$?"; }; f'
check "ro: top-level -i allowed"   'readonly R=1; declare -i R; declare -p R'
check "ro: top-level +i allowed"   'readonly R=1; declare +i R; declare -p R'
check "ro: top-level -x allowed"   'readonly R=1; declare -x R; declare -p R'
check "ro: top-level -l allowed"   'readonly R=1; declare -l R; declare -p R'
check "ro: bare declare allowed"   'readonly R=1; declare R; echo "rc=$?"; declare -p R'
check "ro: the VALUE is guarded"   'readonly R=5; declare -i R; R=7; declare -p R'
check "ro: assignment refused"     'readonly R=1; declare R=2; echo "rc=$?"; declare -p R'
check "ro: local refused"          'readonly R=1; f(){ local R; echo "rc=$?"; }; f'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
