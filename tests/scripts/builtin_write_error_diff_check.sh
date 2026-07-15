#!/usr/bin/env bash
# v298 (#137): a builtin whose stdout write fails (closed fd, full disk) must
# report `<name>: write error: <strerror>` and exit 1, matching bash. Rust's
# line-buffered io::stdout() defers the write(2) to flush, so the builtins'
# own write_all checks miss it; the run_builtin_with_redirects epilogue flush
# is the authoritative detection site. Compares stdout+stderr+rc byte-identically.
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

check 'echo-closed'    'exec >&-; echo end'
check 'printf-closed'  'exec >&-; printf "end\n"'
check 'echo-n-closed'  'exec >&-; echo -n end'
check 'echo-redir-dup' 'echo hi >&-'
# fd 1 open -> no write error, plain success (guards against false positives)
check 'echo-ok'        'echo hi'

if [ $FAIL -ne 0 ]; then echo "builtin_write_error_diff_check FAILED" >&2; exit 1; fi
echo "builtin_write_error_diff_check OK"
