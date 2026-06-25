#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v220: `declare -p` ANSI-C value quoting.
# A value containing a control char (newline, tab, 0x01, …) must render as
# $'…' instead of a literal control byte inside "…", matching bash 5.2.21.
# Control-free values must stay in the double-quoted "…" form.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/release/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
command -v bash >/dev/null 2>&1 || { echo "SKIP: bash not found"; exit 0; }

fragments=(
  $'v=$\'i\\n\'; declare -p v'
  $'v=$\'a\\tb\'; declare -p v'
  $'v=$\'a\\x01b\'; declare -p v'
  $'declare -a a=(x $\'i\\n\'); declare -p a'
  $'declare -A m=([k]=$\'a\\tb\'); declare -p m'
  'v=hello; declare -p v'
  $'v=\'a$b"c\'; declare -p v'
)

PASS=0; FAIL=0
for frag in "${fragments[@]}"; do
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$frag"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$frag"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1))
    fi
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
