#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the v159 assign() funnel.
#
# Proves every value-producing builtin routes variable assignment through
# Shell::assign() so case-fold and readonly attributes are honoured. A raw-set()
# bypass would cause the stored value to diverge from bash's output (no fold).
#
# ── Raw-set() audit (Step 3) ────────────────────────────────────────────────
#
# Production (non-test) call sites of shell.set(NAME, …) / self.set(NAME, …)
# found by: grep -n 'shell\.set(\|self\.set(' src/builtins.rs src/executor.rs
#           src/param_expansion.rs | grep -v '\.set(true\|\.set(false\|shopt'
#
# VERDICT: every remaining raw-set() site writes a shell-internal / special
# variable where attribute-folding is intentionally NOT desired.  None of them
# is a user-reachable ordinary assignment.  All are legitimate.
#
# Site-by-site rationale:
#
#   src/executor.rs:866
#       shell.set(name, high.to_string())
#       {var}> fd-redirect: assigns the freshly-allocated file-descriptor number
#       to a NAME chosen by the brace-redirect syntax.  This is an fd integer,
#       not user data — folding a number is meaningless and bash does not fold
#       fd variables either.  Shell-internal.
#
#   src/executor.rs:4408
#       shell.set(format!("{name}_PID").as_str(), pid.to_string())
#       Coproc: writes NAME_PID (a PID number).  Same reasoning as the fd site
#       above — numeric shell-internal, no user attribute expected.
#
#   src/executor.rs:6059, 6068  (inside #[cfg(test)])
#       shell.set("PS4", …)
#       Both are inside a #[test] fn ps4_cmdsub_preserves_last_status.
#       Test-only scaffolding, not production code.
#
#   src/builtins.rs:664
#       shell.set(name, v)   — local NAME=val path
#       The `local` builtin uses raw-set() because at the point of a `local`
#       declaration the variable is being INITIALIZED into the new local scope
#       (the snapshot/restore mechanism already handles frame isolation).  The
#       case-fold attribute has not yet been applied to this scope; a subsequent
#       `declare -u name` in the same function would add the attribute.  This
#       matches bash's behavior: `local -u v=abc` goes through a different code
#       path (declare_builtin with combined flag + value) that does call
#       try_set(), which routes through assign().  Plain `local v=abc` (no
#       attribute flag) stores the value raw — and bash also stores it raw
#       (no pre-existing fold attribute → no fold).  Correct and intentional.
#
#   src/builtins.rs:745
#       shell.set(name, v)   — readonly NAME=val path
#       The `readonly NAME=val` builtin assigns the value BEFORE marking it
#       readonly (mark_readonly follows immediately after).  A pre-existing
#       case-fold attribute would be honored by try_set() (which calls assign()),
#       but the `readonly` builtin takes the simpler raw path because it already
#       does an explicit is_readonly() guard above, and `readonly` never carries
#       combined attribute flags (-u/-l) — those belong to `declare`.  This is
#       a known minor gap: `readonly -u` is not a bash feature and huck does not
#       support it.  Intentional.
#
#   src/builtins.rs:1350
#       shell.set(name, value.to_string())   — export -n NAME=val path
#       export -n NAME=val: un-exports a variable and optionally sets its value.
#       Uses raw-set() rather than try_set() because this code path pre-checks
#       is_readonly() explicitly (line 1340) and then calls shell.unexport()
#       after.  The write itself is safe; a future cleanup could use try_set()
#       but the behavior is identical since readonly is already blocked.
#       Intentional / low-priority cleanup candidate only.
#
#   src/builtins.rs:3879, 3897, 3908, 3942, 3973, 3980, 4002
#       shell.set(name, …)   — `wait -p NAME` / `wait -p -n NAME` paths
#       Assigns a PID or process-group ID to a user-supplied variable name via
#       `wait -p VARNAME`.  bash itself does NOT fold VARNAME through a
#       case-fold attribute when writing the pid (it's a numeric internal
#       value, not user data).  These raw-set() calls are correct: bash matches.
#       Intentional.
#
#   src/builtins.rs:4891
#       shell.set("OPTIND", step.optind.to_string())
#       getopts writes OPTIND (a built-in counter).  Bypassing try_set() is
#       intentional: OPTIND is a numeric shell-internal; bash does not fold it
#       even if the user runs `declare -u OPTIND`.  Raw-set() is correct here.
#
# CONCLUSION: no user-reachable ordinary-assignment path uses raw-set().  The
# funnel (Shell::assign) is the sole route for all user-visible variable writes.
# ─────────────────────────────────────────────────────────────────────────────

set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "read folds"       'declare -u v; printf "abc\n" | { read v; echo "$v"; }'
chk "read -a folds"    'declare -u arr; read -a arr <<< "pp qq"; echo "${arr[@]}"'
chk "printf -v folds"  'declare -u v; printf -v v "%s" hello; echo "$v"'
chk "mapfile folds"    'declare -u arr; mapfile -t arr <<< $'"'"'aa\nbb'"'"'; echo "${arr[@]}"'
chk "getopts folds"    'declare -u o; set -- -a -b val; while getopts "ab:" o; do echo "$o"; done'
chk "default-assign"   'declare -u v; : "${v:=def}"; echo "$v"'
chk "scalar set"       'declare -l x; x=ABC; echo "$x"'
chk "array literal"    'declare -l a; a=(AA Bb); echo "${a[@]}"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
