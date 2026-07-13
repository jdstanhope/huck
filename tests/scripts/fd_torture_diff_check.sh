#!/usr/bin/env bash
# fd-plumbing remediation regression net (review: docs/superpowers/reviews/
# 2026-07-13-engine-fd-plumbing-review.md). Concentrated fd/redirect/pipeline/
# background matrix, restricted to behavior huck ALREADY matches bash on, so it is
# green today and its job is to catch REGRESSIONS as Phases 1-5 rework the fd
# machinery. #128 (no-hangup) and #129 (bg stdin) cases are added by their tasks.
# Deliberately excluded until their fixing phase: `exec <&-; cat < file | cat`
# (#132) and stage redirect source-order (#50).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'FA\n' > "$WORK/inA"

norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }

# Byte-identical bash vs huck, both timeout-wrapped so a regression that
# reintroduces a hang FAILs instead of wedging. cwd is $WORK for relative paths.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && timeout 5 bash        -c "$frag" </dev/null 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && timeout 5 "$HUCK_BIN" -c "$frag" </dev/null 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    b=${b//$WORK/@W@}; h=${h//$WORK/@W@}
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- freed std fds x pipelines (post-v288 correct) ---
check "freed fd0: cat|cat"        'exec <&-; cat | cat; echo end'
check "freed fd0: cat|cat|cat"    'exec <&-; cat | cat | cat; echo end'
check "freed fd2: err|cat"        'exec 2>&-; ls /no_such_xyz | cat; echo "end=$?"'
# --- per-stage redirects on a non-last stage ---
check "stage stdout redirect"     'echo hi > f | cat; cat f'
check "stage stderr redirect"     'ls /no_such_xyz 2>e | cat; cat e'
check "fd>2 dup"                  'exec 3>f; echo x >&3; exec 3>&-; cat f'
# --- heredoc / here-string into a stage ---
check "heredoc into stage"        $'cat <<EOF | cat\nh1\nh2\nEOF'
check "herestring into stage"     'cat <<< "hs" | cat'
# --- subshell / group redirects ---
check "subshell redirect"         '(echo hi) 2>&1 | cat'
check "group redirect"            '{ echo hi; } > f; cat f'
# --- dup / close / merge ---
check "1>&2 to stderr"            'echo hi 1>&2 2>/dev/null; echo done'
check "2>&1 merge into pipe"      'ls /no_such_xyz 2>&1 | cat'
check "&> merge to file"          'echo hi &> f; cat f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
