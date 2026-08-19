#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the DRIVER-dependence of `return` with
# too many arguments (#683).
#
# The message and the status were already right; the OUTCOME was not. bash
# abandons the command list and carries on in a script file and on stdin, and
# ends the program under `-c`, `source` and `eval`. huck hardcoded `Exit(1)`,
# so it was fatal everywhere — the check predated v358's error-fatality
# classifier, which owns exactly this split ("the kind picks the outcome, the
# driver picks the code").
#
# ⚠️ Measured, and the measurement is why `return` joined `history`'s existing
# ErrorKind instead of getting a new one: the two are byte-identical on all five
# drivers. A third member must be measured against this same table first.
#
# Five DRIVERS is the point of this file, so it does not use the shared
# single-driver `check`: piped stdin, `-c`, a script file, `source` and `eval`
# are different top-level readers in huck, and a fatality is per-driver.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

# ⚠️ Normalises the script name and the program-name prefix. bash says `bash:`
# under `-c` where huck says its own argv[0] (an absolute path here), and the
# temp dir differs per run — differences about how the binary was found, not
# about the message.
norm() { sed -E "s#^[^ :]*/([a-z0-9_]+\.sh):#SH:#; s#^[^ :]*[/ ]?(huck|bash):#SH:#"; }

# ⚠️ Status captured BEFORE the normalising pipe — `cmd | sed; echo $?` reports
# sed's status, which would make every rc assertion here vacuous.
run_driver() {
    local sh="$1" driver="$2" frag="$3" out rc
    case "$driver" in
        script) printf 'echo A\n%s\necho "B st=$?"\n' "$frag" > "$T/p.sh"
                out=$(cd "$T" && "$sh" p.sh 2>&1); rc=$? ;;
        stdin)  out=$(printf 'echo A\n%s\necho "B st=$?"\n' "$frag" | "$sh" 2>&1); rc=$? ;;
        dash-c) out=$("$sh" -c "echo A; $frag; echo \"B st=\$?\"" 2>&1); rc=$? ;;
        source) printf '%s\n' "$frag" > "$T/i.sh"
                out=$(cd "$T" && "$sh" -c 'echo A; . ./i.sh; echo "OUTER=$?"' 2>&1); rc=$? ;;
        eval)   out=$("$sh" -c "echo A; eval '$frag'; echo \"AFTER=\$?\"" 2>&1); rc=$? ;;
    esac
    printf '%s\n' "$out" | norm
    echo "EXIT:$rc"
}

check_all_drivers() {
    local label="$1" frag="$2" d
    for d in script stdin dash-c source eval; do
        compare "$label [$d]" \
            "$(run_driver bash "$d" "$frag")" \
            "$(run_driver "$HUCK_BIN" "$d" "$frag")"
    done
}

# ── the bug ──────────────────────────────────────────────────────────────────
check_all_drivers 'return too many args' 'return 1 2'

# ── the kind's other member, which was already right in all five ─────────────
check_all_drivers 'history too many args' 'history 1 2'

# ── controls: neighbouring builtin usage errors all CONTINUE everywhere ──────
# These are what stop the fix being "usage errors are fatal": bash continues
# past every one of them, including the two SPECIAL builtins.
check_all_drivers 'shift too many'  'shift a b'
check_all_drivers 'break too many'  'break 1 2'
check_all_drivers 'umask too many'  'umask a b'
check_all_drivers 'history one bad' 'history a'

# ── controls: `return`'s OTHER errors keep their own outcomes ────────────────
check_all_drivers 'return bad number' 'return abc'
check_all_drivers 'return in bounds'  'f(){ return 3; }; f'

harness_summary
