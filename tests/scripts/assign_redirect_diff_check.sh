#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the assignment/redirect-only command
# (an "ExecCommand" with an empty program word, e.g. `VAR=val 2>err`). bash
# applies the assignments to the CURRENT shell and performs the redirections for
# their side effects only — it is NOT "command not found". huck previously
# emitted `command not found:` and dropped the assignment; this guards the fix.
#
# Key bash semantics exercised here:
#   - the assignment persists despite the attached redirect;
#   - the RHS command substitution runs with the ORIGINAL fds (the command's own
#     `2>…` does not capture it), and supplies the command's exit status;
#   - `>f` truncation still happens (redirect performed for its side effect);
#   - a failed redirect open still leaves the assignment applied, status reflects
#     the failure.
#
# NOT covered (known, separate divergences — keep out of a byte-compare):
#   - readonly-assignment error text ("huck:" vs "bash: line N:" prefix) and
#     bash aborting a non-interactive shell on a readonly assignment;
#   - redirect-open error routing/ordering (huck reports to the real stderr
#     regardless of an earlier `2>/dev/null` — also true for normal commands);
#   - fd>2 redirects (`3>…`), unsupported by huck's command AST.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "scalar + 2>&1 applies"      'x=1 2>&1; echo "x=$x"'
check "scalar + 2>/dev/null"       'x=hi 2>/dev/null; echo "$x"'
check "array elem + redirect"      'a[0]=9 2>/dev/null; echo "${a[0]}"'
check "RHS cmdsub value"           'x=$(echo body) 2>&1; echo "$x"'
check "RHS cmdsub uses orig fds"   'x=$(echo oops >&2) 2>/dev/null; echo "x=[$x]"'
check "status from RHS cmdsub"     'x=$(exit 3) 2>/dev/null; echo $?'
check "status 0 plain"             'x=1 2>/dev/null; echo $?'
check "two assigns then real cmd"  'v=1 2>&1; w=2; echo "$v $w"'
check "redirect truncates file"    'f=$(mktemp); echo junk > "$f"; v=1 > "$f"; echo "[$(cat "$f")]"; echo "v=$v"; rm -f "$f"'
check "redirect appends to file"   'f=$(mktemp); printf head > "$f"; v=1 >> "$f"; printf tail >> "$f"; echo "[$(cat "$f")]"; rm -f "$f"'
check "assign visible to later cmd" 'PATHX=/x 2>/dev/null; echo "$PATHX"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
