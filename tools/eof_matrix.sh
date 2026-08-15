#!/usr/bin/env bash
# EOF-delimiter matrix (v360, #635): which delimiter each shell names when input
# runs out inside nested constructs, and at which line.
#
# bash picks both from the innermost still-open MATCHED PAIR. huck picks the
# delimiter from whichever scanner happened to raise and the line from the
# innermost mode frame, and those are not the same thing — this tool measures
# the whole space so the model can be derived from bash rather than guessed.
#
# Two sweeps, each placing the fragment on LINE 3 of a 4-line script
# (`echo a`, `echo b`, <fragment>, `echo c`) so a line number that is right for
# the wrong reason — first line, last line, one past EOF — still shows up:
#
#   depth 1 — 15 contexts x 11 openers                        = 165 cells
#   depth 2 — 8 outers x 9 middles x 9 inners                 = 648 cells
#
# Usage:
#   tools/eof_matrix.sh [--tsv] > matrix.tsv     # TSV on stdout, summary on stderr
#   HUCK_BIN=/path/to/huck tools/eof_matrix.sh   # compare a specific build
#
# Columns: DEPTH CONTEXT MIDDLE INNER FRAGMENT BASH HUCK VERDICT
#   BASH/HUCK are "<line> <delimiter>" for a Shape 3 message
#   (`unexpected EOF while looking for matching X`), else "<line> ~<message>"
#   truncated. VERDICT is OK or DIFF.
#
# `--tsv` is accepted for explicitness; output is TSV either way. Every run is
# capped (`ulimit -v`, `timeout`) — an unbounded probe fragment has OOM-killed
# this box before.
set -u

CHECK=0
case "${1:-}" in
    --tsv) shift ;;
    --check) CHECK=1; shift ;;
esac
HUCK="${HUCK_BIN:-$(pwd)/target/debug/huck}"
BASH_BIN="${BASH_BIN:-bash}"
[ -x "$HUCK" ] || { echo "build huck first: $HUCK" >&2; exit 2; }

# ── The expected DIFF set ─────────────────────────────────────────────────────
# Every cell that diverged when this tool was written (v360 Task 2), as
# DEPTH/CONTEXT/MIDDLE/INNER. `--check` compares a live run against it and
# reports what LEFT (a task's fix) and what JOINED (a regression), so each task
# in the model's rollout leaves a reviewable edit to this list rather than to a
# generated artifact. /tools/*.tsv is gitignored by design — the baseline lives
# here, in the tool, where its changes are visible in review.
#
# 72 of these are in v360's scope. The other 6 are out of scope by the spec and
# stay: the four `[[ a == ` cells (conditional-expression wording), `echo (`
# (huck reads it as a function definition) and `v=((` — none of them is a
# matched-pair shape.
EXPECTED_DIFF=$(cat <<'COORDS'
1/arith/-/brace
1/arith/-/legacy
1/arithcmd/-/brace
1/arithcmd/-/legacy
1/arraylit/-/escdq
1/arraylit/-/escsq
1/arraylit/-/none
1/arraylit/-/paren
1/dbracket/-/escdq
1/dbracket/-/escsq
1/dbracket/-/none
1/dbracket/-/paren
1/forhdr/-/brace
1/forhdr/-/legacy
1/legacy/-/brace
1/word/-/paren
2/arith/arith/brace
2/arith/bracename/arith
2/arith/bracename/brace
2/arith/bracename/comsub
2/arith/bracename/escdq
2/arith/bracename/escsq
2/arith/bracename/none
2/arith/bracename/sq
2/arith/legacy/brace
2/arith/legacy/escdq
2/arith/legacy/escsq
2/arith/legacy/none
2/arith/none/brace
2/arith/operand/brace
2/arith/operand/escdq
2/arith/operand/escsq
2/arith/operand/none
2/arith/operand/sq
2/arraylit/arith/brace
2/arraylit/bq/bq
2/arraylit/bracename/arith
2/arraylit/bracename/brace
2/arraylit/bracename/comsub
2/arraylit/dq/dq
2/arraylit/legacy/brace
2/arraylit/none/escdq
2/arraylit/none/escsq
2/arraylit/none/none
2/arraylit/sq/escsq
2/arraylit/sq/sq
2/comsub/arith/brace
2/comsub/bracename/arith
2/comsub/bracename/brace
2/comsub/bracename/comsub
2/comsub/legacy/brace
2/dq/arith/brace
2/dq/bracename/arith
2/dq/bracename/brace
2/dq/bracename/comsub
2/dq/bracename/sq
2/dq/legacy/brace
2/dq/operand/sq
2/legacy/arith/brace
2/legacy/bracename/arith
2/legacy/bracename/brace
2/legacy/bracename/comsub
2/legacy/bracename/escdq
2/legacy/bracename/escsq
2/legacy/bracename/none
2/legacy/bracename/sq
2/legacy/legacy/brace
2/legacy/none/brace
2/legacy/operand/brace
2/legacy/operand/escdq
2/legacy/operand/escsq
2/legacy/operand/none
2/legacy/operand/sq
2/none/arith/brace
2/none/bracename/arith
2/none/bracename/brace
2/none/bracename/comsub
2/none/legacy/brace
COORDS
)

