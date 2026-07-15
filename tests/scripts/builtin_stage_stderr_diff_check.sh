#!/usr/bin/env bash
# v298 (#144): an in-process pipeline stage must apply its OWN redirects in
# source order. `printf abc 2>&1 >f | cat` sends the error to the pipe (bash),
# not the file; huck pre-wired the file into the child's stdout base so 2>&1
# bound to the file. Verifies the fd DESTINATION (pipe vs file) for each stage
# type. Compares captured pipe output + file contents + rc + PIPESTATUS.
#
# printf's invalid-number message text diverges orthogonally (huck quotes the
# arg: `abc' vs bash's bare abc); norm() strips backticks/single-quotes so only
# WHERE the bytes land is compared, not the wording.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }
TMPF=/tmp/huck_st144.$$
trap 'rm -f "$TMPF"' EXIT

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #" \
             -e 's#`##g' -e "s#'##g"; }
# Capture the pipe/terminal output (stdout+stderr, +rc+PIPESTATUS) AND the file
# contents separately, so a byte moving from pipe->file is visible.
run_one() {
  local sh=$1 frag=$2
  rm -f "$TMPF"
  local out; out=$(timeout 10 "$sh" -c "$frag"'; echo "rc=$? PS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  local file; file=$(cat "$TMPF" 2>/dev/null | norm)
  printf 'OUT{%s}FILE{%s}' "$out" "$file"
}
check() {
  local label=$1 frag=$2 b h
  b=$(run_one bash "$frag"); h=$(run_one "$HUCK" "$frag")
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- core #144: interleaved 2>&1 >f (error must reach the pipe) ---
check 'builtin-interleave' "printf '%d\n' abc 2>&1 >$TMPF | cat"
# --- reverse order (already matched: error+0 both to file) ---
check 'builtin-fileorder'  "printf '%d\n' abc >$TMPF 2>&1 | cat"
# --- no error, plain data to file, empty pipe ---
check 'builtin-nodata'     "echo hi 2>&1 >$TMPF | cat"
check 'builtin-fileonly'   "echo hi >$TMPF 2>&1 | cat"
# --- per InProcess stage type: each must re-apply its own redirects in order ---
check 'function-stage'     "f(){ printf '%d\n' abc; }; f 2>&1 >$TMPF | cat"
check 'compound-stage'     "{ printf '%d\n' abc; } 2>&1 >$TMPF | cat"   # regression guard (already correct)
check 'assign-stage'       "x=1 2>&1 >$TMPF | cat"
# --- real open failure in a builtin stage: message parity (norm-compared) + PS ---
check 'open-fail'          "printf hi >/no/such/dir/f 2>&1 | cat"
# --- #140d {var} source-order visibility must survive the base change ---
check 'var-true'           "true {v}>$TMPF 2>&\$v | cat"
check 'var-echo'           "echo hi {v}>$TMPF 2>&\$v | cat"

if [ $FAIL -ne 0 ]; then echo "builtin_stage_stderr_diff_check FAILED" >&2; exit 1; fi
echo "builtin_stage_stderr_diff_check OK"
