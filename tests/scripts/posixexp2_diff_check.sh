#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the ${...}-operand backslash roots fixed
# in v347 (#337): Root A `\}` escapes the `}` delimiter (drop the backslash) in a
# double-quoted operand; Root B `\<newline>` is a line continuation (both bytes
# removed) in a ${...} operand. No external helper needed.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# ── Root A: `\}` escapes the brace delimiter in a double-quoted operand ──
check "A dq brace-escape"       'x=1; echo "${x+\}z}"'
check "A dq brace mid"          'x=1; echo "${x+a\}b}"'
check "A dq two braces"         'x=1; echo "${x+\}\}}"'
# KEEP (already correct)
check "A unquoted already ok"   'x=1; echo ${x+\}z}'
check "A non-delim kept"        'x=1; echo "${x+\p}"'
check "A special dropped"       'x=1; echo "${x+a\$b}"'
check "A pattern operand"       'x=abc; echo "${x%\}}"'
check "A default form"          'x=1; echo "${x-\}z}"; unset u; echo "${u-\}z}"'

# ── Root B: `\<newline>` line-continuation in a ${...} operand ──
check "B dq continuation"       $'x=1; echo "${x+foo\\\nbar}"'
check "B unquoted continuation" $'x=1; echo ${x+foo\\\nbar}'
check "B inner-squote cont"     $'x=1; echo "${x+\'foo\\\nbar\'}"'
# KEEP
check "B bare newline kept"     $'x=1; echo "${x+foo\nbar}"'

# ── a couple of the real posixexp2 lines (set -o posix; ${IFS+...}) ──
check "real posixexp2 t9"  'set -o posix; (echo 9 "${IFS+\"\}\"z}") 2>&- || echo failed'
check "real posixexp2 t14" 'set -o posix; (echo 14 "${IFS+\}z}") 2>&- || echo failed'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
