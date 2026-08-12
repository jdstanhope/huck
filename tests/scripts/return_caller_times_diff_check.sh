#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `return` / `caller` / `times` argument
# handling (#520). None of the three is a getopt builtin in bash, and each gets
# its arguments wrong in a different way.
#
#   return  bash's `get_exitstat`: skip a leading `--`, then the argument must
#           be a `legal_number` (base 10, optional sign, surrounding whitespace
#           allowed — so `" 3 "` is 3 but `0x10` and `""` are not), and the
#           value is masked to `& 255`. A non-number is
#           `return: <arg>: numeric argument required` and the function returns
#           with status 2. MORE than one argument is `too many arguments` and a
#           HARD abort of the shell at status 1 — not catchable by `||` or `if`,
#           though a `( return 3 4 )` kills only the subshell.
#
#   caller  returns 1 SILENTLY when there is no call frame, BEFORE looking at
#           the arguments — so `caller -Q` at the top level is rc 1 with no
#           diagnostic. Inside a frame a leading dash is an INVALID OPTION
#           (huck called it an invalid NUMBER), and `--` is consumed.
#
#   times   takes no options and IGNORES operands: `times x` prints the times,
#           `times -Q` is a usage error. huck ran regardless, so a bad option
#           silently did the work.
#
# `times`'s successful output is a live clock reading and cannot be diffed;
# those rows discard stdout and compare the status only.
#
# Both shells run with an EXPLICIT $0 ("huck5") so the error prologue matches.
#
# NOT covered: a bare `caller` inside a function (bash prints `1 NULL`, huck
# prints nothing and returns 1 — #559), and a top-level `return` (bash reports
# ``can only `return' from a function or sourced script`` and CONTINUES; huck
# stops the script silently — #560).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

# DRIVER: `-c` with an explicit $0. The OUTER status is compared too, because
# `return`'s too-many-arguments abort is visible only there.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- return: the numeric argument ---
check "return -Q"          'f(){ return -Q; }; f; echo "in=$?"'
check "return -Q skips body" 'f(){ return -Q; echo body; }; f; echo "in=$?"'
check "return abc"         'f(){ return abc; }; f; echo "in=$?"'
check "return empty"       'f(){ return ""; }; f; echo "in=$?"'
check "return hex"         'f(){ return 0x10; }; f; echo "in=$?"'
check "return float"       'f(){ return 3.5; }; f; echo "in=$?"'
check "return padded"      'f(){ return " 3 "; }; f; echo "in=$?"'
check "return plus"        'f(){ return +5; }; f; echo "in=$?"'
check "return leading zero" 'f(){ return 010; }; f; echo "in=$?"'
check "return overflow"    'f(){ return 99999999999999999999; }; f; echo "in=$?"'

# --- return: the & 255 mask ---
check "return -1"          'f(){ return -1; }; f; echo "in=$?"'
check "return -5"          'f(){ return -5; }; f; echo "in=$?"'
check "return 256"         'f(){ return 256; }; f; echo "in=$?"'
check "return 300"         'f(){ return 300; }; f; echo "in=$?"'

# --- return: `--` and the argument count ---
check "return --"          'f(){ return --; }; f; echo "in=$?"'
check "return -- 3"        'f(){ return -- 3; }; f; echo "in=$?"'
check "return -- -3"       'f(){ return -- -3; }; f; echo "in=$?"'
check "return two args"    'f(){ return 3 4; }; f; echo never'
check "return three args"  'f(){ return 3 4 5; }; f; echo never'
check "return two uncatchable" 'f(){ return 3 4; }; f || echo caught'
check "return two in if"   'f(){ return 3 4; }; if f; then echo t; else echo e; fi'
check "return two in subshell" 'f(){ (return 3 4); echo after; }; f'

# --- return: the forms that must not move ---
check "return bare"        'f(){ return; }; f; echo "in=$?"'
check "return bare after false" 'f(){ false; return; }; f; echo "in=$?"'
check "return 0"           'f(){ return 0; }; f; echo "in=$?"'
check "return 3"           'f(){ return 3; }; f; echo "in=$?"'
check "return in sourced"  'echo "return 7" > /tmp/huck-ret-$$.sh; . /tmp/huck-ret-$$.sh; echo "in=$?"; rm -f /tmp/huck-ret-$$.sh'

# --- caller: no frame is a silent 1, before the arguments are looked at ---
check "caller top bad opt" 'caller -Q; echo "in=$?"'
check "caller top bare"    'caller; echo "in=$?"'
check "caller top number"  'caller 0; echo "in=$?"'
check "caller top bad num" 'caller abc; echo "in=$?"'

# --- caller: inside a frame, a leading dash is an INVALID OPTION ---
check "caller -Q in fn"    'f(){ caller -Q; }; f; echo "in=$?"'
check "caller -1 in fn"    'f(){ caller -1; }; f; echo "in=$?"'
check "caller bad num"     'f(){ caller abc; }; f; echo "in=$?"'
check "caller 0 in fn"     'f(){ caller 0; }; f; echo "in=$?"'
check "caller 9 in fn"     'f(){ caller 9; }; f; echo "in=$?"'

# --- times: no options, operands ignored ---
check "times bad option"   'times -Q; echo "in=$?"'
check "times two bad opts" 'times -Q -R; echo "in=$?"'
check "times operand rc"   'times x >/dev/null; echo "in=$?"'
check "times ddash rc"     'times -- >/dev/null; echo "in=$?"'
check "times bare rc"      'times >/dev/null; echo "in=$?"'
check "times bad opt then" 'times -Q; echo after'

harness_summary
