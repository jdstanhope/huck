#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v85 `!` pipeline negation.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check "bang false"        '! false; echo $?'
check "bang true"         '! true; echo $?'
check "bang if"           'if ! false; then echo yes; fi'
check "bang while"        'while ! true; do echo x; done; echo done'
check "bang and"          '! false && echo ran'
check "bang pipeline ps"  '! false | true; echo "$? ${PIPESTATUS[@]}"'
check "bang errexit"      'set -e; ! true; echo survived'
check "bang pipefail"     'set -o pipefail; ! false | true; echo $?'
check "bang brace"        '! { false; }; echo $?'
check "bang subshell"     '! (exit 3); echo $?'
check "double bang"       '! ! false; echo $?'
check "test arg bang"     '[ ! -e /nonexistent ]; echo $?'
harness_summary
