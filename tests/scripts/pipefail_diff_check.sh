#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v83 set -o pipefail + $PIPESTATUS.
# Each fragment runs through bash and huck via stdin; stdout+stderr+exit must
# be byte-identical.
#
# NOTE: "set -o | grep pipefail" is intentionally omitted from this harness.
# huck's `set -o` listing uses space-padding ({:<16}) while bash uses a tab
# between the option name and the on/off value — a pre-existing divergence
# shared by errexit/nounset (added in v69). The pipefail option itself is
# covered by the pipefail_listed_in_shell_options unit test in src/builtins.rs.
set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$label"
        PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# 1. PIPESTATUS array after a 3-stage pipeline.
check "pipestatus multistage" \
      'true | false | true; echo "${PIPESTATUS[@]}"'

# 2. PIPESTATUS index and count.
check "pipestatus index+count" \
      'true | false | true; echo "${PIPESTATUS[1]} ${#PIPESTATUS[@]}"'

# 3. pipefail off (default): exit status = last stage.
check "pipefail off rc" \
      'false | true; echo $?'

# 4. pipefail on: exit status = rightmost non-zero stage.
check "pipefail on rc" \
      'set -o pipefail; false | true; echo $?'

# 5. pipefail on: rightmost non-zero (last stage non-zero, earlier also non-zero).
# Note: (exit 2) | (exit 3) fails in huck when preceded by a semicolon on the
# same line (pre-existing parse gap with subshell | subshell after `;`).
# false | (exit 3) exercises the same rightmost-non-zero logic byte-identically.
check "pipefail on rightmost" \
      'set -o pipefail; false | (exit 3); echo $?'

# 6. pipefail on: all-zero pipeline still exits 0.
check "pipefail on allzero" \
      'set -o pipefail; true | true; echo $?'

# 7. PIPESTATUS after a simple command.
check "pipestatus simple" \
      'false; echo "${PIPESTATUS[@]}"'

# 8. PIPESTATUS transparency: if-condition pipeline writes it; compound does not reset.
check "pipestatus if-cond" \
      'if false; then :; fi; echo "${PIPESTATUS[@]} rc=$?"'

# 9. PIPESTATUS transparency: last pipeline inside a for-body is visible outside.
check "pipestatus for-body" \
      'for i in 1; do true | false; done; echo "${PIPESTATUS[@]}"'

# 10. PIPESTATUS transparency: brace group passes through inner pipeline status.
check "pipestatus brace" \
      '{ true | false; }; echo "${PIPESTATUS[@]}"'

# 11. Subshell is one forked unit => 1-element PIPESTATUS.
check "pipestatus subshell" \
      '(true | false); echo "${PIPESTATUS[@]}"'

# 12. Function call is opaque => PIPESTATUS = [function exit status].
check "pipestatus function" \
      'f() { true | false; }; f; echo "${PIPESTATUS[@]}"'

check "pipestatus after break" \
      'for i in 1; do true | false; break; done; echo "${PIPESTATUS[@]}"'

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
