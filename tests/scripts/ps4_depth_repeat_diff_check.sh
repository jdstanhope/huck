#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v131: PS4 depth-repeat + the PS4
# expansion huck supports (escapes + $VAR). Compares STDERR only (set -x writes
# there), stdout discarded. Does NOT test $(...)/$((...))/$LINENO in PS4 — those
# are the known L-29 residual (huck's expand_prompt does not expand them).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null)
    compare "$label" "$b" "$h"
}

check "default depth nested"  'a=$(echo $(echo hi))'
check "cmdsub in function"    'f() { echo $(echo x); }; f'
check "eval depth"            'eval "echo ev"'
check "function no depth"     'g() { echo y; }; f() { g; }; f'
check "subshell no depth"     '( echo s )'
# For the cases below we set PS4 with xtrace OFF (set +x; PS4=...; set -x) so the
# PS4= assignment line itself is NOT traced. This avoids the known PS4-self-
# assignment timing edge (huck traces a PS4= line with the post-assign value,
# bash with the pre-assign value — see L-29) while still exercising the real
# depth-repeat + $VAR-expansion behaviour on the command under test.
check "custom first char"     'set +x; PS4="> "; set -x; a=$(echo hi)'
check "multichar ps4"         'set +x; PS4="XY "; set -x; a=$(echo hi)'
check "triple nest custom"    'set +x; PS4="# "; set -x; a=$(echo $(echo $(echo deep)))'
check "ps4 var expansion"     'set +x; P=Q; PS4="$P "; set -x; echo z'
check "default no regression" 'echo hi'

harness_summary
