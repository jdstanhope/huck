#!/usr/bin/env bash
# v301 (#161): job-control builtin usage/error wording must match bash 5.2.21.
#   - `wait <bad>`     backtick-quotes the offending argument
#   - `disown -Z`      usage string `[-h] [-ar] [jobspec ... | pid ...]`
#   - `fg -s` / `bg -s` report `-<opt>: invalid option` before the usage line,
#     with usage `fg [job_spec]` / `bg [job_spec ...]`
# fg/bg cases run under `set -m`: without job control bash short-circuits to
# `fg: no job control` before parsing options, so `set -m` is what exercises the
# option/usage wording this issue is about (a wording-only fix — huck models no
# job-control-disabled state). Compares stdout+stderr+rc byte-identically.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# wait: offending arg is backtick-quoted (`1-1' not a pid or valid job spec)
check 'wait-badspec'  'wait 1-1'
# disown: invalid option + corrected usage string
check 'disown-badopt' 'disown -Z'
# fg/bg: leading-dash arg reported as invalid option, then corrected usage
check 'fg-badopt'     'set -m; fg -s'
check 'bg-badopt'     'set -m; bg -s'
# fg: multi-char bundle reports only the first bad option char (-s, not -sx)
check 'fg-badopt-bundle' 'set -m; fg -sx'

if [ $FAIL -ne 0 ]; then echo "job_builtin_usage_diff_check FAILED" >&2; exit 1; fi
echo "job_builtin_usage_diff_check OK"
