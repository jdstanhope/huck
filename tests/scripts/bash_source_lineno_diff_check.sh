#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v153: BASH_SOURCE / BASH_LINENO / FUNCNAME matrix.
# Both shells run on the SAME temp file paths so absolute BASH_SOURCE values match.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check_c() {
    local l="$1" f="$2" b h
    b=$(bash -c "$f" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$f" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check_file() {
    local l="$1" body="$2" f b h
    f=$(mktemp /tmp/hk_v153_XXXX.sh)
    printf '%s' "$body" > "$f"
    b=$(bash "$f" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "rc=$?")
    rm -f "$f"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check_sourced() {
    local l="$1" mainfmt="$2" lib="$3" lf mf b h
    lf=$(mktemp /tmp/hk_v153_lib_XXXX.sh)
    mf=$(mktemp /tmp/hk_v153_main_XXXX.sh)
    printf '%s' "$lib" > "$lf"
    # shellcheck disable=SC2059
    printf "$mainfmt" "$lf" > "$mf"
    b=$(bash "$mf" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$mf" 2>&1; echo "rc=$?")
    rm -f "$lf" "$mf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- -c fragment checks (no file paths involved) ---
check_c "c top-level"   'echo "[${FUNCNAME[@]:-u}] [${BASH_SOURCE[@]:-u}] [${BASH_LINENO[@]:-u}]"'
check_c "c in-func"     'f(){ echo "[${FUNCNAME[@]}] [${BASH_SOURCE[@]}] [${BASH_LINENO[@]}]"; }; f'
check_c "c nested"      $'inner(){ echo "[${FUNCNAME[@]}] [${BASH_LINENO[@]}]"; }\nouter(){ inner; }\nouter'
check_c "c scalar"      'f(){ echo "$FUNCNAME $BASH_SOURCE $BASH_LINENO"; }; f'
check_c "c depth"       'f(){ echo "${#FUNCNAME[@]} ${#BASH_SOURCE[@]} ${#BASH_LINENO[@]}"; }; f'

# --- script file checks (both shells get the SAME temp path -> BASH_SOURCE matches) ---
check_file "script top"  $'echo "[${FUNCNAME[@]:-u}] [${#BASH_SOURCE[@]}] [${BASH_LINENO[@]}]"\necho "src=$BASH_SOURCE"\n'
check_file "script func" $'g(){ echo "[${FUNCNAME[@]}] [${BASH_LINENO[@]}]"; }\nf(){ g; }\nf\n'
check_file "script src0" $'echo "$BASH_SOURCE"\n'

# --- sourced-file checks (lib + main run under the same temp paths) ---
check_sourced "sourced top"  $'echo "main-src=$BASH_SOURCE"\nsource %s\n' \
    $'echo "[${FUNCNAME[@]:-u}] [${BASH_SOURCE[@]}] [${BASH_LINENO[@]}]"\n'
check_sourced "sourced func" $'source %s\ncaller(){ libfn; }\ncaller\n' \
    $'libfn(){ echo "[${FUNCNAME[@]}] [${BASH_SOURCE[@]}] [${BASH_LINENO[@]}]"; }\n'
check_sourced "src in func"  $'foo(){ source %s; }\nfoo\n' \
    $'echo "[${FUNCNAME[@]}]"\n'

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
