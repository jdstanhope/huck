#!/usr/bin/env bash
# v214: bash test-suite runner — runs bash's own tests/run-* against huck.
# Reads bash source from $BASH_SOURCE_DIR (operator-supplied; NOT vendored).
# Emits a Markdown summary to stdout; per-category logs land in a /tmp scratch.
#
# Env vars:
#   BASH_SOURCE_DIR             (required) path to extracted bash-5.2.21/ source
#   HUCK_BASH_TEST_CATEGORY     (optional) single category name to run; default: all
#   HUCK_TEST_TIMEOUT           (optional) per-category timeout in seconds; default: 30
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SKIPS_FILE="$ROOT/tests/bash-test-suite/known-skips.txt"
TIMEOUT=${HUCK_TEST_TIMEOUT:-30}

# ---- Preflight -----------------------------------------------------

if [ -z "${BASH_SOURCE_DIR:-}" ]; then
    cat >&2 <<EOF
error: \$BASH_SOURCE_DIR is not set.

This harness reads upstream bash's test files from \$BASH_SOURCE_DIR/tests/
at runtime; nothing is vendored into this repo (GPL licensing posture).
See $ROOT/tests/bash-test-suite/README.md for setup instructions.
EOF
    exit 1
fi

if [ ! -f "$BASH_SOURCE_DIR/tests/run-arith" ]; then
    echo "error: \$BASH_SOURCE_DIR/tests/run-arith not found." >&2
    echo "  BASH_SOURCE_DIR=$BASH_SOURCE_DIR doesn't look like a bash source tree." >&2
    echo "  See $ROOT/tests/bash-test-suite/README.md." >&2
    exit 1
fi

if [ ! -f "$BASH_SOURCE_DIR/tests/README" ]; then
    echo "error: \$BASH_SOURCE_DIR/tests/README not found." >&2
    echo "  BASH_SOURCE_DIR=$BASH_SOURCE_DIR is missing the tests/ README." >&2
    exit 1
fi

# ---- Build huck ----------------------------------------------------

if ! (cd "$ROOT" && cargo build --release --quiet --bin huck 2>/dev/null); then
    echo "error: 'cargo build --release --bin huck' failed; cannot run survey." >&2
    exit 1
fi
HUCK="$ROOT/target/release/huck"

if [ ! -x "$HUCK" ]; then
    echo "error: huck release binary not at $HUCK after build." >&2
    exit 1
fi

# ---- Scratch dir ---------------------------------------------------

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SCRATCH=$(mktemp -d -t "huck-bash-tests-${STAMP}.XXXXXX")

# Copy bash tests/ into the scratch so tests that modify-in-place don't
# touch the operator's $BASH_SOURCE_DIR. Many .tests reference .sub
# files by relative path, so we need the full directory shape.
cp -a "$BASH_SOURCE_DIR/tests" "$SCRATCH/tests"

# ---- Load skip list -----------------------------------------------

declare -A SKIPS=()
if [ -f "$SKIPS_FILE" ]; then
    while IFS= read -r line; do
        # strip comments and whitespace
        cat=$(printf '%s\n' "$line" | sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
        [ -z "$cat" ] && continue
        SKIPS[$cat]=1
    done < "$SKIPS_FILE"
fi

# ---- Discover categories ------------------------------------------

CATEGORIES=()
if [ -n "${HUCK_BASH_TEST_CATEGORY:-}" ]; then
    # Single-category mode (used by the smoke harness).
    if [ -f "$SCRATCH/tests/run-${HUCK_BASH_TEST_CATEGORY}" ]; then
        CATEGORIES=("$HUCK_BASH_TEST_CATEGORY")
    else
        echo "error: HUCK_BASH_TEST_CATEGORY=$HUCK_BASH_TEST_CATEGORY not found at $SCRATCH/tests/run-$HUCK_BASH_TEST_CATEGORY." >&2
        exit 1
    fi
else
    # Full sweep: every run-* except run-all and any in the skip list.
    while IFS= read -r runner; do
        base=$(basename "$runner")
        cat=${base#run-}
        [ "$cat" = "all" ] && continue
        [ -n "${SKIPS[$cat]:-}" ] && continue
        CATEGORIES+=("$cat")
    done < <(ls -1 "$SCRATCH/tests"/run-* 2>/dev/null | sort)
fi

# ---- Per-category execution + classification ---------------------

declare -A STATUS=()
declare -A DIFF_HEAD=()
PASS=0; FAIL=0; TIMEOUTS=0; ERRORS=0

for cat in "${CATEGORIES[@]}"; do
    runner="$SCRATCH/tests/run-$cat"
    if [ ! -x "$runner" ] && [ ! -r "$runner" ]; then
        STATUS[$cat]=ERROR
        ERRORS=$((ERRORS+1))
        echo "missing runner: $runner" > "$SCRATCH/$cat.err"
        continue
    fi

    # bash's runners cd into ./tests (they're invoked from inside that dir).
    # We replicate by running from $SCRATCH/tests.
    out_file="$SCRATCH/$cat.out"
    err_file="$SCRATCH/$cat.err"
    diff_file="$SCRATCH/$cat.diff"

    # bash's runners expect $THIS_SH and $BASH_TSTOUT.
    # Run with `timeout` to bound hangs.
    (
        cd "$SCRATCH/tests"
        THIS_SH="$HUCK" BASH_TSTOUT="$out_file" timeout "$TIMEOUT" sh "./run-$cat" \
            > "$diff_file" 2> "$err_file"
    )
    rc=$?

    if [ "$rc" -eq 124 ]; then
        STATUS[$cat]=TIMEOUT
        TIMEOUTS=$((TIMEOUTS+1))
    elif [ "$rc" -eq 0 ] && [ ! -s "$diff_file" ]; then
        STATUS[$cat]=PASS
        PASS=$((PASS+1))
    elif [ -s "$diff_file" ]; then
        STATUS[$cat]=FAIL
        FAIL=$((FAIL+1))
        DIFF_HEAD[$cat]=$(head -3 "$diff_file" | tr '\n' '|' | sed 's/|$//')
    else
        STATUS[$cat]=ERROR
        ERRORS=$((ERRORS+1))
    fi
done

# ---- Emit Markdown summary ----------------------------------------

TOTAL=${#CATEGORIES[@]}
SKIP_COUNT=${#SKIPS[@]}

cat <<EOF
# bash test-suite sweep — $(date -u +%Y-%m-%d)

bash source: $(basename "$BASH_SOURCE_DIR")
huck commit: $(cd "$ROOT" && git rev-parse --short HEAD 2>/dev/null || echo unknown)
Scratch dir (full diffs): $SCRATCH

## Summary

- Categories run: $TOTAL
- PASS: $PASS
- FAIL: $FAIL
- TIMEOUT: $TIMEOUTS
- ERROR: $ERRORS
- SKIP (from known-skips.txt): $SKIP_COUNT

## Per-category status

| Category | Status |
|---|---|
EOF

for cat in "${CATEGORIES[@]}"; do
    printf '| %s | %s |\n' "$cat" "${STATUS[$cat]}"
done

if [ "$SKIP_COUNT" -gt 0 ]; then
    echo
    echo "## Skipped (from known-skips.txt)"
    echo
    echo "| Category |"
    echo "|---|"
    for cat in "${!SKIPS[@]}"; do
        printf '| %s |\n' "$cat"
    done | sort
fi

exit 0
