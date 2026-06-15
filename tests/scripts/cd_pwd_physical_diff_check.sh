#!/usr/bin/env bash
# Byte-identical bash<->huck harness for logical/physical PWD (cd -P/-L,
# pwd -P/-L, set -o physical). Builds its own mktemp fixture with a symlink so
# paths are machine-independent; compares stdout + exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
T=$(mktemp -d)
mkdir -p "$T/real"; ln -s real "$T/link"
trap 'rm -rf "$T"' EXIT
PASS=0; FAIL=0
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "logical default"     "cd $T/link; echo \"\$PWD\"; pwd; pwd -L"
chk "pwd -P resolves"     "cd $T/link; pwd -P"
chk "cd -P physical"      "cd -P $T/link; echo \"\$PWD\""
chk "cd -L logical"       "cd -L $T/link; echo \"\$PWD\""
chk "cd last-wins LP"     "cd -L -P $T/link; echo \"\$PWD\""
chk "cd last-wins PL"     "cd -P -L $T/link; echo \"\$PWD\""
chk "pwd last-wins LP"    "cd $T/link; pwd -L -P"
chk "pwd last-wins PL"    "cd $T/link; pwd -P -L"
chk "cd .. lexical"       "cd $T/link; cd ..; echo \"\$PWD\""
chk "set -o physical cd"  "set -o physical; cd $T/link; echo \"\$PWD\""
chk "set -o physical pwd" "set -o physical; cd $T/link; pwd"
chk "cd - logical"        "cd $T/link; cd /tmp; cd - >/dev/null; echo \"\$PWD\""
chk "cd root"             'cd /; echo "$PWD"'
chk "pwd -x rc"           'pwd -x >/dev/null 2>&1; echo rc=$?'
chk "pwd extra arg"       "cd $T/link; pwd foo"
chk "cd -x rc"            'cd -x >/dev/null 2>&1; echo rc=$?'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
