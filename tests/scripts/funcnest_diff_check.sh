#!/usr/bin/env bash
# v224 harness: $FUNCNEST enforcement messages + exit codes are byte-identical
# between bash 5.x and huck when run as a script file.
#
# Each fragment is written to a fixed temp-file name (./frag.sh) inside a temp
# directory, then run as:
#   bash ./frag.sh      (from that dir)
#   huck ./frag.sh      (from that dir)
# so $0 = "./frag.sh" identically for both shells and the "maximum function
# nesting level exceeded" prologue matches. Both STDOUT (rc=/n= lines) and
# STDERR (the error) are compared, plus the exit code.
#
# After the diff loop, a HUCK-ONLY backstop assertion verifies that an
# *unbounded* recursion (no FUNCNEST) yields a clean rc-1 error rather than a
# Rust stack-overflow SIGABRT (134). bash segfaults on that input, so it is not
# diffable against bash.
set -u

_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HUCK_BIN="${HUCK_BIN:-$_SCRIPT_DIR/../../target/release/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run: cargo build --release --bin huck" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "SKIP: bash not found on PATH; this differential harness requires bash" >&2
    exit 0
fi

PASS=0; FAIL=0

# Create a temp directory with a fixed script name so $0 is identical in both shells.
TMPDIR_HARNESS=$(mktemp -d)
trap 'rm -rf "$TMPDIR_HARNESS"' EXIT

fragments=(
  $'FUNCNEST=3\nn=0\nf(){ n=$((n+1)); f; }\nf\necho "rc=$? n=$n"'
  $'FUNCNEST=2\nf(){ f; }\nf\necho "rc=$?"'
  $'FUNCNEST=1\ng(){ g; }\nf(){ g; }\nf\necho "rc=$?"'
  $'FUNCNEST=0\nn=0\nf(){ n=$((n+1)); if (( n >= 30 )); then return 7; fi; f; }\nf\necho "rc=$? n=$n"'
)

for frag in "${fragments[@]}"; do
    printf '%s\n' "$frag" > "$TMPDIR_HARNESS/frag.sh"
    b_out=$(cd "$TMPDIR_HARNESS" && bash ./frag.sh 2>err.txt); b_rc=$?
    b_err=$(cat "$TMPDIR_HARNESS/err.txt")
    h_out=$(cd "$TMPDIR_HARNESS" && "$HUCK_BIN" ./frag.sh 2>err.txt); h_rc=$?
    h_err=$(cat "$TMPDIR_HARNESS/err.txt")
    if [[ "$b_out" == "$h_out" && "$b_err" == "$h_err" && "$b_rc" == "$h_rc" ]]; then
        printf 'PASS: %s\n' "${frag//$'\n'/ ; }"
        PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "${frag//$'\n'/ ; }"
        printf '    stdout:\n'; diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") | sed 's/^/      /'
        printf '    stderr:\n'; diff <(printf '%s\n' "$b_err") <(printf '%s\n' "$h_err") | sed 's/^/      /'
        printf '    rc: bash=%s huck=%s\n' "$b_rc" "$h_rc"
        FAIL=$((FAIL + 1))
    fi
done

# Backstop: unbounded recursion with no FUNCNEST must yield a clean error + rc 1,
# NOT a SIGABRT (134). bash segfaults here, so this is huck-only. Capture rc
# directly (no pipe — a pipe subshell would lose the FAIL increment).
bs=$(mktemp -d); printf 'f(){ f; }\nf\n' > "$bs/frag.sh"
( cd "$bs" && "$HUCK_BIN" ./frag.sh >/dev/null 2>err.txt ); rc=$?
if [[ "$rc" == 1 ]] && grep -q "maximum function nesting level exceeded" "$bs/err.txt"; then
  echo "PASS: backstop (clean error, no SIGABRT)"; PASS=$((PASS+1))
else
  echo "FAIL: backstop rc=$rc (expected 1, no abort)"; FAIL=$((FAIL+1))
fi
rm -rf "$bs"

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
