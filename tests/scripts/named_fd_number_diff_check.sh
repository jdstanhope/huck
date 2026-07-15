#!/usr/bin/env bash
# v297 (#141): the fd NUMBER a shell allocates for a `{var}` named-fd redirect
# diverges from bash once other explicit fds (3, 4, ...) are already in use
# for the child -- bash always allocates the named fd starting at 10 (the
# lowest fd >= 10 not otherwise in use), while huck's allocation appears to
# be influenced by the other explicit redirects present (11/12 instead of 10).
# Linux-gated: reads /proc/self/fd of the (already-exited) child via `ls -l`
# inside the child itself, so this only works on Linux.
set -u
cd "$(dirname "$0")/../.." || exit 1
[ "$(uname)" = Linux ] || { echo "SKIP (needs /proc/self/fd)"; exit 0; }
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# List the child's own /proc/self/fd and extract the fd number whose target
# is the created file `x`. NOTE: `ls -l /proc/self/fd` (a directory arg)
# prints BARE fd numbers as the entry name (not the full `/proc/self/fd/N`
# path) -- the extraction below matches the real `ls -l` output format
# rather than a `/proc/self/fd/N` prefix.
child_named_fd() {  # $1 = shell binary, $2 = leading redirects before {v}>x
  "$1" -c "ls -l /proc/self/fd $2 {v}>x 2>/dev/null" 2>/dev/null \
    | grep -oE '[0-9]+ -> .*/x$' | grep -oE '^[0-9]+'
}
check() {
  local label=$1 redirs=$2 b h
  b=$(child_named_fd bash "$redirs")
  h=$(child_named_fd "$HUCK" "$redirs")
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: fd=$b"; echo "  huck: fd=$h"; FAIL=1
  else
    echo "PASS [$label] fd=$b"
  fi
  rm -f a b x 2>/dev/null
}

check 'bare'      ''
check '3>a'       '3>a'
check '3>a-4>b'   '3>a 4>b'

if [ $FAIL -ne 0 ]; then echo "named_fd_number_diff_check FAILED" >&2; exit 1; fi
echo "named_fd_number_diff_check OK"
