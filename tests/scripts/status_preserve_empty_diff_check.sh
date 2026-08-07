#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #332: a top-level line with no command
# (blank, comment-only, whitespace) leaves `$?` untouched; bash only updates
# `$?` when a command actually runs. Fragments are fed through PIPED STDIN
# (the top-level reader path this bug lives in) — NOT `-c`, which already
# preserves via the sourced-contents driver.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Core repro: a comment / blank line between a failing command and `echo $?`.
check "comment preserves"   $'false\n# comment\necho $?'
check "blank preserves"     $'false\n\necho $?'
check "whitespace preserves" $'false\n   \necho $?'
check "success preserved"   $'true\n\necho $?'
check "many blanks"         $'false\n\n\n\necho $?'
check "comment after true"  $'true\n# note\necho $?'
# Leading no-command lines: initial $? is 0.
check "leading comment"     $'# header\necho $?'
check "leading blank"       $'\necho $?'
# A real command between still updates it.
check "cmd updates"         $'false\n# c\ntrue\necho $?'
# Comment-only line at EOF then nothing (rc reflects the last real command).
check "trailing comment"    $'false\n# tail'
check "trailing blank"      $'false\n'

harness_summary
