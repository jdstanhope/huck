#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v221: a prefix (inline) assignment on a
# function call (`var=val funcname`) does NOT persist after the function returns,
# matching bash 5.2.21 — only POSIX special builtins persist a prefix
# assignment. The function's own global set/unset of the same var is clobbered
# back to the pre-command value by the snapshot/restore. Holds in posix mode too.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/release/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
command -v bash >/dev/null 2>&1 || { echo "SKIP: bash not found"; exit 0; }
PASS=0; FAIL=0

fragments=(
  'v=1; f(){ :; }; v=5 f; echo $v'
  'v=1; f(){ v=99; }; v=5 f; echo $v'
  'v=1; f(){ local v=99; }; v=5 f; echo $v'
  'v=1; f(){ unset v; }; v=5 f; echo "[${v-UNSET}]"'
  'f(){ :; }; v=5 f; echo "[${v-UNSET}]"'
  'v=1; f(){ echo $v; }; v=5 f'
  'f(){ printenv V; }; V=x f'
  'set -o posix; v=1; f(){ :; }; v=5 f; echo $v'
  'set -o posix; v=10; f(){ v=20 return; }; f; echo $v'
)

for frag in "${fragments[@]}"; do
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$frag"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$frag"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1))
    fi
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
