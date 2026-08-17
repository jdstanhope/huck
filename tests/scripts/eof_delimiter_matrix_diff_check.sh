#!/usr/bin/env bash
# Which delimiter an unexpected EOF names, across the whole nesting space
# (v362, #643). bash reports at the innermost still-open MATCHED PAIR; this
# asserts huck agrees on every cell except a named list of open issues.
#
# The 813 cells are NOT restated here. `tools/eof_matrix.sh` owns them —
# 15 contexts x 11 openers at depth 1, plus 8 outers x 9 middles x 9 inners at
# depth 2 — and a second copy of those tables in this file would be two things
# that must be edited together, which is exactly the shape that goes stale. This
# harness drives the tool and judges its verdicts.
#
# The gate is one-directional on purpose: **no cell outside the list below may
# diverge**. A cell that starts AGREEING is reported as a note, not a failure —
# a later fix must not turn this harness red. `tools/eof_matrix.sh --check` is
# the two-directional version used while a change is in flight.
#
# Each skip carries its issue. There is no other filter: a fragment that fails
# for an unrelated reason still fails here.
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ── Known-divergent cells, as DEPTH/CONTEXT/MIDDLE/INNER ──────────────────────
#
# Three families of six. v362 closed 60 of the 78 cells that diverged when
# `tools/eof_matrix.sh` was written; these are what is left, and every one has an
# open issue. (v362's plan predicted 12 remaining — it treated #634 as a single
# fix, but its `${${x` half turned out to be an unimplemented construct rather
# than a misreported one, which is #650.)
KNOWN_DIFF=$(cat <<'COORDS'
# Not matched-pair shapes at all — a different message family (by design of the
# v360/v362 scope, not an oversight).
1/word/-/paren          # `echo (` — huck reads it as a function definition
1/arraylit/-/paren      # `v=((`  — bash's near-token error
1/dbracket/-/none       # `[[ a == `  conditional-expression wording
1/dbracket/-/paren      # `[[ a == (`
1/dbracket/-/escdq      # `[[ a == \"`
1/dbracket/-/escsq      # `[[ a == \'`
# #631 / #640 — a `'` inside a `${…}` is a matched pair in bash and swallows the
# `}`. Changes what PARSES, not just what is reported.
2/dq/operand/sq         # echo "${x:-'
2/dq/bracename/sq       # echo "${'
2/arith/operand/sq      # echo $((1+${x:-'
2/arith/bracename/sq    # echo $((1+${'
2/legacy/operand/sq     # echo $[1+${x:-'
2/legacy/bracename/sq   # echo $[1+${'
# #650 — a nested `${` in name position is `unsupported expansion` in huck and a
# bad substitution in bash. The construct is unimplemented, so there is no pair
# to report yet.
2/none/bracename/brace      # echo ${${x
2/dq/bracename/brace        # echo "${${x
2/arith/bracename/brace     # echo $((1+${${x
2/legacy/bracename/brace    # echo $[1+${${x
2/comsub/bracename/brace    # echo $(${${x
2/arraylit/bracename/brace  # v=(${${x
COORDS
)
known() { printf '%s\n' "$KNOWN_DIFF" | sed 's/#.*//' | tr -d '[:blank:]' | grep -v '^$'; }

MATRIX="$(dirname "${BASH_SOURCE[0]}")/../../tools/eof_matrix.sh"
[[ -x "$MATRIX" ]] || {
    echo "missing $MATRIX" >&2
    exit 1
}

rows=$(HUCK_BIN="$HUCK_BIN" "$MATRIX" 2>/dev/null)
total=$(printf '%s\n' "$rows" | tail -n +2 | grep -c .)
[[ "$total" -gt 800 ]] || {
    # A truncated run would make every assertion below vacuously true.
    echo "FAIL: eof_matrix produced only $total cells (expected 813)" >&2
    exit 1
}

# Every DIFF cell, as a coordinate.
diverged=$(printf '%s\n' "$rows" | awk -F'\t' '$8=="DIFF" {printf "%s/%s/%s/%s\n", $1,$2,$3,$4}')

unexpected=$(comm -23 <(printf '%s\n' "$diverged" | sort) <(known | sort))
fixed=$(comm -13 <(printf '%s\n' "$diverged" | sort) <(known | sort))

# One assertion per cell, so the tally reflects the space actually covered.
while IFS=$'\t' read -r depth ctx mid inner frag b h verdict; do
    [[ "$depth" == DEPTH ]] && continue
    coord="$depth/$ctx/$mid/$inner"
    if printf '%s\n' "$unexpected" | grep -qxF "$coord"; then
        compare "$coord  [$frag]" "$b" "$h"
    else
        # Agrees, or is a listed known divergence.
        PASS=$((PASS + 1))
    fi
done <<<"$rows"

printf 'cells: %s, known-divergent: %s\n' "$total" "$(known | grep -c .)"
if [[ -n "$fixed" ]]; then
    echo "NOTE: these are listed as known-divergent but now AGREE — prune the list:"
    printf '%s\n' "$fixed" | sed 's/^/    /'
fi

harness_summary
