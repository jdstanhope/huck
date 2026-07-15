#!/usr/bin/env bash
# v304 (#78): an external pipeline stage whose program can't be run must print
# `<name>: <reason>` to its OWN redirected fd 2, exit 126/127, and let the
# pipeline continue with a populated PIPESTATUS — matching bash 5.2.21.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

NOEXEC=$(mktemp); printf '#x\n' > "$NOEXEC"; chmod 644 "$NOEXEC"
trap 'rm -f "$NOEXEC" /tmp/hk78_e' EXIT

FAIL=0
# Strip the shell-name + `line N:` prologue GLOBALLY (not just at line start):
# the 2>&1-capture case embeds the diagnostic inside `cap=[...]`, so an anchored
# match would miss it and the byte-compare would fail on the prog-name difference.
norm() { sed -e "s#$HUCK: line [0-9]*: #SH: #g" -e "s#$HUCK: #SH: #g" \
             -e 's#bash: line [0-9]*: #SH: #g' -e 's#bash: #SH: #g'; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $(printf '%s' "$b" | tr '\n' '|')"; echo "  huck: $(printf '%s' "$h" | tr '\n' '|')"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# The upstream is `true` (writes nothing), NOT `echo hi`: a finite-writing
# upstream feeding an instant-exiting failed stage can race SIGPIPE (141) into
# PIPESTATUS[0] in BOTH shells (the #151 pattern) — nondeterministic, so it must
# not gate this fix. The failed stage never reads the upstream anyway, so `true`
# gives identical #78 coverage with a deterministic PIPESTATUS[0]=0.
check 'notfound-last'   'true | nosuchcmd; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'notfound-middle' 'true | nosuchcmd | cat; echo "ps=${PIPESTATUS[*]}"'
check 'capture-2>&1'    'x=$(true | nosuchcmd 2>&1); echo "cap=[$x] rc=$?"'
check 'redir-2>file'    'true | nosuchcmd 2>/tmp/hk78_e; echo "rc=$?"; echo "efile=[$(cat /tmp/hk78_e)]"'
check 'nonexec-126'     "true | $NOEXEC; echo \"rc=\$? ps=\${PIPESTATUS[*]}\""
check 'directory-126'   'true | /etc; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'slash-notfound'  'true | /no/such/x; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'pipefail'        'set -o pipefail; true | nosuchcmd | cat; echo "rc=$?"'

if [ $FAIL -ne 0 ]; then echo "pipeline_stage_spawn_fail_diff_check FAILED" >&2; exit 1; fi
echo "pipeline_stage_spawn_fail_diff_check OK"
