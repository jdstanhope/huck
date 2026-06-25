#!/usr/bin/env bash
# v216 harness: arithmetic expansion error messages are byte-identical between
# bash 5.x and huck when run as a script file.
#
# Each fragment is written to a fixed temp-file name (./frag.sh) inside a temp
# directory, then run as:
#   bash ./frag.sh      (from that dir)
#   huck ./frag.sh      (from that dir)
# so $0 = "./frag.sh" identically for both shells and the error prologues match.
# Only STDERR is compared; stdout may differ or be suppressed on error.
#
# NOTE: Standalone "(( ... ))" fragments are deliberately excluded. huck's
# Command::Arith AST variant carries no source line, so standalone (( )) errors
# omit bash's "line N:" segment in script mode. That pre-existing gap
# (Command::Arith LINENO stamping) is out of v216 scope and is documented as a
# deferred divergence in docs/bash-divergences.md.
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
  'echo $(( 7 = 43 ))'
  'echo $(( 44 / 0 ))'
  'echo $(( 2#44 ))'
  'echo $(( 3425#56 ))'
  'echo $(( 2# ))'
  'echo $(( 4 ? : 3 + 5 ))'
  'echo $(( a b ))'
  'let "rv = 7 + (43 * 6"'
  'echo $(( 1 ? 20 : x += 2 ))'
  'echo $(( 0 && B = 42 ))'
)

for frag in "${fragments[@]}"; do
    printf '%s\n' "$frag" > "$TMPDIR_HARNESS/frag.sh"
    # Capture stderr only: 2>&1 >/dev/null inside $() routes stderr to the pipe,
    # stdout to /dev/null.
    b_err=$(cd "$TMPDIR_HARNESS" && bash ./frag.sh 2>&1 >/dev/null)
    h_err=$(cd "$TMPDIR_HARNESS" && "$HUCK_BIN" ./frag.sh 2>&1 >/dev/null)
    if [[ "$b_err" == "$h_err" ]]; then
        printf 'PASS: %s\n' "$frag"
        PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$frag"
        diff <(printf '%s\n' "$b_err") <(printf '%s\n' "$h_err") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
