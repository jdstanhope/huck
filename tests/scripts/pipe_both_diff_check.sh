#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v179: `|&` (pipe stdout+stderr), bash
# shorthand for `2>&1 |`. Each case EXECUTES and asserts identical stdout+exit
# (the merged stderr flows through the pipe to the consumer's stdout).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b bo h ho
    bo=$(bash --norc --noprofile -c "$frag" 2>/dev/null); b=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    if [[ "$bo" == "$ho" && "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash(rc=%s)=[%s]  huck(rc=%s)=[%s]\n' "$label" "$b" "$bo" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

check "simple both streams"   'sh -c "echo O; echo E 1>&2" |& cat'
check "stderr reaches grep"   'sh -c "echo keep; echo DROPME 1>&2" |& grep keep'
check "only stderr"           'sh -c "echo ERR 1>&2" |& cat'
check "compound group"        '{ echo o; echo e 1>&2; } |& cat'
check "subshell producer"     '( echo o; echo e 1>&2 ) |& cat'
check "chained pipe-both"     'sh -c "echo x; echo y 1>&2" |& tr a-z A-Z |& cat'
check "pipe-both then sort"   'sh -c "echo b; echo a 1>&2" |& sort'
check "no-space form"         'sh -c "echo O;echo E 1>&2"|&cat'
check "control plain pipe"    'echo hi | tr a-z A-Z'
check "control logical or"    'false || echo alt'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
