#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for v79 `break N` / `continue N`
# loop levels and the bash-style "outside loop" diagnostic. Each
# fragment runs through `bash` and `huck` via stdin (huck has no -c
# flag); outputs must be byte-identical after normalising the shell-name
# prefix in error messages (bash emits "bash: line N: CMD:" while huck
# emits "huck: CMD:" — only the prefix differs, not the message text).

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

# Run a fragment through a shell, normalise error-message prefixes, and
# append "EXIT:N" so the caller can diff combined output+exit in one string.
run_normalized() {
    local shell="$1"
    local fragment="$2"
    local combined exit_code
    combined=$(printf '%s\n' "$fragment" | "$shell" 2>&1)
    exit_code=$?
    # Normalise "bash: line N: CMD:" and "huck: CMD:" to "SHELL: CMD:"
    printf '%s\n' "$combined" \
        | sed -E 's#^([^:]*/)?bash: (line [0-9]+: )?#SHELL: #g; s#^([^:]*/)?huck: (line [0-9]+: )?#SHELL: #g'
    printf 'EXIT:%d\n' "$exit_code"
}

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(run_normalized bash "$fragment")
    huck_out=$(run_normalized "$HUCK_BIN" "$fragment")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(printf '%s\n' "$bash_out") <(printf '%s\n' "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. break 2 in nested for — prints only the first inner item then exits both.
check "break 2 nested for" \
      'for i in 1 2; do for j in a b; do echo $i$j; break 2; done; echo outer$i; done; echo done'

# 2. continue 2 in nested for — skips to the next outer iteration.
check "continue 2 nested for" \
      'for i in 1 2 3; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo $i$j; done; echo outer$i; done; echo done'

# 3. break overshoot caps to depth — exits both loops, trailing echo runs.
check "break overshoot cap" \
      'for i in 1 2; do for j in a b; do break 999; echo body$i$j; done; echo outer$i; done; echo ok'

# 4. break outside loop, WITH a following command — diagnostic, $?=0, script continues.
check "break outside loop w/ follow-on" \
      'break; echo after; echo rc=$?'

# 5. continue outside loop, WITH a following command.
check "continue outside loop w/ follow-on" \
      'continue; echo after; echo rc=$?'

# 6. break 0 nested WITH body after + trailing echo — break-all (only `done` prints).
check "break 0 nested break-all" \
      'for i in 1 2; do for j in a b; do break 0; echo body$i$j; done; echo outer$i; done; echo done'

# 7. break 0 then probe $? — out-of-range leaves $?=1.
check "break 0 status probe" \
      'for i in 1; do break 0; done; echo rc=$?'

# 8. continue 0 nested WITH body + trailing echo — break-all like bash, $?=1.
check "continue 0 nested break-all" \
      'for i in 1 2; do for j in a b; do continue 0; echo body$i$j; done; echo outer$i; done; echo done; echo rc=$?'

# 9. break with non-numeric arg WITH follow-on — aborts whole script, exit 128 (after must NOT print).
check "break abc whole-script abort" \
      'for i in 1 2; do break abc; echo b$i; done; echo after'

# 10. too-many-args break WITH body + $? probe — break-all, $?=1, script continues.
#     NOTE: the `echo rc=$?` MUST be on a SEPARATE line from the loop. bash
#     treats a special-builtin usage error ("too many arguments") as a
#     usage error that discards the REST OF THE CURRENT INPUT LINE, so a
#     same-line `; echo rc=$?` would not run under bash (huck does not model
#     that line-granularity discard — see docs/bash-divergences.md). On its
#     own line the trailing probe runs identically in both shells.
check "break too many args break-all" \
      'for i in 1 2; do for j in a b; do break 1 2 3; echo body$i$j; done; echo outer$i; done
echo rc=$?'

# 11. too-many-args nested with continue (also separate-line probe; see #10).
check "continue too many args break-all" \
      'for i in 1 2; do for j in a b; do continue 9 9; echo body$i$j; done; echo outer$i; done
echo rc=$?'

# 12. negative arg — break-all, $?=1.
check "break -1 break-all" \
      'for i in 1 2; do for j in a b; do break -1; echo body$i$j; done; echo outer$i; done; echo done; echo rc=$?'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
