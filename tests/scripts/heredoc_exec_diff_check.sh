#!/usr/bin/env bash
# v307 (#169): `exec` with a heredoc/here-string body larger than the pipe
# buffer must NOT hang. huck used to install the heredoc pipe's read end on the
# target fd and then synchronously reap the forked writer — but a PERMANENT
# (`exec`) redirect has no reader until a LATER command, so a >64KB writer was
# still blocked on a full pipe and waitpid never returned.
#
# Every case wraps BOTH shells in `timeout` so a re-introduced HANG fails the
# gate (timeout -> mismatched output) instead of wedging CI.
#
# Deliberately NOT tested here: which fd TYPE the body lands on. bash uses a
# pipe <=64KB and an unlinked temp file above it, but the path differs per
# process, so `readlink /proc/$$/fd/3` could never be byte-identical. That check
# lives in the unit layer (executor/heredoc_body_tests.rs) via fstat.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

# Bodies straddling bash's 65536-byte HEREDOC_PIPESIZE boundary. Kept as shell
# fragments so both shells build them identically.
BIG='"$(head -c 70000 /dev/zero | tr "\0" x)"'
AT='"$(head -c 65535 /dev/zero | tr "\0" x)"'    # +newline = 65536 -> pipe
OVER='"$(head -c 65536 /dev/zero | tr "\0" x)"'  # +newline = 65537 -> temp file

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

# --- #169 proper: exec + big here-string, read by a LATER command.
check 'exec-herestring-rc'     "exec 3<<<$BIG; echo rc=\$?"
check 'exec-herestring-head'   "exec 3<<<$BIG; head -c5 <&3"
check 'exec-herestring-wc'     "exec 3<<<$BIG; wc -c <&3"
# --- the same via a heredoc body rather than a here-string.
check 'exec-heredoc-wc'        "V=\$(head -c 70000 /dev/zero | tr '\0' x); exec 3<<EOF
\$V
EOF
wc -c <&3"
# --- boundary: at (pipe) and just over (temp file) bash's 65536 herelen.
check 'exec-at-boundary'       "exec 3<<<$AT; wc -c <&3"
check 'exec-over-boundary'     "exec 3<<<$OVER; wc -c <&3"
# --- a small body must keep working (pipe path, no regression).
check 'exec-small'             "exec 3<<<hi; cat <&3"
# NOT tested here: writing to the heredoc fd (`echo x >&3`). The fd IS read-only
# on both paths — bash's fdinfo shows O_RDONLY and the kernel returns EBADF, and
# v307's unit tests (executor/heredoc_body_tests.rs) assert that EBADF directly.
# But huck's BUILTIN write path swallows the error and returns rc 0 where bash
# prints `write error: Bad file descriptor` and returns 1 — a PRE-EXISTING gap,
# unrelated to heredocs (it reproduces with `exec 3</etc/hostname; echo x >&3`)
# and orthogonal to #169. Tracked as #186.
# --- several exec heredocs live in one shell at once.
check 'exec-multi-fd'          "exec 3<<<$BIG; exec 4<<<$BIG; head -c3 <&3; head -c3 <&4; echo"
# --- exec heredoc + a partial read, then the shell exits (no lingering writer).
check 'exec-partial-read'      "exec 3<<<$BIG; head -c5 <&3; echo done"

if [ $FAIL -ne 0 ]; then echo "heredoc_exec_diff_check FAILED" >&2; exit 1; fi
echo "heredoc_exec_diff_check OK"
