#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #402: `kill`'s argument diagnostics —
# the usage text, the single "invalid signal specification" wording for every
# rejected sigspec form, and bash's `legal_number()` whitespace rules for a
# numeric target.
#
# SAFETY: as in kill_negative_pid_diff_check.sh, no fragment sends a REAL
# signal to 0 / -0 / -1 — under a non-job-control `bash -c` those name the
# HARNESS's own process group. Targets here are signal-0 probes, pid 1 (which
# the harness may not signal), or the nonexistent group -99999.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Normalize the leading program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — argv[0], not behavior. The usage lines carry no
# such prefix and are compared verbatim.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- usage text (rc 2), every arm that can reach it -------------------------
check "no args"             'kill; echo rc=$?'
check "sig, no targets"     'kill -9; echo rc=$?'
check "-- , no targets"     'kill -- ; echo rc=$?'
check "sig then bare --"    'kill -0 --; echo rc=$?'
check "-s then bare --"     'kill -s TERM --; echo rc=$?'
check "-n then bare --"     'kill -n 9 --; echo rc=$?'
check "-s missing arg"      'kill -s; echo rc=$?'
check "-n missing arg"      'kill -n; echo rc=$?'

# --- one wording for every rejected sigspec form ----------------------------
check "-<num> out of range" 'kill -123 1; echo rc=$?'
check "-<num> huge"         'kill -99999 1; echo rc=$?'
check "-<name> unknown"     'kill -FOO 1; echo rc=$?'
check "-<name> SIG-prefix"  'kill -SIGFOO 1; echo rc=$?'
check "-s unknown name"     'kill -s BOGUS 1; echo rc=$?'
check "-n out of range"     'kill -n 99 1; echo rc=$?'
check "-n negative"         'kill -n -1 1; echo rc=$?'
check "-s empty"            'kill -s "" 1; echo rc=$?'
check "-l unknown name"     'kill -l xyz; echo rc=$?'
check "-l out of range"     'kill -l 99; echo rc=$?'

# --- legal_number() target parsing ------------------------------------------
# strtol skips leading whitespace; legal_number then skips trailing SPACE/TAB
# only, and the whole string must be consumed.
check "leading space"       'kill -0 " 1"; echo rc=$?'
check "trailing space"      'kill -0 "1 "; echo rc=$?'
check "tabs both sides"     'kill -0 "	1	"; echo rc=$?'
check "padded negative"     'kill -0 " -99999 "; echo rc=$?'
check "leading newline"     'kill -0 "
1"; echo rc=$?'
check "trailing newline"    'kill -0 "1
"; echo rc=$?'
check "leading plus"        'kill -0 +1; echo rc=$?'
check "hex is not a pid"    'kill -0 0x10; echo rc=$?'
check "trailing garbage"    'kill -0 12abc; echo rc=$?'
check "whitespace only"     'kill -0 " "; echo rc=$?'
check "interior space"      'kill -0 "1 2"; echo rc=$?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
