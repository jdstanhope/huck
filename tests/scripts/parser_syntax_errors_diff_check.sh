#!/usr/bin/env bash
# v331 (#27/#283): parser near-token error shapes match bash exactly for a
# handful of concrete-wrong-token cases (`for(`, a keyword-position wrong
# token) that huck previously mis-shaped as unexpected-EOF/invalid-name
# errors, plus the driver-loop fix so a non-interactive syntax error aborts
# the whole parse-context (rc 2) instead of skipping the bad line and
# resuming — matching bash. Modeled on syntax_error_diag_diff_check.sh. Part
# of the parser bash-suite category flip.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK_BIN=${HUCK_BIN:-target/debug/huck}
[ -x "$HUCK_BIN" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #"; }
check() {
  local label=$1 frag=$2 bout bexit hout hexit
  bout=$(bash --norc --noprofile -c "$frag" 2>&1); bexit=$?
  bout="$bout"$'\n'"EXIT:$bexit"
  bout=$(printf '%s' "$bout" | norm)
  hout=$("$HUCK_BIN" -c "$frag" 2>&1); hexit=$?
  hout="$hout"$'\n'"EXIT:$hexit"
  hout=$(printf '%s' "$hout" | norm)
  if [ "$bout" != "$hout" ]; then
    echo "FAIL [$label]"
    echo "  bash: [$bout]"
    echo "  huck: [$hout]"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}
check_file() {
  local label=$1 file=$2 bout bexit hout hexit
  bout=$(bash --norc --noprofile "$file" 2>&1); bexit=$?
  bout="$bout"$'\n'"EXIT:$bexit"
  bout=$(printf '%s' "$bout" | norm)
  hout=$("$HUCK_BIN" "$file" 2>&1); hexit=$?
  hout="$hout"$'\n'"EXIT:$hexit"
  hout=$(printf '%s' "$hout" | norm)
  if [ "$bout" != "$hout" ]; then
    echo "FAIL [$label]"
    echo "  bash: [$bout]"
    echo "  huck: [$hout]"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

check "case wrong-token"    'case x in in do do) :; esac'
check "for single-paren"    'for()'
# This task's driver-loop abort fix flips this case from EXIT mismatch to
# PASS: huck previously resumed and ran `true`; now it aborts at the syntax
# error like bash.
check "for-paren newline"   $'for()\ntrue'
# Guard: a missing keyword at genuine EOF (no concrete wrong token) must still
# fall through to the pre-existing "unexpected end of file" error, not the new
# near-token branch (peek_kind() is None here, so the new branch's guard
# correctly defers to the unchanged fallback). NB: the brief's originally
# proposed guard fragment, `echo $(if true; then echo hi`, was verified
# against bash and found to already diverge from huck for an unrelated,
# pre-existing reason (huck emits the generic "unexpected end of file"
# instead of bash's "unexpected EOF while looking for matching `)'" for a
# compound truncated inside a command substitution) -- orthogonal to this
# task's fix and out of scope for this iteration's four fixes. That specific
# EOF-recovery mechanism (`recover_at_eof`/`peek_is_recovery_close`) is only
# ever exercised via tab-completion's `parse_recover`, never via `-c`/script
# parsing, so it is regression-tested by huck-syntax's `recover.rs` unit
# tests (e.g. `parse_recover("if whi")`), not by this bash-diff harness.
check "if-then EOF fallback (unaffected)" 'if true; then echo hi'
check "for bad-name lineno" 'for 1x in a; do :; done'   # `line 1:` prefix, rc 1

# Driver-loop abort: a non-interactive syntax error aborts the whole
# parse-context (rc 2) instead of skipping the bad line and resuming.
check "abort multiline -c" $'echo a\nfor()\necho b'   # a; error; rc 2; NO b

TMPFILE=$(mktemp)
printf 'echo a\nfor()\necho b\n' > "$TMPFILE"
check_file "abort script-file" "$TMPFILE"             # a; error; rc 2; NO b
rm -f "$TMPFILE"

# A sourced file's remainder aborts, but the parent driver loop continues.
SUBFILE=$(mktemp)
printf 'echo in-src\nfor()\necho after-err\n' > "$SUBFILE"
check "source parent-continues" \
  "$(printf 'echo before; source %s; echo parent-after' "$SUBFILE")"
rm -f "$SUBFILE"

# Regression guards: a valid multi-line fragment still runs to completion,
# and a same-line syntax error still aborts (this already matched bash
# before this task's fix, since there is no later command to skip to).
check "valid multi still runs" $'echo x\necho y'      # x; y; rc 0
check "same-line still aborts" 'echo a; for(); echo b'

if [ $FAIL -ne 0 ]; then echo "parser_syntax_errors_diff_check FAILED" >&2; exit 1; fi
echo "parser_syntax_errors_diff_check OK"
