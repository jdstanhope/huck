#!/usr/bin/env bash
# Characterization guard for ${...} bad-substitution raw assembly (#107).
#
# v279 deletes scan_braced_operand (lexer.rs), the last lexer forward-scanner:
# today it hand-rolls depth/quote/$(...)/$'...' matching to reconstruct the
# verbatim `${...}` raw for the ParamBadSubst { raw } diagnostic. The refactor
# moves that assembly to the PARSER (via a source_span accessor). This harness
# locks in huck's CURRENT (pre-refactor) output for every input that routes
# through scan_braced_operand, so Tasks 3-4 can prove byte-identical output
# after the deletion.
#
# Recorded baseline (captured by running current huck; see
# docs/superpowers/plans/2026-07-10-param-badsubst-no-forward-scan.md and
# .superpowers/sdd/task-1-report.md for the full capture transcript):
#
#   input          | rc | message (after stripping the "<prog>: " prefix)
#   ---------------+----+---------------------------------------------------
#   ${}            |  1 | line 1: ${}: bad substitution        [== bash]
#   ${@Z}          |  1 | line 1: ${@Z}: bad substitution       [== bash]
#   ${#x@}         |  1 | line 1: ${#x@}: bad substitution      [== bash]
#   ${x@Z}         |  1 | line 1: ${x@Z}: bad substitution      [bash: rc=0, no output]
#   ${x@}          |  1 | line 1: ${x@}: bad substitution       [bash: rc=0, no output]
#   ${!x@Z}        |  1 | line 1: ${!x@Z}: bad substitution     [bash: "x: invalid indirect expansion"]
#   ${$'y'}        |  1 | line 1: ${$'y'}: bad substitution     [bash: "${'y'}: bad substitution"]
#   ${a$'b'}       |  1 | line 1: ${a$'b'}: bad substitution    [bash: "${a'b'}: bad substitution"]
#   ${x  (no `}`)  |  2 | line 2: syntax error: unexpected end of input
#                  |    | [bash: "unexpected EOF while looking for matching `}'" rc=2]
#
# The first three rows are huck==bash today; the refactor must keep them that
# way (asserted with a live bash diff below, not just the recorded string).
# The remaining rows are PRE-EXISTING huck/bash divergences (out of scope for
# #107 — see the spec's non-goals); this harness pins huck's own current
# wording so the refactor cannot silently change it.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
BASH_BIN="${BASH_BIN:-bash}"
PASS=0; FAIL=0

# huck's <prog> prologue field is $0 verbatim (its own invocation path), which
# differs textually from bash's "bash". Normalize by substituting huck's own
# invocation string for "bash" so the two sides' prologues line up (same
# technique as tests/scripts/error_message_diff_check.sh's normalize()).
normalize() {
    printf '%s' "${1//$HUCK_BIN/bash}"
}

# check_baseline: run `frag` through huck only; assert its normalized
# combined (stdout+stderr+exit) output matches the recorded baseline exactly.
check_baseline() {
    local label="$1" frag="$2" expected="$3" h
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    h=$(normalize "$h")
    if [[ "$h" == "$expected" ]]; then
        printf 'PASS: %s (baseline)\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (baseline)\n' "$label"
        diff <(printf '%s\n' "$expected") <(printf '%s\n' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# check_bash_match: for the huck==bash rows, additionally assert byte-identical
# vs a live bash run (after the same prologue normalization).
check_bash_match() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | "$BASH_BIN" --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    h=$(normalize "$h")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s (vs bash)\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (vs bash)\n' "$label"
        diff <(printf '%s\n' "$b") <(printf '%s\n' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

check_baseline "empty ops \${}"        'echo ${}'    'bash: line 1: ${}: bad substitution
EXIT:1'
check_bash_match "empty ops \${}"      'echo ${}'

check_baseline "@Z transform \${@Z}"   'echo ${@Z}'  'bash: line 1: ${@Z}: bad substitution
EXIT:1'
check_bash_match "@Z transform \${@Z}" 'echo ${@Z}'

check_baseline "hash-name-at \${#x@}"  'echo ${#x@}' 'bash: line 1: ${#x@}: bad substitution
EXIT:1'
check_bash_match "hash-name-at \${#x@}" 'echo ${#x@}'

check_baseline "x@Z transform \${x@Z}" 'echo ${x@Z}' 'bash: line 1: ${x@Z}: bad substitution
EXIT:1'

check_baseline "x@ empty \${x@}"       'echo ${x@}'  'bash: line 1: ${x@}: bad substitution
EXIT:1'

check_baseline "!x@Z indirect \${!x@Z}" 'echo ${!x@Z}' 'bash: line 1: ${!x@Z}: bad substitution
EXIT:1'

check_baseline "dollar-quote \${\$'y'}" $'echo ${$\'y\'}' 'bash: line 1: ${$'"'"'y'"'"'}: bad substitution
EXIT:1'

check_baseline "a-dollar-quote \${a\$'b'}" $'echo ${a$\'b\'}' 'bash: line 1: ${a$'"'"'b'"'"'}: bad substitution
EXIT:1'

check_baseline "unterminated \${x (EOF)" 'echo ${x' 'bash: line 2: syntax error: unexpected end of input
EXIT:2'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
