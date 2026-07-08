#!/usr/bin/env bash
# Byte-identical bash<->huck harness for G3: the `==`/`!=`/`=` RHS pattern
# inside `[[ … ]]` is ALWAYS an extended (extglob) pattern in bash — an
# `@(a|b)`/`!(x)`-shaped group matches as extglob REGARDLESS of `shopt extglob`.
# Every fragment is run with extglob EXPLICITLY OFF (`shopt -u extglob`) and,
# for a control, a paired run with it ON — the results must be identical to
# bash in BOTH cases. Also guards grouping / quoted-paren / pattern-vs-literal.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Run `frag` under an explicit shopt state (`-u` off / `-s` on), set on its OWN
# prior line (huck tokenizes a whole logical line before executing it), and
# assert bash and huck agree byte-for-byte incl. exit status.
check() {
    local label="$1" shopt_state="$2" frag="$3" b h
    b=$(printf 'shopt -%s extglob\n%s\n' "$shopt_state" "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf 'shopt -%s extglob\n%s\n' "$shopt_state" "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Same fragment, extglob OFF and ON — both must match bash.
both() { check "$1 [off]" u "$2"; check "$1 [on]" s "$2"; }

# ── all 5 prefixes, `==`, extglob OFF and ON ────────────────────────────────
both "@ match"     '[[ record == @(record|top) ]] && echo y || echo n'
both "@ nomatch"   '[[ nope == @(record|top) ]] && echo y || echo n'
both "+ match"     '[[ aab == +(a|b) ]] && echo y || echo n'
both "* match"     '[[ abbbc == a*(b)c ]] && echo y || echo n'
both "* empty"     '[[ ac == a*(b)c ]] && echo y || echo n'
both "? match"     '[[ ab == a?(b) ]] && echo y || echo n'
both "? empty"     '[[ a == a?(b) ]] && echo y || echo n'
both "! neg y"     '[[ foo == !(bar) ]] && echo y || echo n'
both "! neg n"     '[[ bar == !(bar) ]] && echo y || echo n'

# ── `!=` operator (negated) ─────────────────────────────────────────────────
both "!= match"    '[[ x != @(a|b) ]] && echo y || echo n'
both "!= nomatch"  '[[ a != @(a|b) ]] && echo y || echo n'
both "!= neg grp"  '[[ foo != !(bar) ]] && echo y || echo n'

# ── the `=` spelling of `==` ────────────────────────────────────────────────
both "= match"     '[[ ab == @(ab|cd) ]] && echo y || echo n'

# ── alternation, glued prefix/suffix, class inside group ────────────────────
both "alt 3-way"   '[[ cd == @(ab|cd|ef) ]] && echo y || echo n'
both "glued mid"   '[[ axc == a@(x|y)c ]] && echo y || echo n'
both "class grp"   '[[ file.txt == +([a-z]).txt ]] && echo y || echo n'

# ── nesting (bash-valid: a prefixed inner group) ────────────────────────────
both "nest match"  '[[ abbbc == @(a*(b)c) ]] && echo y || echo n'
both "nest alt"    '[[ ac == @(a?(b)c|z) ]] && echo y || echo n'

# ── quoted alternation bar is literal, and the RHS is a PATTERN not literal ──
both "quoted bar lit"   '[[ "a|b" == @("a|b") ]] && echo y || echo n'
both "quoted bar nomat" '[[ a == @("a|b") ]] && echo y || echo n'
both "rhs is pattern"   '[[ "@(record|top)" == @(record|top) ]] && echo y || echo n'

# ── real-world shape from perf-completion.sh (quoted alternatives) ──────────
both "perf quoted alts" 'prev=-e; [[ $prev == @("-e"|"--event") ]] && echo y || echo n'
both "perf plain alts"  'sub=record; [[ $sub == @(record|stat|top) ]] && echo y || echo n'

# ── grouping / literal-paren guards MUST still parse (no extglob trigger) ────
both "grouping paren"   '[[ (a == a) ]] && echo y || echo n'
both "quoted lparen"    'x=y; [[ $x == "(" ]] && echo y || echo n'
both "lparen match"     '[[ "(" == "(" ]] && echo y || echo n'
both "bare star glob"   '[[ anything == * ]] && echo y || echo n'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
