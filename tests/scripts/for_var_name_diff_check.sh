#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v180: a `for`-loop variable name is
# accepted as any single word at PARSE time and identifier-validated at RUNTIME.
# Reserved words (`if`, `in`) are valid identifiers and run; non-identifiers
# (`a-b`, `1x`) produce a NON-FATAL "not a valid identifier" error (status 1,
# body not run, the surrounding list continues). The error WORDING differs by
# the intentional prefix convention (`huck:` vs `bash: line N:`), so the
# non-identifier cases compare stdout+exit only (stderr discarded).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Compare stdout + exit code (stderr discarded) — for cases whose only stderr is
# the intentionally-differing error prefix.
check_out() {  # label ; fragment
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Keyword loop vars run (valid identifiers).
check_out "keyword 'if' as loop var"   'for if in 1 2; do echo $if; done'
check_out "keyword 'in' as loop var"   'for in in a b; do echo $in; done'
check_out "keyword loop var, used in body" 'for if in x; do echo got-$if; done'

# Non-identifier names: non-fatal, body skipped, surrounding list continues.
check_out "hyphen name, list continues" 'for a-b in 1; do echo body; done; echo after'
check_out "hyphen name bare (rc 1)"     'for a-b in 1; do echo body; done'
check_out "leading-digit name"          'for 1x in 1; do echo body; done; echo after'
check_out "dotted name"                 'for a.b in 1 2; do echo body; done; echo after'

# Valid loops unchanged.
check_out "valid in-list"               'for x in a b c; do echo v-$x; done'
check_out "valid no-in (positionals)"   'set -- p q; for x; do echo arg-$x; done'
check_out "valid empty in-list"         'for x in; do echo never; done; echo done-empty'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