T=$(mktemp -d "${TMPDIR:-/tmp}/huck-eofmx.XXXXXX")
trap 'rm -rf "$T"' EXIT

# ── depth 1 ───────────────────────────────────────────────────────────────────
d1_contexts=(word dquote squote comsub backtick operand arith legacy arithcmd
             forhdr dbracket subscript arraylit subshell bracegrp)
declare -A CTX=(
    [word]='echo '        [dquote]='echo "'      [squote]="echo '"
    [comsub]='echo $('    [backtick]='echo `'    [operand]='echo ${x:-'
    [arith]='echo $((1+'  [legacy]='echo $[1+'   [arithcmd]='((1+'
    [forhdr]='for ((i=0;i<' [dbracket]='[[ a == ' [subscript]='echo ${a['
    [arraylit]='v=('      [subshell]='( '        [bracegrp]='{ '
)
d1_openers=(none dq sq bq comsub brace arith legacy paren escdq escsq)
declare -A OPEN=(
    [none]=''        [dq]='"'         [sq]="'"        [bq]='`'
    [comsub]='$('    [brace]='${x'    [arith]='$((1+' [legacy]='$[1+'
    [paren]='('      [escdq]='\"'     [escsq]="\\'"
)

# ── depth 2 ───────────────────────────────────────────────────────────────────
# `arraylit` is a BASE rather than a prefix: the assignment has to start the line.
d2_outers=(none dq sq arith legacy comsub bq arraylit)
declare -A OUTER=(
    [none]=''  [dq]='"'  [sq]="'"  [arith]='$((1+'  [legacy]='$[1+'
    [comsub]='$('  [bq]='`'  [arraylit]=''
)
d2_middles=(none dq sq operand bracename comsub bq arith legacy)
declare -A MID=(
    [none]=''  [dq]='"'  [sq]="'"  [operand]='${x:-'  [bracename]='${'
    [comsub]='$('  [bq]='`'  [arith]='$((1+'  [legacy]='$[1+'
)
d2_inners=(none dq sq bq comsub brace arith escdq escsq)
declare -A INNER=(
    [none]=''  [dq]='"'  [sq]="'"  [bq]='`'  [comsub]='$('
    [brace]='${x'  [arith]='$((1+'  [escdq]='\"'  [escsq]="\\'"
)

