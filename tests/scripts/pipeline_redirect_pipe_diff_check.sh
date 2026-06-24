#!/usr/bin/env bash
# v212: bash-diff harness for the M-125 fix. A non-final pipeline stage
# with explicit stdout redirect must give the downstream stage an EOF
# inter-stage pipe (not the parent's stdin).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --workspace --bin huck >/dev/null 2>&1
HUCK=target/debug/huck
if [ ! -x "$HUCK" ]; then
    echo "FAIL: huck binary not found at $HUCK" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local b h
    # Each fragment carries its own `printf '...' |` stdin feed inside
    # the fragment string. We pass the fragment to `bash -c` / `huck -c`
    # with the same parent stdin (the harness's own stdin), so the
    # fragment's piped stdin is what each shell actually sees.
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK" -c "$frag" 2>&1)
    if [ "$b" != "$h" ]; then
        echo "FAIL [$label]"
        echo "  bash: $b"
        echo "  huck: $h"
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# Use a stable temp filename per run.
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

# === The bug fix: non-final stdout redirect now produces EOF for next stage ===
check 'stdout-trunc-non-final-eof'  "printf 'X' | { echo up > $T/o | cat; }"
check 'stdout-append-non-final-eof' "printf 'X' | { echo up >> $T/o | cat; }"
check 'stdout-redir-3-stage'        "printf 'X' | { echo a > $T/o | echo b | cat; }"

# === No-bug regression guards: these paths already worked, must keep working ===
check 'stderr-only-redir-no-bug'    "printf 'X' | { echo up 2> $T/e | cat; }"
# >&2 routes upstream stdout to stderr; both shells should produce no stdout
# (the "up" goes to stderr, which we drop). The downstream stage gets the
# inter-stage pipe (already created in the normal path) and reads EOF.
check 'dup-redirect-no-bug'         "printf 'X' | { echo up >&2 | cat; } 2>/dev/null"
check 'final-stage-redir-no-bug'    "printf 'X' | { cat | tee $T/o >/dev/null; cat $T/o; }"

# === Redirect-open failure: error path, no fd leak ===
# bash and huck use different exact error wording; normalize by extracting
# just the "No such file" portion which is libc-uniform.
check 'redir-failure-still-skips'   "printf 'X' | { echo up >/no/such/dir/m125 | cat; } 2>&1 | grep -oE 'No such file or directory' | head -1"

if [ $FAIL -ne 0 ]; then
    echo "pipeline_redirect_pipe_diff_check FAILED" >&2
    exit 1
fi
echo "pipeline_redirect_pipe_diff_check OK"
