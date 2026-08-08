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