# "<line> <delim>" for a Shape 3 message, else "<line> ~<message prefix>".
extract() {
    local out="$1" l d
    l=$(printf '%s' "$out" | sed -n "s/.*line \([0-9]*\):.*/\1/p" | head -1)
    if printf '%s' "$out" | grep -q "matching"; then
        d=$(printf '%s' "$out" | sed -n "s/.*matching \`\(.*\)'.*/\1/p" | head -1)
        printf '%s %s' "${l:--}" "$d"
    else
        d=$(printf '%s' "$out" | sed -n "s/.*line [0-9]*: //p" | head -1 | cut -c1-28)
        printf '%s ~%s' "${l:--}" "$d"
    fi
}

ok=0; diff=0
emit_cell() {  # $1=depth $2=context $3=middle $4=inner $5=fragment
    local frag="$5" bo ho b h v
    printf 'echo a\necho b\n%s\necho c\n' "$frag" >"$T/f.sh"
    bo=$(cd "$T" && (ulimit -v 500000; timeout 5 "$BASH_BIN" --norc --noprofile f.sh) 2>&1 \
         | grep -v '^[abc]$' | head -2)
    ho=$(cd "$T" && (ulimit -v 500000; timeout 5 "$HUCK" f.sh) 2>&1 \
         | grep -v '^[abc]$' | head -2)
    b=$(extract "$bo"); h=$(extract "$ho")
    if [ "$b" = "$h" ]; then v=OK; ok=$((ok+1)); else v=DIFF; diff=$((diff+1)); fi
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$frag" "$b" "$h" "$v"
}

ROWS=$T/rows.tsv
if [ "$CHECK" = 1 ]; then exec 3>&1 1>"$ROWS"; fi

printf 'DEPTH\tCONTEXT\tMIDDLE\tINNER\tFRAGMENT\tBASH\tHUCK\tVERDICT\n'

for c in "${d1_contexts[@]}"; do
    for o in "${d1_openers[@]}"; do
        emit_cell 1 "$c" "-" "$o" "${CTX[$c]}${OPEN[$o]}"
    done
done

for o in "${d2_outers[@]}"; do
    base='echo '; [ "$o" = arraylit ] && base='v=('
    for m in "${d2_middles[@]}"; do
        for i in "${d2_inners[@]}"; do
            emit_cell 2 "$o" "$m" "$i" "${base}${OUTER[$o]}${MID[$m]}${INNER[$i]}"
        done
    done
done

printf 'eof_matrix: %d cells, %d OK, %d DIFF (huck: %s)\n' \
    "$((ok+diff))" "$ok" "$diff" "$HUCK" >&2

[ "$CHECK" = 1 ] || exit 0

# ── --check: what LEFT the expected DIFF set, and what JOINED it ──────────────
# Leaving is a fix; joining is a regression. A task's gate is "the rows I set out
# to fix left, and NOTHING joined".
exec 1>&3
now=$(awk -F'\t' 'NR>1 && $8=="DIFF"{printf "%s/%s/%s/%s\n", $1,$2,$3,$4}' "$ROWS" | sort)
expected=$(printf '%s\n' "$EXPECTED_DIFF" | grep -v '^[[:space:]]*$' | sort)

left=$(comm -23 <(printf '%s\n' "$expected") <(printf '%s\n' "$now"))
joined=$(comm -13 <(printf '%s\n' "$expected") <(printf '%s\n' "$now"))

nleft=$(printf '%s' "$left" | grep -c . || true)
njoined=$(printf '%s' "$joined" | grep -c . || true)

printf 'expected DIFF: %d   now DIFF: %d\n' \
    "$(printf '%s' "$expected" | grep -c .)" "$(printf '%s' "$now" | grep -c .)"
printf 'FIXED (left the DIFF set): %d\n' "$nleft"
[ "$nleft" -gt 0 ] && printf '%s\n' "$left" | sed 's/^/  + /'
printf 'REGRESSED (joined the DIFF set): %d\n' "$njoined"
[ "$njoined" -gt 0 ] && printf '%s\n' "$joined" | sed 's/^/  ! /'

# A regression fails the gate; fixes alone do not (the expected list is updated
# by the task that made them, so the edit is visible in review).
[ "$njoined" -eq 0 ]
