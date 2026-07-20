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

# --- $LINENO inside eval reflects the outer line (single-line eval string).
# NOTE: with a DOUBLE-quoted eval body ("echo $LINENO"), $LINENO is expanded
# by the OUTER command's own argument expansion (standard word expansion runs
# before eval ever sees its argument) — it already equals the outer line
# independent of this fix, so these two are a regression/control pair, not
# the red/green discriminator.
check 'lineno-c'          'eval "echo $LINENO"'
check_script 'lineno-script-line3' 'echo a' 'echo b' 'eval "echo $LINENO"'
# The real discriminator: a SINGLE-quoted eval body defers $LINENO expansion
# until eval re-parses and executes it — this is what actually exercises the
# current_lineno stamp sites inside eval's nested process_line_in_sinks call.
check_script 'lineno-script-eval-singlequote' 'echo a' 'echo b' 'eval '\''echo $LINENO'\'''
check 'lineno-c-singlequote' 'eval '\''echo $LINENO'\'''
# --- control: top-level $LINENO unaffected
check_script 'lineno-toplevel' 'echo a' 'echo $LINENO'

# --- v315 follow-up (#209): `eval "source badfile"` — the SOURCED file's own
# syntax error must NOT inherit the enclosing eval's `eval:` marker/line-shift
# (eval_frame is per-eval-parse, not inherited by a nested `source`/`.`; bash
# reports badfile's real name/line, no marker). This is the forward direction
# of the bug that was fixed: prior to the fix huck spuriously printed
# `<file>: eval: line 2: ...` with the WRONG (eval-shifted) echoed line.
check_eval_source() {
  local label=$1 inner b h br hr
  inner=$(mktemp)
  printf 'echo a\ncase esac in esac) ;; esac\n' > "$inner"
  b=$(bash -c "eval \"source $inner\"" 2>&1); br=$?; b=$(printf '%s' "$b" | snorm)
  h=$("$HUCK" -c "eval \"source $inner\"" 2>&1); hr=$?; h=$(printf '%s' "$h" | snorm)
  rm -f "$inner"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_eval_source 'eval-wraps-source-nomarker'

# --- reverse direction: a `source`d file whose OWN body contains `eval "bad"`
# must STILL get the `eval:` marker — the fix clears eval_frame only around
# the sourced file's own parse/exec loop, not inside a nested eval that runs
# from within it (must NOT regress; must NOT gate the fix on source_depth==0).
check_source_eval() {
  local label=$1 inner outer b h br hr
  inner=$(mktemp)
  printf 'echo a\neval "case esac in esac) ;; esac"\n' > "$inner"
  outer=$(mktemp)
  printf 'echo outer\nsource %s\n' "$inner" > "$outer"
  b=$(bash "$outer" 2>&1); br=$?; b=$(printf '%s' "$b" | snorm)
  h=$("$HUCK" "$outer" 2>&1); hr=$?; h=$(printf '%s' "$h" | snorm)
  rm -f "$inner" "$outer"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_source_eval 'source-wraps-eval-marker'

if [ $FAIL -ne 0 ]; then echo "eval_line_diag_diff_check FAILED" >&2; exit 1; fi
echo "eval_line_diag_diff_check OK"
