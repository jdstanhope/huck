#!/usr/bin/env bash
# v333 (#289): $'...' ANSI-C quoting bash-diff harness — Root A (\c\\ control-char
# escape off-by-one) + Root B (heredoc value-word $'...' literal-vs-pattern) cases.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK="${HUCK_BIN:-target/debug/huck}"
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash --norc --noprofile -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

# Root A — `\c` control-char escape edge cases
check "cc backslash" "printf '%s' \$'\\c\\\\' | od -An -tx1"                          # 1c
check "cc bs then a"  "printf '%s' \$'\\c\\a' | od -An -tx1"                           # 1c 61
check "cc bs then c]" "printf '%s' \$'\\c\\\\\\c]' | od -An -tx1"                      # 1c 1d
check "cc full run"   "printf '%s' \$'\\c[\\c\\\\\\c]\\c^\\c_\\c?' | od -An -tx1"      # 1b 1c 1d 1e 1f 7f

if [ $FAIL -ne 0 ]; then echo "nquote_ansi_c_diff_check FAILED" >&2; exit 1; fi
echo "nquote_ansi_c_diff_check OK"
