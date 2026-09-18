#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #761/#770/#124 (hub 4 of #765): every
# place the shell names a command back to the user renders it with ONE
# printer, in the style bash's `print_cmd.c` uses OUTSIDE a function
# definition — `;` joins inline, `{ }` is one line, `if`/`for`/`while` keep
# their multi-line shape — because that is the text bash gives a `$( )` body
# at parse time and the `jobs` command column. Inside `declare -f` the
# function-body style applies, but a compound's HEADER prints in the current
# context (`if true;` newline, indented, `true; then`).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$tmp" && bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}

# --- a diagnostic names a $( ) body in its normalised form (#761) -----------
check "redirect in comsub"         'echo hi > $(echo REDIR >&2)'
check "compound in comsub"         'echo hi > $(if true; then echo a; fi; echo b   c)'
check "loop in comsub"             'echo hi > $(for i in 1 2; do echo $i; done)'
check "group + and-or in comsub"   'echo hi > $(echo a; { echo b; } >/dev/null; echo c && echo d || echo e | cat)'
check "quotes kept as typed"       'echo hi > $(echo "a b" '"'"'c'"'"' \d)'
check "procsub body"               'echo hi > $(echo a) <(echo b >&2)'
check "nested comsub"              'echo hi > $(echo $(echo a >&2))'
check "unbound var names word"     'set -u; echo hi > $(echo a >&2)$nope'

# --- the jobs column is the same printer (#770) -----------------------------
jobs_row() { check "$1" "$2"' jobs; kill %1 2>/dev/null; wait 2>/dev/null'; }
jobs_row "coproc named"            'coproc X { sleep 1; };'
jobs_row "coproc default name"     'coproc { sleep 1; };'
jobs_row "coproc simple"           'coproc cat;'
jobs_row "if &"                    'if true; then sleep 1; fi &'
jobs_row "for &"                   'for i in 1; do sleep 1; done &'
jobs_row "while &"                 'while sleep 1; do :; done &'
jobs_row "nested group &"          '{ sleep 1; { sleep 1; }; } &'
jobs_row "subshell &"              '( sleep 1; sleep 1 ) &'
jobs_row "and-or &"                'sleep 1 && sleep 1 || sleep 1 &'
jobs_row "redirects &"             'sleep 1 > /dev/null 2>&1 < /dev/null &'
jobs_row "quotes &"                'sleep 1 '"'"'a b'"'"' "c" \d 2>/dev/null &'
jobs_row "assignment prefix &"     'x=1 y="a b" sleep 1 &'
jobs_row "function &"              'f() { sleep 1; }; f &'
jobs_row "case &"                  'case x in x) sleep 1;; esac &'

# --- declare -f: a header prints in the current context ---------------------
check "if header two commands"     'f() { if true; true; then echo a; fi; }; declare -f f'
check "while header two commands"  'f() { while true; false; do echo a; done; }; declare -f f'
check "group in header"            'f() { if { true; }; then echo a; fi; }; declare -f f'
check "comsub in function body"    'f() { x=$(if true; then echo a; fi; echo b; { echo c; }; echo d); }; declare -f f'
check "coproc in function body"    'f() { coproc { sleep 1; }; coproc cat; }; declare -f f'

# --- &> / &>> survive as the operator the user wrote (#124) -----------------
check "&> in declare -f"           'f() { true &>file; true &>>file; true >file 2>&1; true 2>&1 >file; }; declare -f f'
check "&> with other redirects"    'f() { true &>"$x" 3>&1 <in; }; declare -f f'
jobs_row "&> in jobs"              'sleep 1 &>/dev/null &'
check "&> in a diagnostic"         'echo hi > $(echo REDIR &>/dev/null; echo x >&2)'
check "&> in BASH_COMMAND"         'trap "echo [\$BASH_COMMAND]" DEBUG; echo hi &>/dev/null'
# and still runs as `> f 2>&1` in every shape
check "&> runs: group, append"     'cd "$(mktemp -d)"; { echo o; echo e >&2; } &>both; cat both; { echo o2; echo e2 >&2; } &>>both; cat both'
check "&> runs: noclobber"         'cd "$(mktemp -d)"; set -o noclobber; echo x > f; echo y &> f; echo rc=$?; cat f'
check "&> runs: later 2> wins"     'cd "$(mktemp -d)"; echo o &>f 2>/dev/null; cat f; ls /nope &>f; cat f'
check "&> runs: subshell, pipe, fn" 'cd "$(mktemp -d)"; ( echo o; echo e >&2 ) &>sub; cat sub; echo a | cat &>pipe; cat pipe; f() { echo fo; echo fe >&2; }; f &>fn; cat fn'
check "&> runs: exec"              'cd "$(mktemp -d)"; ( exec &>f; echo o; echo e >&2 ); cat f'

# --- $BASH_COMMAND is the same printer --------------------------------------
check "BASH_COMMAND"               'trap '"'"'echo "[$BASH_COMMAND]"'"'"' DEBUG; echo "a b" '"'"'c'"'"'; x=$(echo hi >&2); { echo g; }'

harness_summary
