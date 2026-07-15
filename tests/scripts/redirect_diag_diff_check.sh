#!/usr/bin/env bash
# v297 (#152/#140): redirect-diagnostic MESSAGE divergences from bash.
# - #152: ambiguous-redirect message omits the offending word (the command
#   substitution / expansion result) that bash names in its error.
# - #140: `{var}` named-fd redirect errors diverge in several ways: a
#   double-message on `{v}>&badfd`, `>&$v` echoing the literal word instead of
#   the resolved fd number, an unset `$v` dup-source producing the wrong
#   message, and external/pipeline source-order visibility of `$v`.
# Compares stdout+stderr byte-identically (mirrors the house style of
# pipeline_stage_redirect_fail_diff_check.sh).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Normalise both shells' own error prefix (`bash: line N:` / `bash: ` /
# `<huckpath>: line N:` / `<huckpath>: `) to a uniform `SH:` so only the
# libc/diagnostic message text is compared. NOTE: bash's `{var}>&badfd`
# double-message first line has NO `line N:` — hence both forms per shell.
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag" 2>&1 | norm)
  h=$(timeout 10 "$HUCK" -c "$frag" 2>&1 | norm)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# #152 -- name the offending word in ambiguous redirect
check 'amb-out' 'cat >$(echo a b)'
rm -f f 2>/dev/null
check 'amb-in'  'cat <$(echo a b)'
rm -f f 2>/dev/null

# #140a -- {var}>&badfd double message
check 'var-dup-bad' '{v}>&9'

# #140b -- >&$v echoes the literal word, not the resolved number
check 'var-echo-word' 'exec {v}>f; exec {v}>&-; echo x >&$v'
rm -f f 2>/dev/null

# #140c -- dup source $v unset -> "$v: ambiguous redirect"
check 'amb-unset'    '2>&$v {v}>f'
rm -f f 2>/dev/null
check 'amb-unset-pl' '2>&$v {v}>f | cat'
rm -f f 2>/dev/null

# #140d -- external/pipeline source-order $v-visibility: assign-then-use
# succeeds (NOTE: the ordering pair below pins the source-order rule --
# use-before-assign errors, assign-then-use succeeds -- both must be present)
check 'ext-vis-true' 'true {v}>f 2>&$v | cat'
rm -f f 2>/dev/null
check 'ext-vis-echo' 'echo hi {v}>f 2>&$v | cat'
rm -f f 2>/dev/null

if [ $FAIL -ne 0 ]; then echo "redirect_diag_diff_check FAILED" >&2; exit 1; fi
echo "redirect_diag_diff_check OK"
