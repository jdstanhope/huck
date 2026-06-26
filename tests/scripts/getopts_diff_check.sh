#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v111: the getopts builtin (M-106).
# Verbose-error fragments redirect builtin stderr (the huck:-vs-bash: prefix is
# the only divergence; the load-bearing name/OPTARG/OPTIND/rc match exactly).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "simple loop"         'set -- -a -b val -c; while getopts "ab:c" o; do echo "$o:${OPTARG-}"; done; echo "i=$OPTIND"'
check "clustered"           'set -- -abc; while getopts "abc" o; do echo "$o"; done'
check "attached arg"        'set -- -bVAL; getopts "b:" o; echo "$o=$OPTARG"'
check "separate arg"        'set -- -b VAL; getopts "b:" o; echo "$o=$OPTARG i=$OPTIND"'
check "double dash"         'set -- -a -- x; while getopts "ab" o; do echo "$o"; done; echo "i=$OPTIND"'
check "non-option stops"    'set -- foo -a; while getopts "a" o; do echo "$o"; done; echo "i=$OPTIND"'
check "invalid verbose"     'set -- -z; getopts "ab" o 2>/dev/null; echo "rc?=$? o=$o"'
check "invalid silent"      'set -- -z; getopts ":ab" o; echo "o=$o OPTARG=$OPTARG"'
check "missing verbose"     'set -- -b; getopts "ab:" o 2>/dev/null; echo "o=$o"'
check "missing silent"      'set -- -b; getopts ":ab:" o; echo "o=$o OPTARG=$OPTARG"'
check "OPTERR=0 suppress"   'set -- -z; OPTERR=0; getopts "ab" o; echo "o=$o"'
check "no-args uses dollar-at" 'f() { while getopts "x" o; do echo "$o"; done; }; f -x -x'
check "local OPTIND reset"  'f(){ local OPTIND=1 o; while getopts "a" o; do echo "f$o"; done; }; set -- -a; getopts "a" t; echo "t=$t"; f -a -a'

# --- file-mode checks: run each fragment as a SCRIPT FILE so the non-
# interactive prologue (`<path>: line N:`) is produced, and assert
# byte-identical stdout+stderr+rc against bash 5.2.21. The same temp path is
# used for both shells, so the prologue path matches. ---
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-getopts.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# B1: too few operands → usage, no shell prologue, rc 2
checkf "file: usage too few"      'getopts; echo "rc=$?"'
# B2: getopts-internal diagnostic prefixed with $0 (the script path)
checkf "file: internal diag \$0"  'set -- -z; getopts ab o; echo "o=$o rc=$?"'
# B3: invalid option to getopts itself → error + usage, rc 2
checkf "file: invalid builtin opt" 'getopts -a opts name; echo "rc=$?"'
# B4: invalid name var → builtin prologue error + OPTIND still bound
checkf "file: invalid name optind" 'set -- -a
getopts :ab: bad-name
echo "oi=$OPTIND"
[ "$OPTIND" -gt 1 ] && shift $(( OPTIND - 1 ))
echo "rest=$*"'
# B5: readonly OPTARG → prologue-prefixed readonly error (generic assign site)
checkf "file: readonly OPTARG"     'set -- -a bb
readonly OPTARG
getopts :x x
echo "done x=$x"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
