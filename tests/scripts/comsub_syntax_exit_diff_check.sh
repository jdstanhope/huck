#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the EXIT STATUS of a syntax error
# inside a command substitution (#492).
#
# Every ordinary syntax error is status 2. A syntax error inside `$( )` is
# bash's one exception:
#
#   * under `-c` it is **127**, not 2;
#   * inside a SOURCED file it is fatal to the WHOLE shell (the caller's next
#     command does not run), where an ordinary syntax error only ends that file;
#   * from a script file or piped stdin it is 2, like everything else.
#
# The split is the error's POSITION, not its shape: an UNTERMINATED `$(` is an
# unexpected-EOF error and stays 2 everywhere, which is why the marker the
# parser adds covers only the at-a-token case.
#
# A BACKTICK body is different again — bash parses it at EXPANSION time, so its
# syntax error is not fatal at all — and is not this harness's subject; the
# `echo AFTER` rows here would fail on huck for a different reason (#576).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-comsub.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# `-c`: compare stdout+stderr+status, with the program-name prologue kept (both
# shells are given an explicit $0).
check_c() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# script file / piped stdin / sourced: the diagnostics embed the temp path, so
# compare the STATUS and stdout only (stderr is checked by the `-c` rows).
check_drivers() {
    local label="$1" frag="$2" f b h
    f="$TMPROOT/case.sh"
    printf '%s\n' "$frag" >"$f"
    b=$(
        timeout 10 "$BASH_BIN" --norc --noprofile "$f" 2>/dev/null; echo "script:$?"
        timeout 10 "$BASH_BIN" --norc --noprofile <"$f" 2>/dev/null; echo "stdin:$?"
        timeout 10 "$BASH_BIN" --norc --noprofile -c ". $f; echo AFTER" huck5 2>/dev/null; echo "source:$?"
    )
    h=$(
        timeout 10 "$HUCK_BIN" "$f" 2>/dev/null; echo "script:$?"
        timeout 10 "$HUCK_BIN" <"$f" 2>/dev/null; echo "stdin:$?"
        timeout 10 "$HUCK_BIN" -c ". $f; echo AFTER" huck5 2>/dev/null; echo "source:$?"
    )
    compare "$label" "$b" "$h"
}

# Status only.
check_status() {
    local label="$1" frag="$2" b h
    timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 >/dev/null 2>&1; b="EXIT:$?"
    timeout 10 "$HUCK_BIN" -c "$frag" huck5 >/dev/null 2>&1; h="EXIT:$?"
    compare "$label" "$b" "$h"
}

# Status + stderr, stdout dropped.
check_err() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1 1>/dev/null; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1 1>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Statuses across the four drivers, stdout and stderr dropped.
check_drivers_status() {
    local label="$1" frag="$2" f b h
    f="$TMPROOT/case.sh"
    printf '%s\n' "$frag" >"$f"
    b=$(
        timeout 10 "$BASH_BIN" --norc --noprofile "$f" >/dev/null 2>&1; echo "script:$?"
        timeout 10 "$BASH_BIN" --norc --noprofile <"$f" >/dev/null 2>&1; echo "stdin:$?"
        timeout 10 "$BASH_BIN" --norc --noprofile -c ". $f; echo AFTER" huck5 >/dev/null 2>&1; echo "source:$?"
    )
    h=$(
        timeout 10 "$HUCK_BIN" "$f" >/dev/null 2>&1; echo "script:$?"
        timeout 10 "$HUCK_BIN" <"$f" >/dev/null 2>&1; echo "stdin:$?"
        timeout 10 "$HUCK_BIN" -c ". $f; echo AFTER" huck5 >/dev/null 2>&1; echo "source:$?"
    )
    compare "$label" "$b" "$h"
}

# --- `$( )` body errors AT A TOKEN: 127 under -c ---
check_c "extra semicolon"   'echo $(echo a; ; echo b)'
check_c "bare if"           'echo $(if)'
# NOT a row: `echo $(for)`. huck raises its own descriptive error there
# ("invalid variable name in 'for' loop") where bash reports an unexpected
# token, so the message AND the status differ for a reason that has nothing to
# do with the fatality rule — #574.
check_c "bare case"         'echo $(case)'
check_c "leading semicolon" 'echo $(;)'
check_c "leading pipe"      'echo $(| )'
check_c "in an assignment"  'x=$(echo a; ; echo b)'
check_c "inside quotes"     'echo "$(echo a; ; echo b)"'
check_c "nested"            'echo $(echo $(echo a; ; echo b))'
check_c "after a command"   'echo one; echo $(echo a; ; echo b)'

# --- UNTERMINATED `$(` is an ordinary EOF error: 2 ---
check_c "unterminated bare" 'echo $('
check_c "unterminated body" 'echo $(echo a'
# Status only: huck's message for `$(()` is its own ("empty subshell") where
# bash reports unexpected-EOF (#574). The STATUS is the point here — this row
# is what pins that huck's non-`Unexpected` parse errors must NOT be marked.
check_status "unterminated pair" 'echo $(()'

# --- ordinary top-level syntax errors: 2, unchanged ---
# Status + stderr only: huck runs the leading `echo` before rejecting the line,
# so its stdout carries an `x` bash never prints (#575).
check_err "double semicolon" 'echo x;;'
check_c "bare if top"       'if'
check_c "stray paren"       'echo a )'
check_c "unterminated brace" 'echo ${'

# --- the successful forms must not move ---
check_c "valid comsub"      'echo $(echo ok)'
check_c "valid two cmds"    'echo $(echo a; echo b)'
check_c "valid nested"      'echo $(echo $(echo ok))'

# --- across the four drivers: script and stdin are 2, sourced is fatal ---
check_drivers "comsub across drivers" 'echo $(echo a; ; echo b)'
check_drivers_status "plain across drivers" 'echo x;;'
check_drivers "unterminated drivers"  'echo $('
check_drivers "valid across drivers"  'echo $(echo ok)'

harness_summary
