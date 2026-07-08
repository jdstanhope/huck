#!/usr/bin/env bash
# Byte-identical bash<->huck harness for two POSIX.2 conformance points
# (bash's own posix2.tests): (1) `$OPTIND` reads as 1 before any getopts runs
# and carries the integer attribute; (2) the `set` (no-args) variable listing
# quotes values with bash's `sh_contains_shell_metas` + `sh_single_quote` rules
# — bare when nothing needs quoting, `#`/`~` metacharacters only in leading
# position, a lone `'` rendered `\'`. Compares stdout+rc.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  h=$("$HUCK_BIN" -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- (1) OPTIND initial value + integer attribute ---
check "OPTIND cold value"   'echo "$OPTIND"'
check "OPTIND declare -p"   'declare -p OPTIND'
check "OPTIND after getopts" 'set -- -a val; getopts a: o; echo "$o $OPTIND $OPTARG"'
check "OPTIND arithmetic"   'echo $((OPTIND + 4))'

# --- (2) `set` listing value quoting (each value read back via sed) ---
q() { check "set-quote $1" "$2=\"$3\"; set 2>/dev/null | sed -n 's:^$2=::p'"; }
q "plain"        V "abc"
q "digits"       V "12345"
q "hash-mid"     V 'ab#cd'
q "hash-lead"    V '#abcd'
q "tilde-lead"   V '~'
q "tilde-mid"    V 'a~b'
q "tilde-after=" V 'x=~'
q "tilde-after:" V 'a:~'
q "space"        V 'a b'
q "glob-star"    V 'a*b'
q "bang"         V 'a!b'
q "empty"        V ''
# lone single quote and embedded quotes (assign via $'…' to dodge harness quoting)
check "set-quote lone-squote"  $'V=$\'\\047\'; set 2>/dev/null | sed -n "s:^V=::p"'
check "set-quote embed-squote" $'V=$\'a\\047b\'; set 2>/dev/null | sed -n "s:^V=::p"'
check "set-quote two-squote"   $'V=$\'\\047\\047\'; set 2>/dev/null | sed -n "s:^V=::p"'
check "set-quote tab-ctrl"     $'V=$\'a\\tb\'; set 2>/dev/null | sed -n "s:^V=::p"'

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
