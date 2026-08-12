#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a redirection word whose EXPANSION
# fails (#606, message half).
#
# Under `set -u` an unset name in a redirection is reported by both shells, but
# huck printed a second diagnostic on top, naming a word that never got the
# chance to be ambiguous:
#
#     set -u; echo hi > $nope
#     bash: nope: unbound variable
#     huck: nope: unbound variable
#           $nope: ambiguous redirect        <- invented
#
# A genuinely ambiguous word — one that expands to zero fields or to several —
# still reports, and those rows are here so the suppression cannot swallow them.
#
# ONLY commands that run IN THE SHELL are compared. bash expands an EXTERNAL
# command's redirection word in the forked child, so `cat < $nope; echo SAME`
# prints SAME and exits 0 there while huck ends the shell — the fatality half of
# #606, which needs the expansion moved relative to the fork and is still open.
# Every row below therefore uses a builtin, a function, a brace group or a loop,
# where bash ends the shell too.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- nounset in a redirection word: ONE message, then the shell ends ---
check "output redirect"    'set -u; echo hi > $nope; echo SAME'
check "append redirect"    'set -u; true >> $nope; echo SAME'
check "input redirect"     'set -u; : < $nope; echo SAME'
check "stderr redirect"    'set -u; echo hi 2> $nope; echo SAME'
check "both redirect"      'set -u; echo x &> $nope; echo SAME'
check "read-write"         'set -u; : <> $nope; echo SAME'
check "exec redirect"      'set -u; exec 3< $nope; echo SAME'
check "dup source"         'set -u; echo hi >&$nope; echo SAME'
check "function body"      'set -u; f(){ :; }; f < $nope; echo SAME'
check "brace group"        'set -u; { :; } < $nope; echo SAME'
check "loop"               'set -u; while false; do :; done < $nope; echo SAME'
check "braced name"        'set -u; echo hi > ${nope}; echo SAME'
check "element of unset"   'set -u; echo hi > ${nope[0]}; echo SAME'
check "length of unset"    'set -u; echo hi > ${#nope}; echo SAME'

# --- a genuinely ambiguous word still reports ---
check "empty array"        'a=(); echo hi > ${a[@]}; echo SAME'
check "several fields"     'set -- a b; echo hi > $@; echo SAME'
check "empty variable"     'v=""; echo hi > $v; echo SAME'
check "unset without -u"   'echo hi > $nope; echo SAME'
check "empty literal"      'echo hi > ""; echo SAME'
check "dup ambiguous"      'set -- a b; echo hi >&$@; echo SAME'
check "several in input"   'set -- a b; : < $@; echo SAME'

# --- controls: redirections that work ---
check "to a real file"     'd=$(mktemp -d); echo hi > $d/f; cat $d/f; rm -rf $d'
check "dup to fd 2"        'echo hi >&2; echo SAME'
check "here string"        'v=x; read -r r <<<"$v"; echo "$r"'

harness_summary
