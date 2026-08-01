#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #231: `$-` reports the current shell
# option flags in bash's fixed order (a b e f h i k m n p r t u v x B C E H P T),
# followed by a trailing invocation letter — `c` for -c, `s` for stdin, none
# for a script file. Default-on flags h (hashall) and B (braceexpand) are
# always present.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Stdin-fed fragments (both shells read from stdin → trailing `s`).
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# `-c COMMAND` invocations (trailing `c`); pass the shell flags before -c.
check_c() {
    local label="$1"; shift
    local b h
    b=$(bash "$@" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$@" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- stdin mode (trailing s) ---
check "default"        'echo "[$-]"'
check "errexit"        'set -e; echo "[$-]"'
check "nounset"        'set -u; echo "[$-]"'
check "noglob"         'set -f; echo "[$-]"'
check "noclobber"      'set -C; echo "[$-]"'
check "allexport"      'set -a; echo "[$-]"'
check "keyword"        'set -k; echo "[$-]"'
check "e then u order" 'set -e; set -u; echo "[$-]"'
check "u before e src" 'set -u; set -e; echo "[$-]"'
check "many flags"     'set -aefkuBCEHPT; echo "[$-]"'
check "add then remove" 'set -e; set +e; echo "[$-]"'
check "B off"          'set +B; echo "[$-]"'

# --- -c mode (trailing c) ---
check_c "c default"    -c 'echo "[$-]"'
check_c "c errexit"    -c 'set -e; echo "[$-]"'
check_c "c restricted" -r -c 'echo "[$-]"'
check_c "c restricted+e" -r -c 'set -e; echo "[$-]"'

# --- script file (no trailing letter) ---
tf=$(mktemp /tmp/hk_dd_XXXX.sh)
printf 'echo "[$-]"\n' > "$tf"
check_c "file default" "$tf"
printf 'set -x\necho "[$-]"\n' > "$tf"
check_c "file xtrace" "$tf"
rm -f "$tf"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
