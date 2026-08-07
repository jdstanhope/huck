#!/usr/bin/env bash
# Shared scaffolding for the `tests/scripts/*_diff_check.sh` bash-diff harnesses.
#
# Source it as the first thing a harness does:
#
#     . "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
#
# and finish with `harness_summary`. `HUCK_BIN` from the environment still wins
# (that is how `run_diff_checks.sh` sweeps both binaries); a harness that must
# default to the RELEASE binary sets `HARNESS_PROFILE=release` before sourcing.
#
# Deliberately NOT a positional parameter: `.` with no arguments passes the
# CALLER's positional parameters through, so a `${1:-debug}` here would silently
# read the harness's own `$1`.
#
# What this file deliberately does NOT own: the DRIVER — how a fragment reaches
# each shell. `printf '%s\n' "$frag" | bash` and `bash -c "$frag"` are different
# top-level readers in huck (the `-c` string, the stdin reader and a script file
# diverge on `$?` preservation, syntax-error abort and error fatality — #332,
# #284, #198), and `--norc --noprofile` vs a bare `bash` is a third axis. A
# harness that pipes to stdin usually does so precisely to pin that reader, so
# each harness keeps its own two driver lines and calls `compare`.
#
# Harnesses are invoked from the REPO ROOT (`tests/scripts/foo_diff_check.sh`),
# which is why the default binary path is `$(pwd)`-relative — unchanged from the
# copies this file replaces.

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/${HARNESS_PROFILE:-debug}/huck}"
[[ -x "$HUCK_BIN" ]] || {
    echo "build huck first: $HUCK_BIN" >&2
    exit 1
}

PASS=0
FAIL=0

# compare <label> <bash-output> <huck-output>
#
# Byte-compares the two captures and records the verdict. On a mismatch it
# prints a diff of bash-vs-huck indented four spaces (`<` is bash, `>` is huck).
compare() {
    local label="$1" b="$2" h="$3"
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$label"
        PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# Print the tally and exit non-zero if anything failed. `run_diff_checks.sh`
# reads the EXIT STATUS, so a harness that prints a total without exiting
# non-zero would fail silently.
harness_summary() {
    echo ""
    echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
    exit $((FAIL > 0 ? 1 : 0))
}
