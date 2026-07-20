#!/usr/bin/env bash
# v318 (#218): process-substitution $! + assignment-RHS fd lifetime.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
# The shell-name prefix normalizes as usual; `/dev/fd/N` ALSO normalizes —
# bash allocates procsub fds from a high, decrementing sentinel (63, 62, ...)
# purely to dodge a script's own low fds, while huck takes the next fd `pipe()`
# hands back. That allocation POLICY isn't part of what these cases assert
# (permission/existence semantics after the fd's lifetime ends); only the
# `assign-plain` control's error text embeds the number, and every OTHER case
# never surfaces one at all, so normalizing here can't mask a real divergence.
norm() { sed -E -e "s#^(bash|.*/huck): #SH: #" -e "s#/dev/fd/[0-9]+#/dev/fd/N#g"; }
check() { local l=$1 f=$2 b h br hr
  b=$(bash -c "$f" 2>&1); br=$?; b=$(printf '%s' "$b" | norm)
  h=$("$HUCK" -c "$f" 2>&1); hr=$?; h=$(printf '%s' "$h" | norm)
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then echo "FAIL [$l]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
# --- Fix 1: $! from a process substitution
check 'bang-wait-status'  'cat <(exit 123) >/dev/null; wait "$!"; echo $?'
check 'bang-is-set'       'cat <(:) >/dev/null; [ -n "$!" ] && echo set || echo unset'
# --- control: $! from a real background job still works (last-writer-wins)
check 'bang-real-bg'      'cat <(:) >/dev/null; sleep 0 & p=$!; wait "$p"; echo "$?"'
# --- Fix 2: assignment-RHS fd lifetime
check 'assign-then-read'  'eval f=<(echo test4) "; cat \$f"'
check 'assign-plain'      'f=<(echo hi); cat "$f"'
# --- control: consuming-command procsub still works (per-command drain)
check 'consume-two'       'cat <(echo a) <(echo b)'
check 'consume-func'      'f2(){ cat "$1"; }; f2 <(echo x)'
# --- v318 whole-branch fix: case subject + [[ ]] operand realize AND close a
# procsub per-command (both realize; the /dev/fd/N path is real for the match).
check 'case-subject'      'case <(echo x) in /dev/fd/*) echo m;; *) echo n;; esac'
check 'bracket-exists'    '[[ -e <(echo x) ]] && echo yes || echo no'
check 'bracket-eq-fdpat'  '[[ <(echo a) == /dev/fd/* ]] && echo yes || echo no'
# --- leak / fd-stability guard: with the fd + zombie leak, a tight loop under a
# low fd ceiling exhausts fds and fails; both shells complete rc 0 "done" once
# each iteration closes its own procsub fd. (Constant fd offset between shells
# is irrelevant — only completion is asserted.)
check 'case-loop-nofdleak'    'ulimit -n 60; for i in $(seq 1 100); do case <(echo x) in *) :;; esac; done; echo done'
check 'bracket-loop-nofdleak' 'ulimit -n 60; for i in $(seq 1 100); do [[ -e <(echo x) ]]; done; echo done'
if [ $FAIL -ne 0 ]; then echo "procsub_lifetime_diff_check FAILED" >&2; exit 1; fi
echo "procsub_lifetime_diff_check OK"
