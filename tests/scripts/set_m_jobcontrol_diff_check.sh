#!/usr/bin/env bash
# #167: `set -m` activates job control non-interactively so scripted fg/bg on a
# live job return the job's real status (rc 0), matching bash 5.2.21. (The async
# `[N]+ Done` completion notice under non-interactive `set -m` is deferred to
# #158(b) and out of scope here — the harness drops that notice line for both
# shells; see norm() below.)
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first" >&2; exit 1; }
FAIL=0
# norm() strips shell-specific `line N:` prefixes and DROPS the async
# job-completion `[N]…` notice line for both shells: bash emits `[1]+ Done …`
# for a bg job that finishes under `set -m`, but huck defers that async notice
# (#158 b, out of scope for #167) and prunes silently non-interactively. This
# harness tests #167's deliverable — that `set -m` activates job control so
# scripted fg/bg return the job's real status — not the deferred notice.
norm() { sed -E -e "s#$HUCK: line [0-9]*: #SH: #g" -e 's#bash: line [0-9]*: #SH: #g' \
             -e '/^\[[0-9]+\]/d'; }
check() { local b h; b=$( { bash -c "$2"; echo "rc=$?"; } 2>&1 | norm); h=$( { "$HUCK" -c "$2"; echo "rc=$?"; } 2>&1 | norm)
  if [ "$b" != "$h" ]; then echo "FAIL [$1] bash=[$(printf %s "$b"|tr '\n' '|')] huck=[$(printf %s "$h"|tr '\n' '|')]"; FAIL=1; else echo "PASS [$1]"; fi }
check fg_live   'set -m; sleep 0.2 & fg %1; echo done'
check fg_rc     'set -m; sleep 0.2 & fg %1; echo rc=$?'
check bg_then   'set -m; sleep 0.2 & bg %1 2>/dev/null; wait; echo ok'
if [ $FAIL -ne 0 ]; then echo "set_m_jobcontrol_diff_check FAILED" >&2; exit 1; fi
echo "set_m_jobcontrol_diff_check OK"
