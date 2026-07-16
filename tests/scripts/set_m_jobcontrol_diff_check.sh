#!/usr/bin/env bash
# #167: `set -m` activates job control non-interactively so scripted fg/bg on a
# live job return the job's real status (rc 0) and completed background jobs emit
# an async completion notice, matching bash 5.2.21.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first" >&2; exit 1; }
FAIL=0
# norm() strips shell-specific `line N:` prefixes AND canonicalizes the async
# job-completion notice line: huck and bash pad the `[N]<flag> State` columns
# differently and huck appends a trailing ` &` (a PRE-EXISTING notification_line
# format divergence that also shows in interactive mode / `jobs` — see
# job_command_line_diff_check.sh's identical "Both shells pad differently" note;
# tracked separately from #167). We collapse the padding and drop the trailing
# ` &` on any `[N]…` notice line so this harness tests #167's behavior — that the
# notice is EMITTED under `set -m` with the right command — not the shared
# padding cosmetics.
norm() { sed -E -e "s#$HUCK: line [0-9]*: #SH: #g" -e 's#bash: line [0-9]*: #SH: #g' \
             -e '/^\[[0-9]+\]/{ s/ +/ /g; s/ &$//; }'; }
check() { local b h; b=$( { bash -c "$2"; echo "rc=$?"; } 2>&1 | norm); h=$( { "$HUCK" -c "$2"; echo "rc=$?"; } 2>&1 | norm)
  if [ "$b" != "$h" ]; then echo "FAIL [$1] bash=[$(printf %s "$b"|tr '\n' '|')] huck=[$(printf %s "$h"|tr '\n' '|')]"; FAIL=1; else echo "PASS [$1]"; fi }
check fg_live   'set -m; sleep 0.2 & fg %1; echo done'
check fg_rc     'set -m; sleep 0.2 & fg %1; echo rc=$?'
check bg_then   'set -m; sleep 0.2 & bg %1 2>/dev/null; wait; echo ok'
if [ $FAIL -ne 0 ]; then echo "set_m_jobcontrol_diff_check FAILED" >&2; exit 1; fi
echo "set_m_jobcontrol_diff_check OK"
