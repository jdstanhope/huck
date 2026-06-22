#!/usr/bin/env bash
# Drives the huck Engine + bash on the same fragments and asserts byte-identical
# (stdout, stderr, exit_code) (or merged-stdout + exit_code for merged mode).
#
# Requires: bash 5+, /bin/sh in PATH, the huck workspace built (`cargo build`).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --example engine_capture_diff -p huck-engine >/dev/null 2>&1
DRIVER=target/debug/examples/engine_capture_diff
if [ ! -x "$DRIVER" ]; then
    echo "FAIL: could not locate engine_capture_diff driver at $DRIVER" >&2
    exit 1
fi

run_huck() { "$DRIVER" "$1" "$2"; }

run_bash_split() {
    local frag=$1
    local out_file err_file exit_code
    out_file=$(mktemp)
    err_file=$(mktemp)
    bash -c "$frag" >"$out_file" 2>"$err_file"
    exit_code=$?
    local out_bytes err_bytes
    out_bytes=$(wc -c <"$out_file")
    err_bytes=$(wc -c <"$err_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:%s\n' "$err_bytes"
    cat "$err_file"
    printf 'EXIT:%s\n' "$exit_code"
    rm -f "$out_file" "$err_file"
}

run_bash_merged() {
    local frag=$1
    local out_file exit_code
    out_file=$(mktemp)
    bash -c "$frag" >"$out_file" 2>&1
    exit_code=$?
    local out_bytes
    out_bytes=$(wc -c <"$out_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:0\n'
    printf 'EXIT:%s\n' "$exit_code"
    rm -f "$out_file"
}

FAIL=0
check() {
    local label=$1 mode=$2 frag=$3
    local huck_out bash_out
    huck_out=$(run_huck "$mode" "$frag")
    if [ "$mode" = "merged" ]; then
        bash_out=$(run_bash_merged "$frag")
    else
        bash_out=$(run_bash_split "$frag")
    fi
    if [ "$huck_out" != "$bash_out" ]; then
        echo "FAIL [$label] mode=$mode"
        diff <(printf '%s' "$huck_out") <(printf '%s' "$bash_out") || true
        FAIL=1
    else
        echo "PASS [$label] mode=$mode"
    fi
}

# Builtin-only fragments
check 'echo-only'        split  'echo hi'
check 'echo-and-err'     split  'echo hi; echo err >&2'
check 'echo-and-err'     merged 'echo hi; echo err >&2'
check 'exit-status'      split  'exit 5'

# External-process fragments
check 'sh-mixed'         split  '/bin/sh -c "echo out; echo err >&2"'
check 'sh-mixed'         merged '/bin/sh -c "echo out; echo err >&2; echo out2"'

# Pipeline fragments
check 'pipeline'         split  'echo hi | cat'
check 'pipeline-err'     split  '/bin/sh -c "echo err >&2" | cat'

# Redirect fragments
check 'redirect-2to1'    split  'echo hi 2>&1'
check 'redirect-err-to-out' merged 'echo err >&2; echo out'

if [ $FAIL -ne 0 ]; then
    echo "engine_capture_diff_check FAILED" >&2
    exit 1
fi
echo "engine_capture_diff_check OK"
