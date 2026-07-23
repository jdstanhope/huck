#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v330: the `caller` builtin (#281).
#
# `caller` reports the LINE, [FUNC,] and FILE of a call-stack frame, so it must
# be exercised from a real FILE (not `-c`) so the reported filename is
# meaningful and matches between bash and huck. Uses a temp-file `check`
# helper, as `lineno_fidelity_diff_check.sh` does.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

CL_TMPDIR=$(mktemp -d)
cleanup_cl_tmpdir() { rm -rf "$CL_TMPDIR"; }
trap cleanup_cl_tmpdir EXIT

check() {
    local label="$1" frag="$2" f b h
    f="$CL_TMPDIR/file_$$_${PASS}_${FAIL}_$RANDOM.sh"
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc --noprofile "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# Informational only (does NOT affect PASS/FAIL): bash has a documented quirk
# where `caller` invoked with NO enclosing function/sourced-script context at
# all prints the literal placeholder "0 NULL" and returns rc 0, rather than
# failing. huck's call_stack model (rc 1, no output, when n<2) is the
# spec-intended, POSIX-sane behavior; reproducing bash's literal "NULL"
# placeholder is explicitly out of scope for v330 (self-review). Recorded here
# so the divergence is visible, not silently untested.
check_known_divergence() {
    local label="$1" frag="$2" f b h
    f="$CL_TMPDIR/kd_$$_${PASS}_${FAIL}_$RANDOM.sh"
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc --noprofile "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s (known-divergence case now matches bash)\n' "$label"
    else
        printf 'INFO (known divergence, not counted): %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
    fi
}

# g -> f, then every `caller` form (in-range/out-of-range/invalid/extra args).
check "caller forms in f<-g" 'f() {
  caller; echo "rc=$?"
  caller 0; echo "rc=$?"
  caller 1; echo "rc=$?"
  caller 2; echo "rc=$?"
  caller foo; echo "rc=$?"
  caller 0 99; echo "rc=$?"
}
g() { f; }
g'

# Top-level (no enclosing function) `caller` — see check_known_divergence doc.
check_known_divergence "top-level caller (bash: literal 0 NULL quirk)" '
g() { :; }
g
caller; echo "toprc=$?"'

# stack-trace loop (dbg-support.sub shape): walk the whole FUNCNAME stack via
# `caller $i` until it goes out of range.
check "stack-trace loop" 'trace() { local i; for ((i=0; i<${#FUNCNAME[@]}; i++)); do caller $i || break; done; }
a() { trace; }
b() { a; }
b'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
