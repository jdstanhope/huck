#!/usr/bin/env bash
# v288 (#130): a pipeline pipe end must never reuse a freed std fd (0/1/2). After
# `exec <&-` frees fd 0, huck's make_pipe() used to hand the pipe read end fd 0,
# aliasing the first stage's stdin onto the pipe -> the stage read its own output
# and hung. bash keeps pipe fds >= 3, so its first stage inherits the closed fd 0
# and errors immediately. We compare EXTERNAL-command pipelines (byte-identical
# program messages) and add a functional no-hang check for the `read` repro (whose
# builtin error WORDING differs from bash for unrelated reasons — out of scope).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT

# Strip the shell's program-name/line prefix so only command output is compared.
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }

# Byte-identical bash vs huck. Both are wrapped in `timeout` so a pre-fix hang
# surfaces as a FAIL (rc 124) instead of hanging the whole harness.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && timeout 5 bash        -c "$frag" </dev/null 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && timeout 5 "$HUCK_BIN" -c "$frag" </dev/null 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "closed fd0: cat | cat"        'exec <&-; cat | cat; echo end'
check "closed fd0: cat | grep"       'exec <&-; cat | grep nomatch; echo "end=$?"'
check "baseline (no close) a | b"    'printf "hi\n" | cat; echo end'

# Functional no-hang check for the #130 repro. The `read` builtin's error wording
# differs from bash (out of scope), so compare only: huck did NOT hang (rc != 124)
# and huck's exit status equals bash's.
nohang() {
    local label="$1" frag="$2" brc hrc
    timeout 5 bash        -c "$frag" </dev/null >/dev/null 2>&1; brc=$?
    timeout 5 "$HUCK_BIN" -c "$frag" </dev/null >/dev/null 2>&1; hrc=$?
    if [[ "$hrc" != 124 && "$hrc" == "$brc" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash rc=%s huck rc=%s; 124=hang)\n' "$label" "$brc" "$hrc"; FAIL=$((FAIL+1)); fi
}
nohang "closed fd0: read | cat no-hang" 'exec <&-; read x | cat; echo end'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
