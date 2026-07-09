#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `set -e` (errexit) and the ERR trap
# exemption on AND-OR lists.
#
# bash rule: under `set -e`, a command in an `&&`/`||` (and-or) list triggers
# errexit / the ERR trap ONLY when it is the SYNTACTICALLY LAST command in that
# list. A command followed by `&&` OR `||` is exempt. huck previously exempted
# only the `||`-next case (`!next_is_or`), so `false && echo x` wrongly exited
# under `set -e` — breaking `cmd | grep -q x && ...` and
# `command -v x && ... || ...`. Fixed v275 (next_is_or -> is_last in
# run_andor_group). Compares stdout+rc.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  h=$("$HUCK_BIN" -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- non-last failure in an and-or list: EXEMPT (must NOT exit) ---
check "false&&echo"        'set -e; false && echo x; echo after'
check "false&&true"        'set -e; false && true; echo after'
check "false&&false"       'set -e; false && false; echo after'
check "false&&a||b"        'set -e; false && echo a || echo b; echo after'
check "true&&false&&echo"  'set -e; true && false && echo x; echo after'   # middle-fail exempt
check "false&&echo&&echo"  'set -e; false && echo x && echo y; echo after'
check "false&&(sub)"       'set -e; false && (false); echo after'
check "false&&grp"         'set -e; false && { echo x; }; echo after'

# --- last-command failure: NOT exempt (must exit rc1) ---
check "echo&&false"        'set -e; echo a && false; echo after'
check "true&&false"        'set -e; true && false; echo after'
check "false||false"       'set -e; false || false; echo after'
check "a&&b&&false"        'set -e; echo a && echo b && false; echo after'
check "bare-false"         'set -e; false; echo after'
check "brace-false"        'set -e; { false; }; echo after'

# --- || short-circuit (unchanged behavior; regression guard) ---
check "false||echo"        'set -e; false || echo x; echo after'
check "true||false"        'set -e; true || false; echo after'
check "false||true"        'set -e; false || true; echo after'

# --- pipelines as the group command ---
check "true|false"         'set -e; true | false; echo after'   # pipeline last, fails -> exit
check "false|true"         'set -e; false | true; echo after'   # pipeline status 0 -> after
check "false|true&&echo"   'set -e; false | true && echo ok; echo after'

# --- negated pipeline (exempt regardless) ---
check "!false"             'set -e; ! false; echo after'
check "!false&&echo"       'set -e; ! false && echo x; echo after'

# --- real-world idioms under set -e ---
check "grep-q-notfound"    'set -e; printf "hi\n" | grep -q zz && echo Y; echo after'
check "grep-q-found"       'set -e; printf "hi\n" | grep -q hi && echo Y; echo after'
check "cmd-v-missing"      'set -e; command -v nope >/dev/null && echo has || echo missing; echo after'
check "cmd-v-present"      'set -e; command -v echo >/dev/null && echo has || echo missing; echo after'

# --- ERR trap follows the SAME exemption rule ---
check "ERR:false&&echo"    'set -E; trap "echo T" ERR; false && echo x; echo after'
check "ERR:echo&&false"    'set -E; trap "echo T" ERR; echo a && false; echo after'
check "ERR:true&&false"    'set -E; trap "echo T" ERR; true && false; echo after'
check "ERR:false||echo"    'set -E; trap "echo T" ERR; false || echo x; echo after'
check "ERR:mid-fail"       'set -E; trap "echo T" ERR; true && false && echo x; echo after'
check "ERR:bare-false"     'set -E; trap "echo T" ERR; false; echo after'

# --- errexit suppressed in conditions (must stay suppressed) ---
check "if-false-cond"      'set -e; if false; then :; fi; echo after'
check "while-false-cond"   'set -e; while false; do :; done; echo after'

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
