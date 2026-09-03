#!/usr/bin/env bash
# Byte-identical bash<->huck harness for NON-UTF-8 bytes arriving through the
# process boundary — argv and the inherited environment (#553).
#
# `std::env::args()` and `std::env::vars()` unwrap internally, so a single byte
# that is not valid UTF-8 anywhere in argv or the environment PANICKED huck
# before it ran a line:
#
#     $ env "V=$(printf 'a\xe9b')" huck -c 'echo ok'
#     thread 'main' panicked at library/std/src/env.rs:162:83
#     rc=101
#
# The environment case is the serious one: every invocation inherits its
# parent's environment, so one Latin-1 byte in an unrelated variable — a PATH
# component, an `LC_*` under a non-UTF-8 locale, anything a C program put there
# — made the shell refuse to START. Nothing a script does can prevent it.
#
# WHAT THIS ROUND FIXES, and therefore what the `check` rows below assert: the
# shell survives the boundary and runs the program. Those rows are byte-exact
# against bash.
#
# WHAT IT DOES NOT FIX: huck is not byte-transparent. `Shell` stores variable
# values as `String`, so a non-UTF-8 value is lossy-converted at the boundary
# (each bad byte becomes U+FFFD) where bash carries the raw bytes through
# untouched — into `$V`, into `${#V}`, and into the environment of a child.
# Those rows CANNOT be byte-equal until the value type is a byte string, which
# is the second half of #553 and is tracked separately; they are asserted with
# `check_rc` (exit status only, which does agree) rather than omitted, so the
# harness still pins that the shell RUNS them.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

# The offending byte. 0xE9 is a lone Latin-1 'e-acute': a valid byte in a C
# string, never a valid UTF-8 sequence on its own.
BAD=$(printf 'a\xe9b')

# --- drivers -------------------------------------------------------------
#
# Each fragment has to reach the two shells with the bad bytes INTACT, which a
# plain `-c "$frag"` cannot do for the env/argv cases — hence three drivers
# rather than the usual two, splitting by WHERE the bad byte enters.

# check_env <label> <frag> — the bad byte is an inherited environment value.
check_env() {
    local label="$1" frag="$2" b h
    b=$(env "V=$BAD" timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(env "V=$BAD" timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# check_argv <label> <frag> — the bad byte is a positional parameter.
check_argv() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 "$BAD" 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 "$BAD" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# check_rc <label> <driver> <frag> — STATUS ONLY, for the rows whose output
# cannot agree until huck carries bytes rather than `String` (see the header).
# Asserting the status still proves the shell started, ran the program and
# exited normally instead of panicking with 101.
check_rc() {
    local label="$1" driver="$2" frag="$3" b h
    case "$driver" in
        env)
            b=$(env "V=$BAD" timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 >/dev/null 2>&1; echo "EXIT:$?")
            h=$(env "V=$BAD" timeout 10 "$HUCK_BIN" -c "$frag" huck5 >/dev/null 2>&1; echo "EXIT:$?")
            ;;
        argv)
            b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 "$BAD" >/dev/null 2>&1; echo "EXIT:$?")
            h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 "$BAD" >/dev/null 2>&1; echo "EXIT:$?")
            ;;
    esac
    compare "$label" "$b" "$h"
}

# --- the shell STARTS with a bad byte in the environment ------------------
#
# The headline case. None of these fragments mentions `$V` at all: the variable
# is a bystander, exactly as it is in the real-world report.

check_env 'env bystander: trivial command'      'echo ok'
check_env 'env bystander: arithmetic'           'echo $((1 + 1))'
check_env 'env bystander: a pipeline'           'echo hi | tr a-z A-Z'
check_env 'env bystander: a function call'      'f() { echo in-f; }; f'
check_env 'env bystander: exit status kept'     'false; echo "rc=$?"'
check_env 'env bystander: a failing command'    'exit 3'
check_env 'env bystander: set -u is unaffected' 'set -u; good=1; echo "$good"'
check_env 'env bystander: an external command'  'true; echo after-external'

# A bad byte in the NAME, not the value. bash does not make the name available
# as a shell variable either, so both shells simply run the program.
check_env_name() {
    local label="$1" frag="$2" b h
    local badname
    badname=$(printf 'N\xe9M')
    b=$(env "$badname=hello" timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(env "$badname=hello" timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check_env_name 'env bad NAME: shell still runs' 'echo ok'

# --- the shell STARTS with a bad byte in argv -----------------------------

check_argv 'argv bystander: trivial command'  'echo ok'
check_argv 'argv bystander: positional count' 'echo "$#"'
check_argv 'argv bystander: $0 is intact'     'echo "$0"'
check_argv 'argv bystander: an unset one'     'echo "${2-none}"'
check_argv 'argv bystander: shift'            'shift; echo "$#"'
check_argv 'argv bystander: exit status'      'exit 4'

# --- both at once ---------------------------------------------------------

check_both_label='env + argv together: shell still runs'
b=$(env "V=$BAD" timeout 10 "$BASH_BIN" --norc --noprofile -c 'echo ok' huck5 "$BAD" 2>&1; echo "EXIT:$?")
h=$(env "V=$BAD" timeout 10 "$HUCK_BIN" -c 'echo ok' huck5 "$BAD" 2>&1; echo "EXIT:$?")
compare "$check_both_label" "$b" "$h"

# --- reading the bad value: status agrees, bytes do not (phase 2) ---------
#
# Each of these prints U+FFFD in huck where bash prints the raw 0xE9. The row
# is here to pin that the shell RUNS — a panic would show up as EXIT:101.

check_rc 'READ bad env value'          env  'echo "$V"'
check_rc 'READ bad env value: length'  env  'echo "${#V}"'
check_rc 'READ bad env value: test -n' env  '[[ -n "$V" ]]'
check_rc 'READ bad env value to child' env  'env'
check_rc 'READ bad positional'         argv 'echo "$1"'
check_rc 'READ bad positional: length' argv 'echo "${#1}"'
check_rc 'READ all positionals'        argv 'echo "$@"'

harness_summary
