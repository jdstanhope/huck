#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #269: `[[ ]]` and `(( ))` must fire
# the DEBUG trap like any other command.
#
# The issue was filed as "DEBUG fires once, not twice, for a `stmt && stmt`
# operand", but `&&` turns out to have nothing to do with it — `true && echo x`
# was already correct. Re-measured 2026-08-06: huck never fired DEBUG for a
# `[[ ]]` or `(( ))` command at all, connector or no connector, so
# `$BASH_COMMAND` and `$LINENO` were never stamped for them either.
#
# Fire ORDER matters and is pinned below: bash runs the DEBUG action BEFORE
# the command's own `set -x` line.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$( ulimit -v 800000; timeout 10 bash --norc --noprofile -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    h=$( ulimit -v 800000; timeout 10 "$HUCK_BIN" -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the fire itself, with and without a connector -------------------------
check "[[ ]] alone"            'trap "echo D" DEBUG; [[ 1 == 1 ]]; echo x'
check "[[ ]] false"            'trap "echo D" DEBUG; [[ 1 == 2 ]]; echo x'
check "(( )) alone"            'trap "echo D" DEBUG; (( 1 )); echo x'
check "(( )) zero"             'trap "echo D" DEBUG; (( 0 )); echo x'
check "[[ ]] && cmd"           'trap "echo D" DEBUG; [[ 1 == 1 ]] && echo x'
check "[[ ]] || cmd"           'trap "echo D" DEBUG; [[ 1 == 2 ]] || echo x'
check "(( )) && cmd"           'trap "echo D" DEBUG; (( 1 )) && echo x'
check "cmd && [[ ]]"           'trap "echo D" DEBUG; true && [[ 1 == 1 ]]'
check "[[ ]] in if condition"  'trap "echo D" DEBUG; if [[ 1 == 1 ]]; then echo t; fi'
check "(( )) in while"         'trap "echo D" DEBUG; i=0; while (( i < 2 )); do i=$(( i + 1 )); done'
check "[[ ]] in a function"    'trap "echo D" DEBUG; set -T; f() { [[ 1 == 1 ]]; }; f'
check "[[ ]] negated"          'trap "echo D" DEBUG; ! [[ 1 == 2 ]]'

# --- exit status is unaffected by the fire ---------------------------------
check "[[ ]] status preserved"  'trap "echo D" DEBUG; [[ 1 == 2 ]]; echo rc=$?'
check "(( )) status preserved"  'trap "echo D" DEBUG; (( 0 )); echo rc=$?'
check "[[ ]] status, no trap"   '[[ 1 == 2 ]]; echo rc=$?'

# --- $BASH_COMMAND is the command's SOURCE text ----------------------------
# ⚠️ bash renders the two differently: `[[ ]]` is NORMALIZED (runs of spaces
# collapse) while `(( ))` is VERBATIM (inner spacing preserved). Both are
# unexpanded — `$x` stays `$x`.
check "BASH_COMMAND [[ ]]"      'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; [[ 1 == 1 ]]'
check "BASH_COMMAND [[ ]] spaced" 'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; [[    1   ==   1   ]]'
check "BASH_COMMAND [[ ]] quoted" 'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; [[ -n "a b" ]]'
check "BASH_COMMAND [[ ]] unexpanded" 'x=hello; trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; [[ $x == a* ]]'
check "BASH_COMMAND (( ))"      'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; (( 1+1 ))'
check "BASH_COMMAND (( )) tight" 'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; ((1+1))'
check "BASH_COMMAND (( )) spaced" 'trap "printf \"[%s]\n\" \"\$BASH_COMMAND\"" DEBUG; ((   1  +  1   ))'

# --- $LINENO at the fire ---------------------------------------------------
check "LINENO [[ ]]"           'trap "echo L=\$LINENO" DEBUG
[[ 1 == 1 ]]'
check "LINENO (( ))"           'trap "echo L=\$LINENO" DEBUG
(( 1 ))'

# --- the DEBUG action runs BEFORE the command's own xtrace line ------------
check "xtrace order [[ ]]"     'set -x; trap "echo D" DEBUG; [[ 1 == 1 ]]'
check "xtrace order (( ))"     'set -x; trap "echo D" DEBUG; (( 1 ))'

# --- extdebug: a skipped [[ ]] / (( )) returns 0, like every other command --
check "extdebug skip [[ ]]"    'shopt -s extdebug; false; trap "trap - DEBUG; false" DEBUG; [[ 1 == 1 ]]; echo rc=$?'
check "extdebug skip (( ))"    'shopt -s extdebug; false; trap "trap - DEBUG; false" DEBUG; (( 1 )); echo rc=$?'
check "extdebug no skip"       'shopt -s extdebug; trap "trap - DEBUG; true" DEBUG; [[ 1 == 2 ]]; echo rc=$?'
check "extdebug return 2"      'shopt -s extdebug; set -T; f() { trap "trap - DEBUG; exit 2" DEBUG; [[ 1 == 1 ]]; echo unreached; }; f; echo rc=$?'

# --- regression guards -----------------------------------------------------
check "simple command"         'trap "echo D" DEBUG; true; echo x'
check "and-or of simples"      'trap "echo D" DEBUG; true && echo x'
check "arith for header"       'trap "echo D" DEBUG; for ((i=0;i<2;i++)); do echo $i; done'
check "case header"            'trap "echo D" DEBUG; case x in x) echo hit;; esac'
check "test builtin"           'trap "echo D" DEBUG; test 1 = 1; echo x'
check "arith substitution"     'trap "echo D" DEBUG; echo $(( 1 + 1 ))'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
