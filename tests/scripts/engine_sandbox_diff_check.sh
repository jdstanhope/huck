#!/usr/bin/env bash
# Bash-diff harness for the engine sandbox knobs.
# Compares huck Engine (via the engine_sandbox_diff example binary) against
# bash on the same fragments. For restricted-mode fragments uses
# `bash --restricted -c '…'`.
#
# Requires: bash 5+, the huck workspace built (`cargo build`).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --example engine_sandbox_diff -p huck-engine >/dev/null 2>&1
DRIVER=target/debug/examples/engine_sandbox_diff
if [ ! -x "$DRIVER" ]; then
    echo "FAIL: could not locate engine_sandbox_diff driver at $DRIVER" >&2
    exit 1
fi

# Output capture helpers — mirror the engine_capture_diff protocol.
emit_capture() {
    local out_file=$1 err_file=$2 exit_code=$3
    local out_bytes err_bytes
    out_bytes=$(wc -c <"$out_file")
    err_bytes=$(wc -c <"$err_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:%s\n' "$err_bytes"
    cat "$err_file"
    printf 'EXIT:%s\n' "$exit_code"
}

run_bash() {
    local flags=$1 frag=$2
    local out_file err_file exit_code
    out_file=$(mktemp)
    err_file=$(mktemp)
    # shellcheck disable=SC2086
    bash $flags -c "$frag" >"$out_file" 2>"$err_file"
    exit_code=$?
    emit_capture "$out_file" "$err_file" "$exit_code"
    rm -f "$out_file" "$err_file"
}

FAIL=0
check() {
    local label=$1 huck_mode=$2 bash_flags=$3 frag=$4
    local huck_out bash_out
    huck_out=$("$DRIVER" "$huck_mode" "$frag")
    bash_out=$(run_bash "$bash_flags" "$frag")
    if [ "$huck_out" != "$bash_out" ]; then
        echo "FAIL [$label]"
        diff <(printf '%s' "$huck_out") <(printf '%s' "$bash_out") || true
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# Bare fragments (sanity — exercise the driver itself).
check 'bare-echo'              bare       ''         'echo hi'

# Restricted refusals: bash and huck differ on the program-name-prefix in
# the diagnostic ('bash: restricted: cd' vs 'huck: restricted: cd'), so we
# compare ONLY the exit code (and verify both reject by exiting nonzero).
check_exit_only() {
    local label=$1 huck_mode=$2 bash_flags=$3 frag=$4
    local huck_code bash_code
    huck_code=$("$DRIVER" "$huck_mode" "$frag" | sed -n 's/^EXIT://p')
    # shellcheck disable=SC2086
    bash $bash_flags -c "$frag" >/dev/null 2>&1
    bash_code=$?
    if [ "$huck_code" != "$bash_code" ]; then
        echo "FAIL [$label] huck_exit=$huck_code bash_exit=$bash_code"
        FAIL=1
    else
        echo "PASS [$label] exit=$huck_code"
    fi
}

# Restricted: cd refused (both should exit nonzero).
check_exit_only 'r-cd'            restricted --restricted   'cd /tmp'
# Restricted: exec refused.
check_exit_only 'r-exec'          restricted --restricted   'exec /bin/true'
# Restricted: slash command refused.
check_exit_only 'r-slash-cmd'     restricted --restricted   '/bin/echo hi'
# Restricted: bare command works under restricted.
check 'r-bare-true' restricted --restricted  'true; echo ok'
# Restricted: source with slash refused.
check_exit_only 'r-source-slash'  restricted --restricted   '. /etc/profile'

# CWD fragment: bash equivalent uses `cd $tmp` prefix.
TMP=$(mktemp -d)
check_exit_only 'cwd-pwd'         "cwd:$TMP"  ''         "cd $TMP; pwd"

if [ -d "$TMP" ]; then rm -rf "$TMP"; fi

if [ $FAIL -ne 0 ]; then
    echo "engine_sandbox_diff_check FAILED" >&2
    exit 1
fi
echo "engine_sandbox_diff_check OK"
