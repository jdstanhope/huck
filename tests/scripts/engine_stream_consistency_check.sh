#!/usr/bin/env bash
# Self-consistency harness for v207 streaming.
#
# No bash equivalent exists for the streaming callback API, so this harness
# verifies the INTERNAL property: running a fragment through `.capture()` (the
# v205 path) and through `.on_stdout_line(cb).capture()` (the v207 streaming
# path) must produce byte-identical transcripts.
#
# Requires: bash 5+, the huck workspace built (`cargo build`).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --example engine_stream_diff -p huck-engine >/dev/null 2>&1
DRIVER=target/debug/examples/engine_stream_diff
if [ ! -x "$DRIVER" ]; then
    echo "FAIL: driver not found at $DRIVER" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local cap stream
    cap=$("$DRIVER" cap "$frag")
    stream=$("$DRIVER" stream "$frag")
    if [ "$cap" != "$stream" ]; then
        echo "FAIL [$label]"
        diff <(printf '%s' "$cap") <(printf '%s' "$stream") || true
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# All fragments end with \n (via the final `echo`/loop body) so the streaming
# partial-line-at-EOF edge doesn't trip the comparison.
check 'builtin-only'    'echo a; echo b; echo c'
check 'external-only'   '/bin/sh -c "echo x; echo y"'
check 'mixed'           'echo bi; /bin/sh -c "echo ext"; echo bo'
check 'pipeline'        'echo hi | tr a-z A-Z'
check 'redirect-2to1'   'echo a; echo b 2>&1'
# `$(seq 1 50)` exercises an inner-capture cmdsub — its output must NOT leak
# into the streaming callbacks (only `line-$i` from the for-body should).
check 'long-output'     'for i in $(seq 1 50); do echo line-$i; done'
# Regression guard for the v207 task-8 fixup: the executor must suspend
# callbacks across $(…) so hidden cmdsub output never reaches on_stdout_line.
check 'cmdsub-no-leak'  'x=$(echo hidden); echo visible'

if [ $FAIL -ne 0 ]; then
    echo "engine_stream_consistency_check FAILED" >&2
    exit 1
fi
echo "engine_stream_consistency_check OK"
