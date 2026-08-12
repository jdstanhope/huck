#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the ARGUMENT-LESS `-o` / `+o` command
# line flag (#164).
#
# `huck -o` with no option name after it LISTS the shell options — the same two
# forms the `set` builtin prints with no name: the `name<TAB>on|off` table for
# `-o`, the `set -o name` reinput form for `+o`. huck used to reject it as
# `-o: option requires an argument`.
#
# Only the LAST option can be name-less: anything following `-o` is taken as the
# NAME, even another `-o` or a script path, and an unknown name is still the
# ordinary `invalid option name` error. The list also prints at its own POSITION
# in the sequence, so `-o xtrace -o` shows `xtrace on`.
#
# The `emacs` row is normalised away: bash prints its listing DURING option
# parsing, before a non-interactive shell turns emacs off, so bash says `on` and
# huck says `off` for that one line — a timing divergence of its own (#583), not
# this flag's. Every other row already matches byte for byte.
#
# stdout only: the two shells buffer stdout and stderr differently, so a merged
# capture would order the xtrace line and the table by buffering rather than by
# behaviour. The error rows compare stderr on its own instead.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

norm_emacs() { sed -E -e 's/^emacs( +)\t(on|off)$/emacs\1\tEMACS/' -e 's/^set [-+]o emacs$/set ?o emacs/'; }

# stdout + status; stdin is /dev/null so nothing is read.
check_out() {
    local label="$1" b h
    shift
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile "$@" </dev/null 2>/dev/null | norm_emacs; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" "$@" </dev/null 2>/dev/null | norm_emacs; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# stdout + status, with a program on stdin, to show the flag does not eat it.
check_stdin() {
    local label="$1" prog="$2" b h
    shift 2
    b=$(printf '%s\n' "$prog" | timeout 10 "$BASH_BIN" --norc --noprofile "$@" 2>/dev/null | norm_emacs; echo "EXIT:$?")
    h=$(printf '%s\n' "$prog" | timeout 10 "$HUCK_BIN" "$@" 2>/dev/null | norm_emacs; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the listing itself ---
check_out "bare -o lists"        -o
check_out "bare +o reinputs"     +o
check_out "-o keeps its place"   -o xtrace -o
check_out "+o keeps its place"   +o xtrace +o
check_out "-o after +o"          +o allexport -o
check_out "two listings"         -o allexport -o -o

# --- the listing does not consume the program ---
check_stdin "bare -o then stdin"  'echo ran' -o
check_stdin "bare +o then stdin"  'echo ran' +o
check_stdin "-o name then stdin"  'echo ran' -o allexport

# --- a NAME still binds, and an unknown one is still an error ---
check_out "-o with a name"       -o allexport
check_out "+o with a name"       +o allexport
check_out "-o then -o is a name" -o -o
check_out "-o empty name"        -o ""
check_out "-o unknown name"      -o nosuchoption

harness_summary
