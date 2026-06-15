#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `bind` builtin (stdout + exit).
# Targeted greps only: huck models a 5-variable subset; bash lists ~30, so a
# whole-output `bind -v` compare would (correctly) differ. NOTE: a bogus
# function name (`bind '"\C-x":no-such-fn'`) is intentionally NOT tested for rc
# parity — bash SKIPS bind-arg validation non-interactively (rc 0) while huck
# validates (rc 1); huck's stricter behavior is a deliberate divergence.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "default editing-mode" 'bind -v | grep -E "^set editing-mode "'
chk "default bell-style"   'bind -v | grep -E "^set bell-style "'
chk "set editing-mode vi"  "bind 'set editing-mode vi'; bind -v | grep -E '^set editing-mode '"
chk "set bell-style none"  "bind 'set bell-style none'; bind -v | grep -E '^set bell-style '"
chk "set show-all on"      "bind 'set show-all-if-ambiguous on'; bind -v | grep show-all-if-ambiguous"
chk "set completion-items" "bind 'set completion-query-items 50'; bind -v | grep completion-query-items"
chk "l has accept-line"    'bind -l | grep -cx accept-line'
chk "l has beginning-of"   'bind -l | grep -cx beginning-of-line'
chk "l has kill-line"      'bind -l | grep -cx kill-line'
chk "keyseq:fn rc ok"      "bind '\"\\C-x\":kill-line'; echo rc=\$?"
chk "set var rc ok"        "bind 'set editing-mode emacs'; echo rc=\$?"
chk "unknown option rc"    'bind -Z >/dev/null 2>&1; echo rc=$?'
chk "remove binding rc"    "bind -r '\"\\C-x\"'; echo rc=\$?"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
