#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #198: when an error occurs, does it
# abandon the current command LIST, exit the SHELL, or neither — and with what
# exit code?
#
# ⚠️ THE DISCRIMINATOR IS A TWO-LINE SCRIPT. `-c` cannot tell "abandon the rest
# of this command list" from "exit the shell", because both suppress
# everything after the error. Two lines separate them:
#
#     line 1:   <ERROR>; echo SAME
#     line 2:   echo NEXT
#
#     SAME NEXT -> continue     NEXT -> abort LIST     (nothing) -> exit SHELL
#
# ⚠️ CAPTURE THE EXIT CODE SEPARATELY. `rc=$?` after a pipeline reads the
# pipeline's last stage, not the shell. That reported false agreement TWICE
# while this contract was being measured — once here and once on $PIPESTATUS.
#
# ⚠️ EVERY ROW RUNS UNDER THREE DRIVERS. The exit code depends on the driver
# and on nothing else: `-c` substitutes 127 for the error kind's own code,
# while a script or piped stdin keeps it. A single-driver harness would have
# shown agreement on cells that disagree.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# $1 label, $2 prelude, $3 error fragment, $4 driver (dashc|script|stdin)
check() {
    local label="$1" pre="$2" frag="$3" drv="$4"
    local bo brc ho hrc b h o c sh
    printf '%s\n%s; echo SAME\necho NEXT\n' "$pre" "$frag" > "$TMP/s.sh"
    for sh in bash "$HUCK_BIN"; do
        case "$drv" in
            dashc)
                o=$( ulimit -v 800000; timeout 10 "$sh" -c "$pre
$frag; echo SAME
echo NEXT" 2>/dev/null ); c=$? ;;
            script)
                o=$( ulimit -v 800000; timeout 10 "$sh" "$TMP/s.sh" 2>/dev/null ); c=$? ;;
            stdin)
                o=$( ulimit -v 800000; timeout 10 "$sh" < "$TMP/s.sh" 2>/dev/null ); c=$? ;;
        esac
        # Truncate AFTER capturing $? — piping the shell into `head` would make
        # `$?` report `head`. This harness header warns about exactly that, and
        # the first draft did it anyway: it reported PASS on every cell whose
        # only divergence was the exit code (`posix arith` under a script is
        # bash 1 / huck 127, and it showed green).
        o=${o:0:400}
        if [[ "$sh" == bash ]]; then bo="$o"; brc=$c; else ho="$o"; hrc=$c; fi
    done
    b="[$(echo $bo)] rc=$brc"
    h="[$(echo $ho)] rc=$hrc"
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s (%s)\n' "$label" "$drv"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (%s)\n    bash %s\n    huck %s\n' "$label" "$drv" "$b" "$h"
        FAIL=$((FAIL+1))
    fi
}

for drv in dashc script stdin; do
    # --- expansion ---------------------------------------------------------
    check "arith"              ''  'echo $((1/0))'            "$drv"
    check "bad substitution"   ''  'echo ${x[}'               "$drv"
    check "readonly assign"    'readonly r=1'  'r=2'          "$drv"
    check "nounset"            'set -u'  'echo $undef_zz'     "$drv"
    check "posix arith"        'set -o posix'  'echo $((1/0))' "$drv"
    check "posix readonly"     'set -o posix
readonly r=1'  'r=2'                                          "$drv"
    check "posix nounset"      'set -o posix
set -u'  'echo $undef_zz'                                     "$drv"

    # --- builtins ----------------------------------------------------------
    # Only `history` with too many arguments abandons the list. The rest are
    # the "must not change" set for that rule — if one of them starts aborting,
    # the HistoryTooManyArgs kind has been over-applied.
    check "history too many"   ''  'history 1 2 3'            "$drv"
    check "history bad num"    ''  'history a'                "$drv"
    check "history opt"        ''  'history -Q'               "$drv"
    check "posix history many" 'set -o posix'  'history 1 2 3' "$drv"
    check "cd bad opt"         ''  'cd -Q'                    "$drv"
    check "cd missing dir"     ''  'cd /nonexistent-zz'       "$drv"
    check "kill bad opt"       ''  'kill -Q 1'                "$drv"
    check "read bad opt"       ''  'read -Q'                  "$drv"
    check "getopts no args"    ''  'getopts'                  "$drv"
    check "umask bad"          ''  'umask a b'                "$drv"

    # --- special builtins --------------------------------------------------
    # Fatal on USAGE rejection in posix mode only. `shift a b` and `break 1 2`
    # are special builtins whose errors bash still continues past in BOTH
    # modes — they guard against over-applying SpecialBuiltinUsage.
    check "special set -Q"     ''  'set -Q'                   "$drv"
    check "special unset -Q"   ''  'unset -Q x'               "$drv"
    check "special export -Q"  ''  'export -Q'                "$drv"
    check "special shift bad"  ''  'shift a b'                "$drv"
    check "break too many"     ''  'break 1 2'                "$drv"
    check "posix set -Q"       'set -o posix'  'set -Q'       "$drv"
    check "posix unset -Q"     'set -o posix'  'unset -Q x'   "$drv"
    check "posix export -Q"    'set -o posix'  'export -Q'    "$drv"
    check "posix shift bad"    'set -o posix'  'shift a b'    "$drv"

    # --- syntax ------------------------------------------------------------
    check "backtick syntax"    ''  'echo `echo a; ; echo b`'  "$drv"
    check "dollarparen syntax" ''  'echo $(echo a; ; echo b)' "$drv"
    check "posix backtick"     'set -o posix'  'echo `echo a; ; echo b`' "$drv"

    # --- must-not-change ---------------------------------------------------
    check "command not found"  ''  'no_such_cmd_zz'           "$drv"
    check "redirect failure"   ''  'echo hi > /proc/nope/x'   "$drv"
    check "bad fd"             ''  'exec 3>&99'               "$drv"
    check "posix cmd notfound" 'set -o posix'  'no_such_cmd_zz' "$drv"
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
