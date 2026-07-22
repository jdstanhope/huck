#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v324 (#257): DEBUG trap firing points.
# Covers the two firing points added in this iteration's task 2 — pipeline
# stages (fired in the PARENT before forking each stage) and functrace
# subshells (DEBUG/RETURN preserved across a subshell fork under `set -T`,
# NOT preserved without it) — plus the compound-header fires from task 1
# (for/select/case/arith-for) and unchanged constructs (while/if/group/single
# command), all gated on the fixed marker `D` (not $LINENO).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "pipeline stages"       'trap "echo D" DEBUG; echo a | cat'
check "for per-iteration"     'trap "echo D" DEBUG; for x in 1 2 3; do echo $x; done'
check "arith-for expressions" 'trap "echo D" DEBUG; for ((i=0;i<2;i++)); do echo $i; done'
check "case header"           'trap "echo D" DEBUG; case a in a) echo m;; esac'
check "select header"         'trap "echo D" DEBUG; select x in a b; do echo $x; break; done <<< 1'
check "functrace subshell"    'set -T; trap "echo D" DEBUG; ( echo a; echo b )'
check "subshell no functrace" 'trap "echo D" DEBUG; ( echo a; echo b )'
check "while unchanged"       'trap "echo D" DEBUG; i=0; while [ $i -lt 2 ]; do echo $i; i=$((i+1)); done'
check "if unchanged"          'trap "echo D" DEBUG; if true; then echo y; fi'
check "group unchanged"       'trap "echo D" DEBUG; { echo a; echo b; }'
check "single command"        'trap "echo D" DEBUG; echo solo'
check "nested pipeline+for"   'trap "echo D" DEBUG; for x in 1 2; do echo $x | cat; done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
