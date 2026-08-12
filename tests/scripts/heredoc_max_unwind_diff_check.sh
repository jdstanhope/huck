#!/usr/bin/env bash
# Byte-identical bash<->huck harness for how far a HEREDOC_MAX overflow unwinds
# (#340).
#
# Exceeding bash's here-document limit (17 on one command) is the ONE lex error
# bash treats as fatal to the WHOLE shell rather than to the parse context that
# raised it. huck made it fatal to the current parse driver only, so a sourced
# file or an `eval` reported it and the caller carried on:
#
#     . many-heredocs.sh; echo OUTER
#     bash: (diagnostic), rc 1, no OUTER      huck: OUTER, rc 0
#
# At the TOP level — a script file, `-c`, piped stdin — it is status 2 and the
# shell ends there; both shells already agreed, and those rows are here so the
# nested fix cannot change them.
#
# An ORDINARY syntax error in the same positions must stay non-fatal to the
# caller: `. badsyntax.sh; echo OUTER` prints OUTER in both. Those are the
# control rows.
#
# stdout + STATUS only. The diagnostic itself is not compared: it embeds the
# temp path, and under `eval` huck reports the line of the last heredoc body
# where bash reports the command's own line (#592).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-hdmax.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# 17 here-documents on ONE command — one past bash's limit.
{
    printf 'cat'
    for i in $(seq 0 16); do printf ' <<E%d' "$i"; done
    printf '\n'
    for i in $(seq 0 16); do printf 'x%d\nE%d\n' "$i" "$i"; done
} >"$TMPROOT/many.sh"
# `if` alone: a syntax error with NO partial execution. `echo ;;` would have
# done, except huck runs the leading `echo` before rejecting the line (#575),
# which is a different divergence and would show up as a stray blank line.
printf 'if\n' >"$TMPROOT/badsyn.sh"

check() {
    local label="$1" frag="$2" b h
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" 2>/dev/null; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" -c "$frag" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check_file() {
    local label="$1" file="$2" b h
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile "$file" 2>/dev/null; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" "$file" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check_stdin() {
    local label="$1" file="$2" b h
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile <"$file" 2>/dev/null; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" <"$file" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- NESTED: fatal to the whole shell, status 1 ---
check "sourced"            '. many.sh; echo OUTER'
check "sourced in a func"  'f(){ . many.sh; echo INNER; }; f; echo OUTER'
check "eval"               'eval "$(cat many.sh)"; echo AFTER'
check "eval in a func"     'f(){ eval "$(cat many.sh)"; echo INNER; }; f; echo OUTER'
check "sourced twice"      '. many.sh; . many.sh; echo OUTER'
check "sourced then trap"  'trap "echo EXITTRAP" EXIT; . many.sh; echo OUTER'

# --- TOP LEVEL: status 2, and already agreed ---
check_file  "script file"  many.sh
check_stdin "piped stdin"  many.sh
check "-c string"          "$(cat "$TMPROOT/many.sh")
echo AFTER"

# --- controls: an ordinary syntax error stays non-fatal to the caller ---
check "sourced syntax err" '. badsyn.sh; echo OUTER'
check "eval syntax err"    'eval "if"; echo AFTER'
check "func sourced synerr" 'f(){ . badsyn.sh; echo INNER; }; f; echo OUTER'
# ...and 16 here-documents (the limit itself) is not an error at all.
check "sixteen is fine"    'cat <<A <<B <<C <<D <<E <<F <<G <<H <<I <<J <<K <<L <<M <<N <<O <<P
a
A
b
B
c
C
d
D
e
E
f
F
g
G
h
H
i
I
j
J
k
K
l
L
m
M
n
N
o
O
p
P
echo AFTER'

harness_summary
