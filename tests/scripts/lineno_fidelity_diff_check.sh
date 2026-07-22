#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v325: $LINENO fidelity cluster (#258).
#
# Two comparison helpers, shared across the v325 task cluster:
#   check_file  "label" 'fragment'  - fragment written to a temp file, run as
#                                     `bash --norc <file>` vs `"$HUCK_BIN" <file>`
#                                     (FILE ARG).
#   check_stdin "label" 'fragment'  - same fragment, run as
#                                     `bash --norc < file` vs `"$HUCK_BIN" < file`
#                                     (piped STDIN).
#
# Task 1 (this task) covers eval's multi-line-body $LINENO offset via
# check_file only. Task 2 (per-line stdin $LINENO) adds check_stdin variants
# of these same eval cases plus its own cases -- until Task 2 lands, the
# stdin form of a multi-line eval can still differ from bash for reasons
# unrelated to the eval offset fixed here (piped-stdin $LINENO tracking is a
# separate bug), so those variants are deliberately deferred rather than
# included in a red state.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

LF_TMPDIR=$(mktemp -d)
cleanup_lf_tmpdir() { rm -rf "$LF_TMPDIR"; }
trap cleanup_lf_tmpdir EXIT

check_file() {
    local label="$1" frag="$2" f b h
    f="$LF_TMPDIR/file_$$_${PASS}_${FAIL}_$RANDOM.sh"
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s (file)\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (file)\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

check_stdin() {
    local label="$1" frag="$2" f b h
    f="$LF_TMPDIR/stdin_$$_${PASS}_${FAIL}_$RANDOM.sh"
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc < "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" < "$f" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s (stdin)\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (stdin)\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# --- Task 1: eval body $LINENO offset (check_file only; see header comment) ---
check_file "eval@1 1-line" 'eval '\''echo $LINENO'\'''
check_file "eval@3 2-line" ':
:
eval '\''echo $LINENO
echo $LINENO'\'''
check_file "eval@2 3-line" ':
eval '\''echo $LINENO
echo $LINENO
echo $LINENO'\'''

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
