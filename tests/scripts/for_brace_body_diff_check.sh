#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for the ksh-derived `{ … }` brace
# group used in place of `do … done` as a `for`/`select` loop body. bash
# accepts a brace body on `for` (both C-style and word-list) and `select`,
# but NOT on `while`/`until`. Each fragment runs through `bash` and `huck`
# via stdin (huck has no -c flag); outputs must be byte-identical. The
# final NEGATIVE case only asserts that both shells reject the fragment
# (their error wording differs, so we compare exit-nonzero, not text).

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(printf '%s\n' "$fragment" | bash 2>&1; echo "EXIT:$?")
    huck_out=$(printf '%s\n' "$fragment" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# Both shells must REJECT the fragment (exit non-zero). Error text differs
# between bash and huck, so only the reject/accept decision is compared.
check_both_error() {
    local label="$1"
    local fragment="$2"
    local bash_rc huck_rc

    printf '%s\n' "$fragment" | bash >/dev/null 2>&1; bash_rc=$?
    printf '%s\n' "$fragment" | "$HUCK_BIN" >/dev/null 2>&1; huck_rc=$?

    if [[ "$bash_rc" -ne 0 && "$huck_rc" -ne 0 ]]; then
        printf "PASS: %s (bash rc=%s, huck rc=%s)\n" "$label" "$bash_rc" "$huck_rc"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s (bash rc=%s, huck rc=%s — expected both non-zero)\n" \
               "$label" "$bash_rc" "$huck_rc"
        FAIL=$((FAIL + 1))
    fi
}

# 1. C-style for with brace body (only a Blank before `{`, no `;`).
check "c-style for brace body" \
      'for ((i=0;i<3;i++)) { echo $i; }'

# 2. Word-list for with brace body (after a `;`).
check "word-list for brace body" \
      'for x in a b c; { echo $x; }'

# 3. Newline before the brace.
check "for brace body newline" \
      'for x in a b
{ echo $x; }'

# 4. Multi-statement brace body.
check "for brace body multi-statement" \
      'for x in a b; { echo start $x; echo end $x; }'

# 5. Nested brace-body for loops.
check "nested for brace bodies" \
      'for x in 1 2; { for y in a b; { echo $x$y; } }'

# 6. break inside a brace body.
check "for brace body break" \
      'for x in 1 2 3 4; { [ $x = 3 ] && break; echo $x; }'

# 7. continue inside a brace body.
check "for brace body continue" \
      'for x in 1 2 3; { [ $x = 2 ] && continue; echo $x; }'

# 8. Redirect on the whole loop (attaches to the loop, not the brace group).
check "for brace body redirect on loop" \
      'for x in a b; { echo $x; } > /tmp/hk_forbrace.$$ ; cat /tmp/hk_forbrace.$$ ; rm -f /tmp/hk_forbrace.$$'

# 9. C-style empty condition section with `; do` (regression: the `)) ; do`
#    separator must be skippable past its leading Blank).
check "c-style empty-cond ; do" \
      'for ((i=4;;i--)) ; do echo $i; if (( i == 0 )); then break; fi; done'

# 10. select with brace body (feed one choice on stdin).
check "select brace body" \
      'printf "1\n" | select x in a b; { echo "got=$x"; break; }'

# 11. do/done forms still work unchanged (with and without `;` before do).
check "for do/done still works" \
      'for ((i=0;i<2;i++)) do echo $i; done'

# 12. NEGATIVE: while does NOT accept a brace body — both shells must error.
check_both_error "while brace body rejected" \
      'while false; { echo hi; }'

# 13. NEGATIVE: until does NOT accept a brace body either.
check_both_error "until brace body rejected" \
      'until true; { echo hi; }'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
