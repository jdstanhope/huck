#!/usr/bin/env bash
# v303 (#142): in-process redirect appliers must restore fds (closing installed
# heredoc read ends) BEFORE reaping heredoc/here-string writers. A >64KB body
# whose reader never drains it (a later redirect fails, or the body reads only
# part) left the writer blocked on a full pipe; reaping before restore hung.
#
# Each case wraps both shells in `timeout` so a re-introduced HANG fails the
# gate (timeout → mismatched output) rather than hanging CI. Output is compared
# byte-identically after normalizing the shell-name prologue AND `line N:`: the
# COMPOUND redirect-error path drops the `line N:` prefix (a separate,
# pre-existing divergence tracked elsewhere) — orthogonal to this hang fix.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

# A here-string body larger than the pipe buffer (~64KB) so an undrained writer
# blocks. Kept as a shell fragment so both shells build it identically.
BIG='"$(head -c 70000 /dev/zero | tr "\0" x)"'

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"
    echo "  bash: $(printf '%s' "$b" | head -c 200)"
    echo "  huck: $(printf '%s' "$h" | head -c 200)"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# Error path: big heredoc installed on fd 3, then a later redirect fails.
# rc 1, "77: Bad file descriptor", NO hang.
check 'hang-compound' "{ :; } 3<<<$BIG 4>&77"      # with_redirect_scope
check 'hang-builtin'  ": 3<<<$BIG 4>&77"           # run_builtin_with_redirects
# Success path: big heredoc fully drained by the body → rc 0, full body echoed.
check 'big-drained'   "{ cat <&3; } 3<<<$BIG"
# Success path: big heredoc only PARTIALLY read, then the scope tears down —
# the writer is still blocked until restore closes the read end (#142).
check 'big-partial'   "{ head -c 5 <&3; echo; } 3<<<$BIG"

if [ $FAIL -ne 0 ]; then echo "heredoc_redirect_fail_hang_diff_check FAILED" >&2; exit 1; fi
echo "heredoc_redirect_fail_hang_diff_check OK"
