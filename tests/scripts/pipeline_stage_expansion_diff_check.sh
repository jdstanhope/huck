#!/usr/bin/env bash
# Byte-identical bash<->huck harness for WHEN a pipeline stage expands what, and
# what a failure there costs (#753 + #760).
#
# bash forks every stage and expands it inside that child — words first, then
# redirections. Two divergences follow from huck resolving an EXTERNAL stage's
# words at SPAWN time instead, i.e. last:
#
#   * #753 — a failure in those words reached the parent. An unbound name under
#     `set -u` ended the shell; a bad substitution or an arithmetic error was
#     refused at the spawn, which had nowhere to put the refusal but an
#     `io::Error`, so `resolve failed with code 1` leaked to the user and the
#     whole pipeline was abandoned — the failed stage missing from
#     `$PIPESTATUS` entirely (`0` where bash says `1 0`).
#
#   * #760 — the ORDER. `cmd $(a) > $(b)` runs `a` before `b` in bash; huck ran
#     `b` first and, when the redirect failed, never ran `a` at all.
#
# The two are one root, so they are one harness: every row here would have been
# fixed by the same hoist.
#
# `$PIPESTATUS` is the point of most rows, because it is the only place the
# failed stage is still visible — the PIPELINE's own status is the last stage's
# and says nothing about the one that died.
#
# The LINE a stage names is here too (#755): its own line was stamped only in
# the external branch, which runs after the stdin section, so a BUILTIN stage's
# stdin error was reported at whatever line was stamped last — one early. The
# `line N` rows below put the stage on line 3 of a 3-line script, which is the
# only way to tell a right line from a stale one.
#
# NOT here: a redirect word whose expansion errored is still opened, adding
# `: No such file or directory` (#754), so the arithmetic rows use a WORD rather
# than a redirect target. Which side of the fork a REDIRECT word lands on is
# `redirect_word_fork_diff_check.sh` (#606).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# DRIVER 1 — `-c`. Pins the message text and the `-c` status substitution (127
# for the nounset kind), which the script driver does not show.
check_c() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    compare "-c: $label" "$b" "$h"
}

# DRIVER 2 — a script file with a line AFTER the fragment: the only way to see
# "this stage failed" apart from "the shell left".
check_script() {
    local label="$1" frag="$2" b h
    printf 'set -u\n%s\necho "PS=${PIPESTATUS[@]} st=$?"\necho NEXTLINE\n' "$frag" > "$TMP/s.sh"
    b=$(cd "$TMP" && timeout 10 "$BASH_BIN" --norc --noprofile s.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMP" && timeout 10 "$HUCK_BIN" s.sh 2>&1; echo "EXIT:$?")
    compare "script: $label" "$b" "$h"
}

check() {
    check_c "$1" "$2"
    check_script "$1" "$2"
}

# DRIVER 3 — a script whose fragment sits on line 3, so the `line N:` prologue
# of every diagnostic is compared against a line that is neither the first nor
# the last (#755).
check_line() {
    local label="$1" frag="$2" b h
    printf 'set -u\necho FIRST\n%s\necho LAST\n' "$frag" > "$TMP/l.sh"
    b=$(cd "$TMP" && timeout 10 "$BASH_BIN" --norc --noprofile l.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMP" && timeout 10 "$HUCK_BIN" l.sh 2>&1; echo "EXIT:$?")
    compare "line: $label" "$b" "$h"
}

# --- #753: an unbound name in a stage's WORDS fails only that stage ----------
check "unbound first stage"   'cat $nope | cat'
check "unbound last stage"    'true | cat $nope'
check "unbound both stages"   'cat $nope | cat $nope'
check "unbound middle stage"  'true | cat $nope | cat'
check "unbound with redirect" 'cat $nope > /dev/null | cat'
check "unbound three stages"  'cat $nope | cat $nope | cat $nope'

# --- the other expansion errors take the same path --------------------------
check "bad subst in words"    'cat ${x!} | cat'
check "arith error in words"  'cat $((1/0)) | cat'
check "bad subst in redirect" '/bin/echo hi > ${x!} | cat'
check "bad subst last stage"  'true | cat ${x!}'

# --- a stage that fails for an ORDINARY reason is unchanged -----------------
check "missing file"          'cat < /nonexistent-xyz | cat'
check "command not found"     'notacommand-xyz | cat'
check "empty program word"    '$nope | cat'
check "false stage"           'false | cat'

# --- a single command is expanded in the PARENT either way ------------------
check "single command word"   'cat $nope'
check "single, no pipe"       'cat ${x!}'

# --- #760: words are expanded BEFORE the stage's redirections ---------------
# `$(echo REDIR >&2)` contributes nothing to the word, so the redirect target
# stays `/dev/null` and both substitutions simply announce themselves in order.
check "order, stage"          '/bin/echo $(echo WORD >&2) > /dev/null$(echo REDIR >&2) | cat'
check "order, single"         '/bin/echo $(echo WORD >&2) > /dev/null$(echo REDIR >&2)'
check "order, builtin stage"  'echo $(echo WORD >&2) > /dev/null$(echo REDIR >&2) | cat'
# With a redirect that FAILS, bash has still run the word's substitution first.
check "order, failing redir"  '/bin/echo $(echo WORD >&2) > $(echo A; echo B) | cat'
check "order, two words"      '/bin/echo $(echo ONE >&2) $(echo TWO >&2) > /dev/null | cat'

# --- the words are expanded exactly ONCE (a second is a second execution) ---
check "no double expansion"   'n=0; /bin/echo $(echo TICK >&2) | cat'
check "single, once"          '/bin/echo $(echo TICK >&2)'

# --- lastpipe: the last stage runs in the shell, so it is NOT contained -----
check_c "lastpipe builtin"    'shopt -s lastpipe; echo A | read x < $nope'

# --- #755: the line a stage names is its OWN --------------------------------
check_line "builtin stage open"  'echo hi < /nonexistent-xyz | cat'
check_line "builtin stage word"  'echo hi < $nope | cat'
check_line "external stage open" 'cat < /nonexistent-xyz | cat'
check_line "external stage word" 'cat $nope | cat'
check_line "brace group stage"   '{ echo hi; } < /nonexistent-xyz | cat'
check_line "loop stage"          'echo A | while read l; do :; done < /nonexistent-xyz'
check_line "last stage builtin"  'echo A | read x < /nonexistent-xyz'
check_line "single command"      'echo hi < /nonexistent-xyz'
check "lastpipe external"     'shopt -s lastpipe; echo A | cat $nope'

# --- the status consumers still see the stage's failure ---------------------
check "errexit"               'set -e; cat $nope | cat; echo AFTER'
check "err trap"              'trap "echo ERRTRAP" ERR; cat $nope | cat'
check "and-or"                'cat $nope | cat && echo YES || echo NO'
check "loop body"             'for i in 1 2; do cat $nope | cat; done; echo done'
check "comsub"                'v=$(cat $nope | cat); echo "v=[$v]"'
check "negated"               '! cat $nope | cat; echo "neg=$?"'

harness_summary
