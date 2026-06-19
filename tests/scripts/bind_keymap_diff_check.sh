#!/usr/bin/env bash
# bash<->huck harness for v191: bind -p/-P default-keymap honesty + format.
# huck uses rustyline (fewer functions than GNU readline), so we do NOT expect
# byte-identical full output; we assert (a) huck's keymap is a SUBSET of bash's
# (no fabricated bindings), (b) core bindings match bash's exact line, (c) user
# override/unbind behave like bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); printf 'PASS: %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL: %s\n' "$1"; shift; printf '%s\n' "$@" | sed 's/^/    /'; }

# (a) honesty: every huck `"...": func` line must be a whole line in bash bind -p
extra=$(comm -23 \
  <("$HUCK_BIN" -c 'bind -p' 2>/dev/null | grep '^"' | sort -u) \
  <(bash -c 'bind -p' 2>/dev/null | grep '^"' | sort -u))
if [[ -z "$extra" ]]; then ok "huck bind -p subset of bash"; else bad "huck has bindings bash lacks" "$extra"; fi

# (b) core bindings: huck's exact line == bash's exact line
core_check() {
    local label="$1" pat="$2" b h
    b=$(bash -c 'bind -p' 2>/dev/null | grep -F "$pat")
    h=$("$HUCK_BIN" -c 'bind -p' 2>/dev/null | grep -F "$pat")
    if [[ "$b" == "$h" && -n "$h" ]]; then ok "$label"; else bad "$label" "bash: $b" "huck: $h"; fi
}
core_check "C-a beginning-of-line" '"\C-a": beginning-of-line'
core_check "C-e end-of-line"       '"\C-e": end-of-line'
core_check "C-k kill-line"         '"\C-k": kill-line'
core_check "C-y yank"              '"\C-y": yank'
core_check "C-i complete"          '"\C-i": complete'

# (c) -P line for beginning-of-line: huck's exact line, and bash's line starts the same
hP=$("$HUCK_BIN" -c 'bind -P' 2>/dev/null | grep '^beginning-of-line can be found on')
bP=$(bash -c 'bind -P' 2>/dev/null | grep '^beginning-of-line can be found on')
if [[ "$hP" == 'beginning-of-line can be found on "\C-a".' && "$bP" == 'beginning-of-line can be found on "\C-a"'* ]]; then
  ok "-P beginning-of-line format"; else bad "-P beginning-of-line format" "bash: $bP" "huck: $hP"; fi

# (d) user override: rebinding C-a to kill-line (no space after colon — see L-note)
ov_b=$(bash -c 'bind "\"\C-a\":kill-line"; bind -p' 2>/dev/null | grep -F '"\C-a"')
ov_h=$("$HUCK_BIN" -c 'bind "\"\C-a\":kill-line"; bind -p' 2>/dev/null | grep -F '"\C-a"')
if [[ "$ov_b" == "$ov_h" && "$ov_h" == '"\C-a": kill-line' ]]; then ok "user override C-a"; else bad "user override C-a" "bash: $ov_b" "huck: $ov_h"; fi

# (e) unbind a default: C-a gone from both
ub_b=$(bash -c 'bind -r "\C-a"; bind -p' 2>/dev/null | grep -c '"\\C-a"')
ub_h=$("$HUCK_BIN" -c 'bind -r "\C-a"; bind -p' 2>/dev/null | grep -c '"\\C-a"')
if [[ "$ub_b" == "$ub_h" && "$ub_h" == 0 ]]; then ok "unbind default C-a"; else bad "unbind default C-a" "bash:$ub_b huck:$ub_h"; fi

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
