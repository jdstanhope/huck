#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v326 (#262): honoring the
# DebugDecision (extdebug SkipCommand / status-2 ReturnFromSub) at the five
# compound-header DEBUG fire sites added in v324/v325 (for/select/case
# per-iteration or entry header, and arith-for init/cond/step).
#
# huck does NOT implement $BASH_COMMAND, so every fragment below drives the
# DEBUG action off a plain COUNTER (n, incremented once per DEBUG fire)
# instead — both shells support that. Each counter value was determined by
# first tracing bash's own fire order with a verbose `echo "D$n"` action
# (see the v326 task-1 report for the derivation), then picking the n that
# lands on the intended fire. Every check below is expected to PASS
# (bash == huck) after the fix.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- for-header skip: every header fire skips -> whole loop skipped -----
# Fire order (single-statement body): n1=header(i1) n2=body(i1) n3=header(i2)
# n4=body(i2) n5=header(i3) n6=body(i3) n7=after. Skipping is a `continue`
# (bash execute_cmd.c: `if (debugging_mode && retval) continue;`), so a
# skip on EVERY header (n<=3 covers all 3 header fires, since a skipped
# iteration has no body-fire) skips all 3 iterations; n=4 (after's own
# fire) is untouched so "after" still runs.
check "for-header skip whole loop" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n -le 3 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'

# --- for-header skip on a later iteration -> remaining also skipped -----
# Skipping iter2's header (n=3) means x is never rebound (stays "1" from
# iter1), so if the trigger were a stale-variable check it would cascade
# forever; reproduced here directly with n==3 (iter2 header) and n==4
# (iter3 header) both skipped, while "after"'s own fire (n=5) is untouched.
check "for-header skip later iter (remaining skipped)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 3 || $n == 4 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'

# --- for-body skip (v322 regression guard, NOT a header fire) -----------
# n=4 is iter2's BODY fire (header n3 already ran normally, so x=2 IS
# bound); only "echo b2" is skipped, then the loop continues to iter3.
check "for-body skip (v322 regression)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 4 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'

# --- select-header skip: entry fire (single input line -> one iteration) -
check "select-header skip" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 1 ]] && return 1; return 0; }; trap tr DEBUG; select x in a b; do echo $x; break; done <<< 1; echo after'

# --- case-header skip: entry fire (case is not a loop) ------------------
check "case-header skip" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 1 ]] && return 1; return 0; }; trap tr DEBUG; case a in a) echo m;; esac; echo after'

# --- arith-for init skip: init eval skipped, loop still runs (i defaults
# to unset/empty -> 0 in arithmetic context) ------------------------------
check "arith-for init skip (loop runs)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 1 ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; done; echo after'

# --- arith-for cond skip: cond treated as false (0) -> loop exits --------
check "arith-for cond skip (loop exits)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 2 ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; done; echo after'

# --- arith-for step skip: step eval skipped once -> i doesn't advance for
# that one iteration (echo b0 repeats), then resumes normally. Naturally
# bounded (only one skip), no safety-break needed. --------------------------
check "arith-for step skip (loop continues)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 4 ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; done; echo after'

# --- ReturnFromSub: status 2 inside a function simulates `return 2` -----
# extdebug implies functrace (bash shell.c: shopt_set_debug_mode sets
# function_trace_mode = debugging_mode), so DEBUG fires inside f() too.
# n=4 is the first for-header fire inside f (n1..n3 cover the outer `f`
# call's own fire + entering the function + "echo pre").
check "ReturnFromSub (return 2 in fn)" \
  'shopt -s extdebug; n=0; tr(){ n=$((n+1)); [[ $n == 4 ]] && return 2; return 0; }; trap tr DEBUG; f(){ echo pre; for x in 1 2; do echo b$x; done; echo post; }; f; echo "rc=$?"'

# --- no extdebug: a non-zero DEBUG action status must NOT skip anything -
check "no-extdebug: non-zero status does not skip" \
  'n=0; tr(){ n=$((n+1)); [[ $n == 1 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2; do echo b$x; done; echo after'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
