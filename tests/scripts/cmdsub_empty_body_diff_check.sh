#!/usr/bin/env bash
# Byte-identical bash<->huck harness for G2: a command-substitution body that
# is only whitespace/newlines (`$( )`, `$(   )`, `$(\t)`, `$(\n)`) must parse
# as an EMPTY command substitution — same as truly-empty `$()` — not a
# syntax error. Covers `$( )` variants, the `x=$( )"a"$( )"b"` glue idiom,
# process substitution's analogous empty-body case, and confirms a body with
# a real command still runs.
#
# `check` pipes a single-LINE fragment through stdin (matching the other
# cmdsub_*_diff_check.sh harnesses). Newline-only-body variants are NOT run
# through the stdin/REPL path here: piped multi-line command substitutions
# hit a separate, already-tracked, pre-existing REPL-continuation gap
# (see docs memory "cmdsub comment-only body EOF bug" /
# cmdsub_comment_diff_check.sh's 1/8 pre-existing failure) that is unrelated
# to G2 and out of scope for this fix. `check_file` instead runs a multi-line
# script as a FILE argument (bash's and huck's ordinary script-file readers,
# not the incremental line-by-line REPL reader) to exercise the `$(\n)` /
# `$(\n\t\n)` newline-body cases byte-identically.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TAB=$'\t'  # a literal tab char, built OUTSIDE any double-quoted frag string
           # ($'\t' is not interpreted when it appears inside "…")

check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check_file() {
    local label="$1" script="$2" b h tmp
    tmp=$(mktemp)
    printf '%s\n' "$script" > "$tmp"
    b=$(bash --norc --noprofile "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# ── truly-empty body: unchanged ──────────────────────────────────────────
check "empty body unchanged"        'echo $()end'

# ── whitespace-only body, single space/tab runs (single physical line) ──
check "single space body"           'echo $( )end'
check "multi space body"            'echo $(   )end'
check "tab body"                    "echo \$( ${TAB} )end"

# ── the real-world adjacency idiom ───────────────────────────────────────
check "adjacency glue"              'x=$( )"a"$( )"b"; echo "$x"'
check "adjacency glue tab"          "x=\$(${TAB})\"a\"\$(${TAB})\"b\"; echo \"\$x\""

# ── a real command still runs, surrounded by blanks ──────────────────────
check "real command, spaced body"   'echo $( echo hi )end'

# ── process substitution: analogous empty-body rule ──────────────────────
check "procsub empty body"          'cat <( )'
check "procsub tab body"            "cat <(${TAB})"

# ── newline/newline+tab-only bodies: script-FILE path (not piped stdin) ──
check_file "newline-only body" 'echo $(
)end'
check_file "newline+tab-only body" 'echo $(
	)end'
check_file "adjacency glue, newline body" 'x=$(
)"a"$(
)"b"
echo "$x"'
check_file "real command, newline-wrapped body" 'echo $(
echo hi
)end'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
