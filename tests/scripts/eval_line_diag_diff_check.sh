#!/usr/bin/env bash
# v315 (#209): syntax error inside eval reports bash's `eval:` marker + the
# outer line where eval sits.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
snorm() { sed -E "s#^.*/[^:]+: #SH: #"; }
# Capture merged stdout+stderr (the $LINENO cases print to stdout, the error
# cases to stderr) AND the SHELL's rc — WITHOUT a pipe (a pipe to norm would make
# $? = sed's exit, not the shell's), then normalize the captured text afterward.
# -c fragment cases
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1); br=$?; b=$(printf '%s' "$b" | norm)
  h=$("$HUCK" -c "$frag" 2>&1); hr=$?; h=$(printf '%s' "$h" | norm)
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# script-file cases (eval on a specific line)
check_script() {
  local label=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1); br=$?; b=$(printf '%s' "$b" | snorm)
  h=$("$HUCK" "$f" 2>&1); hr=$?; h=$(printf '%s' "$h" | snorm)
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# --- eval: marker + line (the fix)
check 'eval-c-marker'     'eval "case esac in esac) ;; esac"'
check_script 'eval-script-line3' 'echo a' 'echo b' 'eval "case esac in esac) ;; esac"'
# --- control: a non-eval top-level syntax error still uses -c:, no eval: marker
check 'noneval-control'   'case esac in esac) ;; esac'
if [ $FAIL -ne 0 ]; then echo "eval_line_diag_diff_check FAILED" >&2; exit 1; fi
echo "eval_line_diag_diff_check OK"
