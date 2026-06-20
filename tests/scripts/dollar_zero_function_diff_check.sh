#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v196: `$0` is the shell/script
# invocation name and is NOT rebound inside a function (bash keeps the script
# name, unlike ksh/zsh). Also covers `${@:0}` / `${*:0}` (which include $0)
# and FUNCNAME (which IS the function name — must stay correct).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Compare a fragment run via a SCRIPT FILE (so $0 = the script path). Both
# shells get the same temp file, so $0 is identical when correct.
check_file() {
    local label="$1" frag="$2" f b h
    f=$(mktemp /tmp/huck_v196.XXXXXX.sh)
    printf '%s\n' "$frag" > "$f"
    b=$(bash "$f" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "rc=$?")
    rm -f "$f"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Compare a fragment run via `-c` with an explicit argv0 ("prog"), so $0 = prog.
check_c() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" prog a b 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" prog a b 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- $0 inside a function keeps the invocation name ---
check_file "fn \$0 via file"        'f(){ echo "[$0]"; }; f'
check_file "nested fn \$0"          'g(){ echo "[$0]"; }; f(){ g; }; f'
check_file "\$0 basename pattern"   'f(){ echo "${0##*/}"; }; f'
check_c    "fn \$0 via -c argv0"    'f(){ echo "[$0]"; }; f'
check_c    "nested fn \$0 via -c"   'g(){ echo "[$0]"; }; f(){ g; }; f'
# --- ${@:0} / ${*:0} include $0 (also not rebound in a function) ---
check_c    "\${@:0} in fn"          'f(){ echo "[${@:0}]"; }; f x y'
check_c    "\${*:0:1} in fn"        'f(){ echo "[${*:0:1}]"; }; f'
check_c    "\${@:0} top level"      'echo "[${@:0}]"'
# --- FUNCNAME must STILL be the function name (regression guard) ---
check_c    "FUNCNAME in fn"         'f(){ echo "fn=$0 func=${FUNCNAME[0]}"; }; f'
check_c    "FUNCNAME nested"        'g(){ echo "$0:${FUNCNAME[0]}:${FUNCNAME[1]}"; }; f(){ g; }; f'
# --- top-level $0 unchanged ---
check_file "top-level \$0"          'echo "[$0]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
