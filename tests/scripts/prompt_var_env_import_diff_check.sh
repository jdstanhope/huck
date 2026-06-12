#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v148: prompt-string variables and the
# environment. bash does NOT import PS1/PS2 from the environment (a non-interactive
# shell leaves them empty), but DOES import PS0/PS4. huck must match.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# check LABEL  ENV-ASSIGNMENTS  FRAGMENT  — runs `env <assigns> <shell> -c <frag>`.
check() {
    local label="$1" assigns="$2" frag="$3" b h
    b=$(env $assigns bash --norc -c "$frag" 2>&1; echo "rc=$?")
    h=$(env $assigns "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "PS1 not imported from env"   "PS1=ENVPS1" 'printf "[%s]" "$PS1"'
check "PS2 not imported from env"   "PS2=ENVPS2" 'printf "[%s]" "$PS2"'
check "PS0 IS imported from env"    "PS0=ENVPS0" 'printf "[%s]" "$PS0"'
check "PS4 IS imported from env"    "PS4=ENVPS4" 'printf "[%s]" "$PS4"'
check "inherited PS1 cmdsub inert"  'PS1=$(echo hi)' 'printf "[%s]" "$PS1"'
check "PS1 assignable after skip"   "PS1=ENVPS1" 'PS1="set>"; printf "[%s]" "$PS1"'
check "normal var still imported"   "FOO=bar"    'printf "[%s]" "$FOO"'
check "PS1 unset is empty"          ""           'printf "[%s]" "${PS1:-}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
