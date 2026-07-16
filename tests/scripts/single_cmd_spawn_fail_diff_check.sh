#!/usr/bin/env bash
# v305 (#172): a SINGLE external command whose program can't be run must print
# `<name>: <reason>` to its OWN (redirected) fd 2, exit 126/127, and route the
# diagnostic to a `2>file` / `$(… 2>&1)` capture exactly like bash 5.2.21.
# Sibling of pipeline_stage_spawn_fail_diff_check.sh (#78/v304), which covers the
# pipeline path; this covers run_subprocess (the non-pipeline path).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

# Scratch: a non-executable regular file (126 Permission denied) and a PATH dir
# holding a bare non-executable `foo`.
NOEXEC=$(mktemp); printf '#x\n' > "$NOEXEC"; chmod 644 "$NOEXEC"
PDIR=$(mktemp -d); : > "$PDIR/foo"; chmod 644 "$PDIR/foo"
trap 'rm -rf "$NOEXEC" "$PDIR" /tmp/hk172_e' EXIT

FAIL=0
# Strip the shell-name + `line N:` prologue GLOBALLY (not just at line start): the
# 2>&1-capture case embeds the diagnostic inside `cap=[...]`, so an anchored match
# would miss it and the byte-compare would fail on the prog-name difference.
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
# Same as check() but runs the inner shell with a custom PATH via `env` (so the
# outer `timeout` still resolves normally). The inner fragment uses only builtins.
check_path() {
  local label=$1 frag=$3 b h
  # Prepend the scratch dir so `foo` matches ONLY there; keep system dirs so the
  # inner shell binary and any tools still resolve. $HUCK has a slash → env runs
  # it as a relative path; `bash` has none → env finds it via this PATH.
  local pathval="$2:/usr/bin:/bin"
  b=$( { timeout 10 env PATH="$pathval" bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 env PATH="$pathval" "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $(printf '%s' "$b" | tr '\n' '|')"; echo "  huck: $(printf '%s' "$h" | tr '\n' '|')"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

check 'notfound-plain'   'nosuchcmd; echo "rc=$?"'
check 'notfound-2>file'  'nosuchcmd 2>/tmp/hk172_e; echo "rc=$?"; echo "efile=[$(cat /tmp/hk172_e)]"'
check 'notfound-capture' 'x=$(nosuchcmd 2>&1); echo "cap=[$x] rc=$?"'
check 'directory-126'    '/etc; echo "rc=$?"'
check 'directory-2>file' '/etc 2>/tmp/hk172_e; echo "rc=$?"; echo "efile=[$(cat /tmp/hk172_e)]"'
check 'nonexec-126'      "$NOEXEC; echo \"rc=\$?\""
check 'nonexec-capture'  "x=\$($NOEXEC 2>&1); echo \"cap=[\$x] rc=\$?\""
check 'slash-notfound'   '/no/such/x; echo "rc=$?"'
# Gap 3: a bare non-executable file in PATH → 126 "Permission denied" with the
# RESOLVED path. Point PATH at the scratch dir only (bash and huck resolve `foo`
# there identically); norm strips the shell prologue but the resolved path stays.
check_path 'bare-nonexec-PATH-126' "$PDIR" 'foo; echo "rc=$?"'
check_path 'bare-nonexec-PATH-capture' "$PDIR" 'x=$(foo 2>&1); echo "cap=[$x] rc=$?"'

if [ $FAIL -ne 0 ]; then echo "single_cmd_spawn_fail_diff_check FAILED" >&2; exit 1; fi
echo "single_cmd_spawn_fail_diff_check OK"
