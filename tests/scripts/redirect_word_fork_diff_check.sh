#!/usr/bin/env bash
# Byte-identical bash<->huck harness for an expansion error in a REDIRECTION
# WORD, and which side of the fork it lands on (#606).
#
#     set -u; cat < $nope; echo SAME    bash: reports, SAME       huck: shell EXITS
#
# The issue guessed the rule was the redirection KIND — that `<` was exempt and
# `>` was not. It is not: measuring `cat` against `echo` in both directions
# shows the discriminator is whether bash FORKED before applying the
# redirection. bash expands a simple command's WORDS in the parent and then, for
# an external command, forks and does the redirections in the CHILD. So the
# unbound-variable fatality kills that child (the command's status, 1 from a
# script and 127 under `-c`) and the shell carries straight on; for a builtin, a
# function, a brace group or `exec` there is no child and the same error ends
# the shell.
#
#     /bin/echo hi > $nope    forked  -> shell survives   (an OUTPUT redirect)
#     echo hi < $nope         builtin -> shell exits      (an INPUT redirect)
#
# Both driver lines are here because they answer different questions. `-c`
# cannot tell "abandon the list" from "exit the shell" — they look identical
# when there is no next line — so the script driver carries a line AFTER the
# fragment; and the status a contained child reports is driver-dependent (1 from
# a script, 127 under `-c`), which only the `-c` driver pins.
#
# NOT here, each its own divergence: an external pipeline stage's command WORDS
# (`cat $nope | cat`), which bash also expands in the child — fixed since, and
# covered by `pipeline_stage_expansion_diff_check.sh` (#753/#760, which also
# owns the ORDER of a stage's words against its redirections, and the line a
# stage names, #755); a redirect word whose expansion
# errored but still yields a field (`cat < $((1/0))`), which huck goes on to
# open, adding a second `: No such file or directory` (#754); a non-external
# `time cat < $nope`, where huck runs GNU `/usr/bin/time`
# rather than the reserved word (#756); and errexit's own status under `-c`,
# where bash leaves with 1 after a contained failure whose `$?` reads 127
# (#757), so the `errexit` row runs on the script driver only.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# DRIVER 1 — `-c`. Pins the message text and the status a contained child
# reports there (127, the nounset kind's `-c` substitution).
check_c() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    compare "-c: $label" "$b" "$h"
}

# DRIVER 2 — a script file with a line AFTER the fragment, which is the only
# way to see the difference between abandoning the list and leaving the shell.
check_script() {
    local label="$1" frag="$2" b h
    printf 'set -u\n%s\necho "STATUS=$?"\necho NEXTLINE\n' "$frag" > "$TMP/s.sh"
    b=$(cd "$TMP" && timeout 10 "$BASH_BIN" --norc --noprofile s.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMP" && timeout 10 "$HUCK_BIN" s.sh 2>&1; echo "EXIT:$?")
    compare "script: $label" "$b" "$h"
}

check() {
    check_c "$1" "$2"
    check_script "$1" "$2"
}

# --- an EXTERNAL command: bash forks first, so the shell survives ---
check "external read"        'cat < $nope'
check "external write"       '/bin/echo hi > $nope'
check "external append"      '/bin/echo hi >> $nope'
check "external readwrite"   'cat <> $nope'
check "external dup-in"      'cat <&$nope'
check "external herestring"  'cat <<<$nope'
check "external fd 3"        'cat 3< $nope'
check "external stderr"      '/bin/true 2> $nope'
check "external quoted word" 'cat < "$nope"'
check "external two redirs"  'cat < $nope > /dev/null'
check "not found"            'notacommand < $nope'
check "command prefix"       'command cat < $nope'

# --- other things bash runs in a child ---
check "subshell"             '( : ) < $nope'
check "subshell inside"      '( : < $nope )'
check "background"           'cat < $nope & wait'
check "pipeline stage"       'cat < $nope | cat'
check "pipeline pipestatus"  'cat < $nope | cat; echo "PS=${PIPESTATUS[@]}"'
check "builtin stage"        'echo hi < $nope | cat'
# lastpipe moves the LAST stage into the shell itself — but only when it can run
# there. A builtin last stage is not forked, so the fatality still ends the
# shell; an external one is still forked to exec, so it is still contained.
check_c "lastpipe builtin"   'shopt -s lastpipe; echo A | read x < $nope'
check "lastpipe external"    'shopt -s lastpipe; echo A | cat < $nope'

# --- no fork: the same error ends the shell in BOTH shells ---
check "builtin read"         'echo hi < $nope'
check "builtin write"        'echo hi > $nope'
check "builtin herestring"   ': <<<$nope'
check "brace group"          '{ :; } < $nope'
check "function"             'f(){ :; }; f < $nope'
check "while loop"           'while false; do :; done < $nope'
check "exec"                 'exec 3< $nope'
check "heredoc body"         $'cat <<E\n$nope\nE'

# --- the command WORDS are expanded in the parent either way ---
check "word not redirect"    'cat $nope'
check "word and redirect"    'cat $nope < /dev/null'

# --- a contained child still feeds the status consumers ---
check_script "errexit"       'set -e; cat < $nope'
check "err trap"             'trap "echo ERRTRAP" ERR; cat < $nope'
check "and-or"               'cat < $nope && echo YES || echo NO'
check "loop body"            'for i in 1 2; do cat < $nope; echo body; done'
check "comsub"               'v=$(cat < $nope); echo "v=[$v]"'

# --- an empty expansion is still an ambiguous redirect, not this ---
check "empty not unbound"    'e=; cat < $e'
check "set +u"               'set +u; cat < $nope'

harness_summary
