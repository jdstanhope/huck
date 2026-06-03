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
        | sed 's/^bash: line [0-9]*: /SHELL: /g; s/^huck: /SHELL: /g'
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

# 1. break 2 in nested for.
check "break 2 nested for" \
      'for i in 1 2; do for j in a b; do echo $i$j; break 2; done; done'

# 2. continue 2 in nested for.
check "continue 2 nested for" \
      'for i in 1 2 3; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo $i$j; done; done'

# 3. break overshoot caps to depth.
check "break overshoot cap" \
      'for i in 1; do break 999; done; echo ok'

# 4. break outside loop — error message + exit code.
check "break outside loop" \
      'break; echo $?'

# 5. continue outside loop.
check "continue outside loop" \
      'continue; echo $?'

# 6. break with non-numeric arg — exits script with status 128.
check "break abc error" \
      'for i in 1; do break abc; done; echo $?'

# 7. break with zero arg — loop continues, $?=1.
check "break 0 error" \
      'for i in 1; do break 0; done; echo $?'

# 8. break with negative arg — loop continues, $?=1.
check "break -1 error" \
      'for i in 1; do break -1; done; echo $?'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
