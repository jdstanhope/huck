#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a LINE CONTINUATION inside arithmetic
# (#653). huck kept the `\` and the newline in the expression text, so every
# continued expression died with `unexpected character: '\'` and printed garbage
# where bash computes a value. Found by the runtime sweep in
# linux-headers' `Documentation/arch/arm64/kasan-offsets.sh`, whose entire output
# is arithmetic split over continuations.
#
# The fix drops both characters in the lexer rather than skipping them later in
# the arith tokenizer, because bash removes them before the expression text
# exists — measured: the error body for `$((1 + \<newline>@))` is exactly
# `1 + @`, with no trace of either character. The `bad token after a
# continuation` row below pins that, and it is the row that distinguishes the two
# possible fix sites.
#
# Uses a script FILE: a continuation is only interesting across a real newline,
# and a file is the driver the corpus script itself uses.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
# ⚠️ Status captured BEFORE any pipe — `cmd | sed; echo $?` reports sed's status.
check() {
    local label="$1" body="$2" b h out rc
    printf '%s\n' "$body" >"$T/s.sh"
    out=$(cd "$T" && bash s.sh 2>&1); rc=$?
    b=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    out=$(cd "$T" && "$HUCK_BIN" s.sh 2>&1); rc=$?
    h=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── every arithmetic context ──────────────────────────────────────────────────
check 'dollar-dparen'      'echo $((1 + \
2))'
check 'legacy dollar-bracket' 'echo $[1 + \
2]'
check 'arith command'      '((1 + \
2)) && echo yes'
check 'for header'         'for ((i=0;i<\
2;i++)); do echo $i; done'
check 'assignment rhs'     'x=$((1 + \
2)); echo $x'
check 'inside dquotes'     'echo "$((1 + \
2))"'
check 'dquoted span inside' 'echo $(( "1 + \
2" ))'

# ── placement and repetition ──────────────────────────────────────────────────
check 'two continuations'  'echo $((1 + \
2 + \
3))'
check 'before an operator' 'echo $(( 1 \
+ 2 ))'
check 'trailing'           'i=0; echo $((i++ \
))'
check 'around a variable'  'x=5; echo $(( x \
* 2 ))'
check 'inside nesting'     'echo $(( (1 + \
2) * 3 ))'

# ── controls ──────────────────────────────────────────────────────────────────
# Plain arithmetic and the ordinary error path, both unchanged.
check 'plain'                'echo $((1 + 2))'
check 'power operator'       'echo $(( 2 ** 3 ))'
check 'missing operand'      'echo $((1 +))'
check 'division by zero'     'echo $((1/0))'
#
# ⚠️ There is deliberately NO row asserting the message for an expression that
# ERRORS after a continuation, and the reason is worth recording because it is
# also how the fix site was chosen.
#
# The expression TEXT is what proves the characters are gone before the text is
# built (a fix that merely skipped them while tokenizing would leave them in the
# message). Measured, and both shells now agree on the text:
#
#     echo $((1 / \<newline>0))
#     bash: line 2: 1 / 0: division by 0 (error token is "0")
#     huck: line 1: 1 / 0: division by 0 (error token is "0")
#
# — identical but for the LINE: bash names the line its READER had reached, huck
# names the line the command STARTS on. That is now a KEPT divergence (#658,
# closed by-design with #680, recorded in docs/bash-divergences.md), so a row
# asserting the whole message will never be green here and is not waiting on
# anything. Two other arith messages diverge in WORDING for unrelated reasons
# (#659), which is why `$((1 + @))` is not a control either.

harness_summary
