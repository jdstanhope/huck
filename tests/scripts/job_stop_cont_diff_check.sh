#!/usr/bin/env bash
# v299 (#158): kill -STOP / kill -s CONT must update a job's Running/Stopped
# state as seen by jobs/jobs -s/jobs -r/bg — matching bash. Non-interactive
# reap wiring: builtin_jobs/bg/fg drain pending WUNTRACED/WCONTINUED reports.
#
# bash also emits ASYNC job notices under `set -m` even non-interactively
# (deferred scope (b), issue #158) — so this harness does NOT byte-compare the
# whole stream. Instead each fragment absorbs the async notice with an
# intervening `:` command, prints a `===` marker, then a jobs query; we compare
# only the post-marker query, normalized to `[id] <State>` (dropping the flag,
# spacing, and the command column — the command column is the separate #80).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Everything after the `===` marker line, each job-status line reduced to
# `[<id>] <State>` (first word after the flag). Non-job lines are dropped.
# NOTE: deliberately NOT `sed '1,/^===$/d'` — that idiom mishandles the case
# where `===` is itself the first output line (GNU sed searches for a SECOND
# occurrence of the end pattern once the start address is a bare line number
# that already matched, so with no second `===` it deletes to EOF instead of
# just the marker line). That case is exactly what happens here: bash's async
# job notice does not reliably precede the marker in this environment, so
# `===` is often line 1 — the buggy idiom silently emptied BOTH sides,
# producing false PASSes that masked the pre-wiring RED. awk's explicit
# seen-flag has no such edge case.
post_marker_state() {
  awk '/^===$/{seen=1; next} seen' | sed -nE 's/^(\[[0-9]+\])[-+ ]+ *([A-Za-z]+).*/\1 \2/p'
}
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 20 bash    -c "$frag" 2>/dev/null | post_marker_state)
  h=$(timeout 20 "$HUCK" -c "$frag" 2>/dev/null | post_marker_state)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- STOP reflected: jobs -s lists the stopped job, jobs -r does not ---
check 'stop-shows-stopped' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs -s; kill -9 %1 2>/dev/null'
check 'stop-not-running'   'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
# --- CONT reflected: jobs -r lists the resumed job, jobs -s does not ---
check 'cont-shows-running' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; kill -s CONT %1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
check 'cont-not-stopped'   'set -m; sleep 30 & kill -STOP %1; sleep 1; :; kill -s CONT %1; sleep 1; :; echo ===; jobs -s; kill -9 %1 2>/dev/null'
# --- bg resumes a stopped job -> Running ---
check 'bg-resumes-running' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; bg %1 >/dev/null 2>&1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
# --- plain `jobs` shows Stopped after STOP (state token only) ---
check 'stop-plain-jobs'    'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs; kill -9 %1 2>/dev/null'

if [ $FAIL -ne 0 ]; then echo "job_stop_cont_diff_check FAILED" >&2; exit 1; fi
echo "job_stop_cont_diff_check OK"
