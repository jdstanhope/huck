#!/usr/bin/env bash
# v298 (#137) + v308 (#186 #190 #191): a builtin whose stdout write fails must
# report `<name>: write error: <strerror>` and exit 1, matching bash — and must
# deliver NOTHING to the real stdout. Builtin stdout goes through an unbuffered
# FdWriter over raw fd 1 (v308), so the errno is faithful (io::stdout() swallows
# EBADF) and no failed bytes remain to leak; run_builtin_with_redirects'
# epilogue is the single reporter. Compares stdout+stderr+rc byte-identically.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Normalise each shell's own prefix (`bash: line N:` / `<huckpath>: line N:` /
# `<huckpath>:`) to `SH:` so only the diagnostic text + rc are compared.
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
# The frag may close fd 1 (`exec >&-`), so read the rc via fd 2, never fd 1.
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag"'; e=$?; echo "rc=$e" >&2' 2>&1 | norm)
  h=$(timeout 10 "$HUCK" -c "$frag"'; e=$?; echo "rc=$e" >&2' 2>&1 | norm)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# #191: a FAILED write must deliver nothing to the real stdout. `check` folds
# stderr into stdout, so it cannot see a leak; compare stdout ALONE. Before
# v308, io::stdout()'s LineWriter retained the failed bytes and flushed them to
# fd 1 once the redirect scope restored it.
check_stdout() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag" 2>/dev/null | od -c)
  h=$(timeout 10 "$HUCK" -c "$frag" 2>/dev/null | od -c)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label] (stdout leak)"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

check 'echo-closed'    'exec >&-; echo end'
check 'printf-closed'  'exec >&-; printf "end\n"'
check 'echo-n-closed'  'exec >&-; echo -n end'
check 'echo-redir-dup' 'echo hi >&-'
# fd 1 open -> no write error, plain success (guards against false positives)
check 'echo-ok'        'echo hi'

# --- v308 #186: an OPEN but read-only fd. bash: `<name>: write error: Bad file
# descriptor` + rc 1. huck was silent with rc 0 before v308.
RO='exec 3</etc/hostname;'
check 'ro-echo'       "$RO"' echo x >&3'
check 'ro-echo-n'     "$RO"' echo -n x >&3'
check 'ro-printf'     "$RO"' printf x >&3'
check 'ro-printf-nl'  "$RO"' printf "x\n" >&3'
check 'ro-pwd'        "$RO"' pwd >&3'
# `declare`/`export` DISCARD their own write Result (~82 such sites), so these
# pass only because the WRITER records the errno.
check 'ro-declare'    "$RO"' x=1; declare -p x >&3'
check 'ro-export'     "$RO"' export -p >&3'
# Reported once PER INVOCATION — bash prints the message twice here.
check 'ro-echo-twice' "$RO"' echo x >&3; echo x >&3'

# --- Zero bytes written: bash attempts no write(2), so it is SILENT (rc 0).
# These guard the FdWriter empty-write short-circuit; without it a zero-byte
# write(2) to a bad fd returns EBADF and huck would report where bash does not.
check 'ro-echo-empty'   "$RO"' echo -n "" >&3'
check 'ro-printf-empty' "$RO"' printf "" >&3'
check 'ro-colon'        "$RO"' : >&3'
check 'ro-true'         "$RO"' true >&3'
check 'ro-jobs-none'    "$RO"' jobs >&3'
check 'ro-cd'           "$RO"' cd /tmp >&3'
# A builtin that fails for an UNRELATED reason writes nothing to fd 1, so bash
# reports only its own error — no write error.
check 'ro-declare-nope' "$RO"' declare -p NOPE >&3'
# Control: an O_RDWR fd is writable — no error, no false positive.
check 'rw-ok'           'exec 3<>/tmp/huck-v308-rw.txt; echo x >&3'

# --- v308 #190: ENOSPC via /dev/full. The wording must NOT depend on a trailing
# newline (before v308 the newline chose the reporter and the two disagreed —
# one omitted `write error: `, the other leaked Rust's `(os error 28)`).
check 'full-echo'       'echo x > /dev/full'
check 'full-echo-n'     'echo -n x > /dev/full'
check 'full-printf'     'printf "x" > /dev/full'
check 'full-printf-nl'  'printf "x\n" > /dev/full'
check 'full-declare'    'x=1; declare -p x > /dev/full'

# --- v308 #191: no payload on the real stdout when the write failed.
check_stdout 'leak-echo'      'echo x > /dev/full'
check_stdout 'leak-echo-n'    'echo -n x > /dev/full'
check_stdout 'leak-printf'    'printf "x" > /dev/full'
check_stdout 'leak-declare'   'x=1; declare -p x > /dev/full'
check_stdout 'leak-ro-echo'   'exec 3</etc/hostname; echo x >&3'

if [ $FAIL -ne 0 ]; then echo "builtin_write_error_diff_check FAILED" >&2; exit 1; fi
echo "builtin_write_error_diff_check OK"
