#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `-n` / `set -n` (noexec / parse-only).
#
# Compares STDOUT + EXIT CODE (stderr suppressed): parse-only's observable
# contract is "did anything run (stdout) and what exit code" — a valid script
# runs nothing (empty stdout, rc 0); a syntax error exits 2. The syntax-error
# DIAGNOSTIC text legitimately differs (huck's parser messages never byte-match
# bash's), so stderr is intentionally not compared here.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# With the -n flag (parse-only).
chkn() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -n -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -n -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# Without the flag (exercises `set -n` taking effect mid-script).
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- valid input: parses clean, runs nothing (empty stdout), rc 0 ---
chkn "valid simple"      'echo SHOULD_NOT_RUN'
chkn "valid for"         'for i in 1 2 3; do echo "$i"; done'
chkn "valid if"          'if true; then echo x; else echo y; fi'
chkn "valid func"        'f(){ local v=1; echo "$v"; }; f'
chkn "valid case"        'case $z in a) echo a;; *) echo def;; esac'
chkn "valid while"       'while read l; do echo "$l"; done'
chkn "valid coproc"      'coproc cat; echo "${COPROC[0]}"'
chkn "valid pipeline"    'echo a | tr a-z A-Z | cat'
chkn "valid subshell"    '( cd /tmp && echo here )'
chkn "valid redirects"   'echo x >/tmp/zz 2>&1; exec 3<&-'

# --- invalid input: syntax error, rc 2 (stderr wording differs, not compared) ---
chkn "err if-then"       'if then'
chkn "err for-no-in"     'for x in'
chkn "err lone done"     'done'
chkn "err unbalanced ("  '( echo unbalanced'
chkn "err case open"     'case x in'
chkn "err unterminated " 'echo "open quote'

# --- set -n taking effect mid-script (no -n flag) ---
chk  "set -n stops after"   'echo a; set -n; echo b; echo c'
chk  "set -n then +n stays" 'set -n; set +n; echo hi'
chk  "no set -n runs all"   'echo one; echo two'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
