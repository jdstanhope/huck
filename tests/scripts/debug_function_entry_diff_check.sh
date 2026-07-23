#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v329 (#274): DEBUG trap fires ONCE on
# function ENTRY (after the call-site fire, before the first body command)
# with $LINENO = the function's DEFINITION line (`f() {` / `function f {`).
# huck's function-BODY $LINENO is already correct; this harness isolates the
# entry-fire gap. `$LINENO` values below were derived by running each
# fragment through real bash --norc --noprofile FIRST (bash 5.2.21).
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

check "entry multiline -T"  'set -T
trap "echo D=\$LINENO" DEBUG
f() {
  echo a
}
f'
check "entry oneline -T"    'set -T
trap "echo D=\$LINENO" DEBUG
f() { echo a; }
f'
check "entry function-kw"   'shopt -s extdebug
trap "echo D=\$LINENO" DEBUG
function f {
  echo a
}
f'
check "entry nested -T"     'set -T
trap "echo D=\$LINENO" DEBUG
g() { echo g; }
f() { g; }
f'
check "no-tracing no-entry" 'trap "echo D=\$LINENO" DEBUG
f() { echo a; }
f'
check "funcname at entry"   'set -T
trap "echo \${FUNCNAME[0]:-main}" DEBUG
f() { echo a; }
f'

# v329 (leak fix): the DEBUG action's own function call must not leak its line
# into the surrounding code's $LINENO (bash restores LINENO across the action).
check "debug action no lineno leak" 'set -o functrace
pdt() { echo "dbg $1"; }
fn1() {
  echo "L $LINENO"
  echo "L $LINENO"
}
trap "pdt \$LINENO" DEBUG
fn1'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
