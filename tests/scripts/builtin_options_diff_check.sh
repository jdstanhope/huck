#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the one builtin option scanner (v359,
# #496). Both shells run with an EXPLICIT $0 ("huck5") so the error prologue
# matches and this is a plain byte comparison — no normalisation, which would
# also hide real prologue bugs.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$("$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# ── the four reported bugs (#496) ──
check "readonly -pa"      'readonly -pa >/dev/null'
check "wait -fn"          'wait -fn'
check "history -cd"       'history -cd 1'
check "unset -vf"         'unset -vf x'

# ── invalid option: message, usage line, status, for every in-scope builtin ──
for b in unset readonly read type hash declare typeset printf command mapfile \
         readarray help complete compgen compopt jobs trap alias unalias builtin \
         export cd wait history getopts shopt disown umask ulimit pwd enable; do
    check "$b -Q invalid option" "$b -Q"
done
check "local -Q invalid option" 'f() { local -Q; }; f'

# ── missing-value message: `NAME: -C: option requires an argument`, NOT the
# getopt(3) `NAME: option requires an argument -- C` shape (#496 Task 5
# review: the scanner had the wrong shape, caught only because `hash -p` was
# the first `:`-spec builtin converted). `hash -p` is ON the scanner and
# pins the fixed shape (must PASS). `printf -v` still hand-rolls its own
# scan — Task 6 is the one that converts it onto this scanner — and its
# FIRST line already matches bash today; it's DELIBERATELY, EXPECTEDLY RED
# here only because its hand-rolled code is missing the second (usage) line
# entirely, a pre-existing gap unrelated to this fix. Task 6 turns this row
# green (both lines). If it goes green with the WRONG shape on line one
# instead, that's the regression this row exists to catch — leave it red
# until then, it is not a bug in this branch.
check "hash -p missing value"    'hash -p'
check "printf -v missing value"  'printf -v'

# ── hash -l/-t precedence with operand names (#496 Task 5 review, 2 rounds) ──
# `-t` (with or without `-l`) wins over a bare `-l` for reporting an
# UNHASHED name as `not found`, but when BOTH are given and the name IS
# hashed, `-l` wins the PRINT FORMAT (the reusable `-p` form, not `-t`'s
# bare-path form). A bare `-l NAME` (no `-t`) is a DIFFERENT thing again —
# not a table lookup at all, but the same fresh-$PATH-search-and-hash side
# effect as no flags at all, silent on success even when the name was
# already hashed to something else. All four shapes must hold together;
# the second review round caught the fourth only after the first round's
# fix broke it (a `hash -l ls` regression, rc 0 -> "not found").
check "hash -lt hashed name"        'hash -p /bin/ls ls; hash -lt ls'
check "hash -lt unhashed name"      'hash -lt ls'
check "hash -l unhashed resolvable" 'hash -l ls'
check "hash -l already hashed"      'hash -p /bin/ls ls; hash -l ls'
check "hash -l unresolvable"        'hash -l __hash_no_such_cmd_xyzzy__'

# ── the contract rows (huck already matches these; they must STAY matching) ──
check "bundle order -ap"      'readonly -ap >/dev/null'
check "-- terminates"         'readonly -- x=1; echo $x'
check "lone - is an operand"  'hash -'
check "stop at non-option"    'v=1; readonly v -p'
check "attached value"        'read -n3 </dev/null; echo rc=$?'
check "separate value"        'read -n 3 </dev/null; echo rc=$?'

# ── posix fatality of a special-builtin usage error (v358) ──
# readonly IS a POSIX special builtin: a bad option exits a posix shell.
check "posix readonly -Q"     'set -o posix; readonly -Q; echo SURVIVED'
check "non-posix readonly -Q" 'readonly -Q; echo SURVIVED'

# declare/typeset/local are NOT POSIX special builtins: a bad option must
# NOT exit a posix shell, even though they share the same Getopt scanner as
# readonly/export. This axis caught a real regression (#496 Task 4 review)
# where the scanner called report_error unconditionally instead of leaving
# the fatality decision to the executor's is_special_builtin-gated consume.
check "posix declare -Q"      'set -o posix; declare -Q; echo SURVIVED'
check "non-posix declare -Q"  'declare -Q; echo SURVIVED'
check "posix typeset -Q"      'set -o posix; typeset -Q; echo SURVIVED'
check "non-posix typeset -Q"  'typeset -Q; echo SURVIVED'
check "posix local -Q"        'set -o posix; f() { local -Q; }; f; echo SURVIVED'
check "non-posix local -Q"    'f() { local -Q; }; f; echo SURVIVED'

harness_summary
