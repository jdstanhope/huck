#!/usr/bin/env bash
# v214: bash test-suite runner — runs bash's own tests/run-* against huck.
# Reads bash source from $BASH_SOURCE_DIR (operator-supplied; NOT vendored).
# Emits a Markdown summary to stdout; per-category logs land in a /tmp scratch.
#
# Env vars:
#   BASH_SOURCE_DIR             (required) path to extracted bash-5.2.21/ source
#   HUCK_BASH_TEST_CATEGORY     (optional) single category name to run; default: all
#   HUCK_TEST_TIMEOUT           (optional) per-category timeout in seconds; default: 30
#   HUCK_TEST_TIMEOUT_LONG      (optional) timeout for INHERENTLY-long categories
#                               (jobs, minimal); default: 180. See LONG_CATEGORIES.
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SKIPS_FILE="$ROOT/tests/bash-test-suite/known-skips.txt"
TIMEOUT=${HUCK_TEST_TIMEOUT:-30}

# A few upstream categories are INHERENTLY long-running — their .tests contain
# real foreground `sleep`/`wait` calls or deliberate `read -t` timeout tests, so
# they take far longer than 30s in ANY shell (jobs.tests alone runs ~60s in bash
# 5.2.21; minimal's read.tests spends ~17s in its own `read -t` waits). These are
# NOT huck hangs — huck is as-fast-or-faster than bash on them (verified v299) —
# so the default 30s cap mislabels them TIMEOUT (which reads like a hang). Give
# them a generous per-category timeout so the runner reports their TRUE PASS/FAIL
# status; the default still catches real hangs quickly for every other category.
LONG_TIMEOUT=${HUCK_TEST_TIMEOUT_LONG:-180}
LONG_CATEGORIES=" jobs minimal "
category_timeout() {
    case "$LONG_CATEGORIES" in
        *" $1 "*) echo "$LONG_TIMEOUT" ;;
        *) echo "$TIMEOUT" ;;
    esac
}

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

# ---- Provision test helpers (recho / zecho / printenv) -------------
# bash's .tests invoke these standalone helper programs (NOT builtins);
# bash builds them from support/*.c. We compile them at runtime from the
# operator-supplied $BASH_SOURCE_DIR (nothing vendored; GPL posture) into
# an ephemeral dir and prepend it to PATH for the category runs. An
# operator may instead point HUCK_BASH_TEST_HELPERS at a pre-built dir.
# If no compiler is available, we warn and continue (categories that need
# the helpers stay FAIL, exactly as before) — provisioning never aborts
# the sweep.
HELPER_DIR=""
HELPERS="recho zecho printenv"

if [ -n "${HUCK_BASH_TEST_HELPERS:-}" ]; then
    # Override: use a pre-built dir if it has all three executables.
    # (subshell so the `exit 1` aborts only the check, not the runner)
    if ( for h in $HELPERS; do [ -x "$HUCK_BASH_TEST_HELPERS/$h" ] || exit 1; done ); then
        HELPER_DIR="$HUCK_BASH_TEST_HELPERS"
    else
        echo "warning: HUCK_BASH_TEST_HELPERS=$HUCK_BASH_TEST_HELPERS is missing one of: $HELPERS; compiling from source instead." >&2
    fi
fi

if [ -z "$HELPER_DIR" ]; then
    CC="${CC:-cc}"
    if ! command -v "$CC" >/dev/null 2>&1; then
        echo "warning: no C compiler ('$CC') found; test helpers (recho/zecho/printenv) unavailable. Categories needing them will FAIL. Set HUCK_BASH_TEST_HELPERS to a pre-built dir or install a compiler." >&2
    elif [ ! -f "$BASH_SOURCE_DIR/support/recho.c" ]; then
        echo "warning: $BASH_SOURCE_DIR/support/recho.c not found; cannot build test helpers. Categories needing them will FAIL." >&2
    else
        built_dir=$(mktemp -d -t "huck-bash-helpers.XXXXXX")
        inc="-I$BASH_SOURCE_DIR -I$BASH_SOURCE_DIR/include -I$BASH_SOURCE_DIR/builtins"
        for h in $HELPERS; do
            # -include string.h: printenv.c uses strlen without including it,
            # which modern gcc treats as a hard error.
            if ! "$CC" $inc -include string.h -o "$built_dir/$h" "$BASH_SOURCE_DIR/support/$h.c" 2>"$built_dir/$h.log"; then
                echo "warning: failed to compile test helper '$h' (see $built_dir/$h.log); categories needing it will FAIL." >&2
            fi
        done
        # Use the dir if anything compiled (a partial build still helps).
        if [ -n "$(ls -A "$built_dir" 2>/dev/null | grep -vE '\.log$')" ]; then
            HELPER_DIR="$built_dir"
        fi
    fi
fi

if [ -n "$HELPER_DIR" ]; then
    PATH="$HELPER_DIR:$PATH"
    export PATH
fi

# ---- TMPDIR ---------------------------------------------------------
# bash's own tests/run-all does `: ${TMPDIR:=/tmp}; export TMPDIR` (lines
# 17-18) before running any category, and its shipped .right files were
# generated under that condition. 64 of the test .sub files use a BARE
# $TMPDIR (e.g. extglob6.sub's `DIR=$TMPDIR/extglob-$$; mkdir $DIR; cd
# $DIR; touch a`). With TMPDIR unset those `mkdir`/`cd` fail and the test
# leaks files into the shared scratch dir for ANY shell (verified: real
# bash leaks the same stray `a`), which then pollutes later categories
# (a stray `a` makes getopts's unquoted `[-a]` glob expand). Match bash's
# harness precondition so our run replicates the .right conditions.
: "${TMPDIR:=/tmp}"
export TMPDIR

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
    # Run with `timeout` to bound hangs. Inherently-long categories get a
    # generous cap (see category_timeout / LONG_CATEGORIES) so they are not
    # mislabeled TIMEOUT.
    cat_timeout=$(category_timeout "$cat")
    (
        cd "$SCRATCH/tests"
        THIS_SH="$HUCK" BASH_TSTOUT="$out_file" timeout "$cat_timeout" sh "./run-$cat" \
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
