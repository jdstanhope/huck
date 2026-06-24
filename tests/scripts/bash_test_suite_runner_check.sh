#!/usr/bin/env bash
# v214: smoke harness for the bash test-suite runner.
# Validates runner mechanics by running it for a single category and
# asserting the Markdown output is well-formed (contains an arith row
# with some valid status). Does NOT assert a specific PASS — that's
# what the committed baseline doc captures.
#
# When $BASH_SOURCE_DIR is unset, exits 0 with a SKIP message so the
# standard tests/scripts/*_diff_check.sh sweep loop stays green on
# machines without bash sources.
set -u

cd "$(dirname "$0")/../.." || exit 1

if [ -z "${BASH_SOURCE_DIR:-}" ]; then
    echo "SKIP: \$BASH_SOURCE_DIR unset; bash-test-suite runner not exercised."
    echo "      See tests/bash-test-suite/README.md to enable."
    exit 0
fi

# Run the runner against a single category.
OUT=$(HUCK_BASH_TEST_CATEGORY=arith bash tests/bash-test-suite/runner.sh 2>&1)
rc=$?

if [ "$rc" -ne 0 ]; then
    echo "FAIL: runner exited $rc"
    echo "$OUT"
    exit 1
fi

# Look for the arith row in the Markdown table. Pattern: `| arith | <status> |`.
status=$(printf '%s\n' "$OUT" | awk -F'|' '/^\| arith / { gsub(/ /, "", $3); print $3 }')

if [ -z "$status" ]; then
    echo "FAIL: could not find arith row in runner output (runner mechanics broken)"
    echo "$OUT"
    exit 1
fi

# Validate the status is a recognized value — pins runner classifies correctly.
case "$status" in
    PASS|FAIL|TIMEOUT|ERROR)
        echo "PASS [bash_test_suite_runner_check] runner mechanics OK (arith=$status)"
        exit 0
        ;;
    *)
        echo "FAIL: unrecognized status '$status' for arith (runner classification broken)"
        echo "$OUT"
        exit 1
        ;;
esac
