#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `[[ … ]]` operand expansion under
# `set -x` (#220).
#
# Expanding an operand is what RUNS a `<(cmd)` or `$(cmd)` inside it. huck
# expanded twice — once to build the trace line, once to evaluate — so the inner
# command ran twice, with its side effects, and ONLY under `set -x`. Every row
# below counts the side effect rather than the output, because a doubled
# execution is otherwise invisible: the second run's result is discarded.
#
# The trace BODY is checked separately, and is where the same fix shows up as a
# quoting change: bash renders each operand in the form its operator takes, so a
# quoted regex escapes (`[[ a.b =~ "a.b" ]]` -> `a.b =~ a\.b`). huck rendered a
# plain word expansion and lost that.
#
# NOT compared: a trace body containing a `/dev/fd/N` path (bash numbers from 63
# downward, huck from 3 upward) — the counting rows cover procsub instead.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-dbxt.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# How many times did the operand's inner command actually run?
check_runs() {
    local label="$1" frag="$2" b h d
    d=$(mktemp -d "$TMPROOT/case.XXXXXX")
    ( cd "$d" && timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" >/dev/null 2>&1 )
    b="runs:$(wc -l <"$d/c" 2>/dev/null || echo 0) rc-ignored"
    rm -f "$d/c"
    ( cd "$d" && timeout 10 "$HUCK_BIN" -c "$frag" >/dev/null 2>&1 )
    h="runs:$(wc -l <"$d/c" 2>/dev/null || echo 0) rc-ignored"
    rm -rf "$d"
    compare "$label" "$b" "$h"
}

# The `+ [[ … ]]` line itself.
check_trace() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "set -x; $frag" 2>&1 | grep '^+ \[\['; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "set -x; $frag" 2>&1 | grep '^+ \[\['; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the operand runs ONCE, traced or not ---
check_runs "procsub traced"     'set -x; [[ -e <(echo M >>c) ]]'
check_runs "procsub untraced"   '[[ -e <(echo M >>c) ]]'
check_runs "comsub rhs traced"  'set -x; [[ "x" == "$(echo M >>c; echo x)" ]]'
check_runs "comsub lhs traced"  'set -x; [[ $(echo M >>c; echo a) == a ]]'
check_runs "comsub unary"       'set -x; [[ -n $(echo M >>c) ]]'
check_runs "two procsubs"       'set -x; [[ -e <(echo M >>c) && -e <(echo M >>c) ]]'
check_runs "negated procsub"    'set -x; [[ ! -e <(echo M >>c) ]]'
check_runs "regex rhs"          'set -x; [[ a =~ $(echo M >>c; echo a) ]]'
check_runs "arith rhs"          'set -x; [[ 1 -eq $(echo M >>c; echo 1) ]]'
check_runs "file compare rhs"   'set -x; [[ c -nt $(echo M >>c; echo c) ]]'
check_runs "short-circuit skips" 'set -x; [[ -n "" && -e <(echo M >>c) ]]'
check_runs "or short-circuits"  'set -x; [[ -n x || -e <(echo M >>c) ]]'

# --- the trace body: each operand in the form its operator takes ---
check_trace "unary"             '[[ -n "$HOME" ]]'
check_trace "empty operand"     '[[ -z "" ]]'
check_trace "negated"           '[[ ! -e /nope ]]'
check_trace "glob rhs bare"     '[[ abc == a* ]]'
check_trace "regex quoted"      '[[ "a.b" =~ "a.b" ]]'
check_trace "regex from comsub" '[[ x =~ $(echo "a.b") ]]'
check_trace "string compare"    '[[ a < b ]]'
check_trace "arith compare"     '[[ 1 -eq $(echo 1) ]]'
check_trace "file compare"      '[[ /etc -nt /nope ]]'
check_trace "varset"            '[[ -v HOME ]]'
check_trace "optenabled"        '[[ -o xtrace ]]'
check_trace "and connective"    '[[ -n x && -n y ]]'
check_trace "or connective"     '[[ -z x || -n y ]]'

harness_summary
