#!/usr/bin/env bash
# Byte-identical bash<->huck harness for WHERE a failure is adjudicated (#676,
# #685).
#
# POSIX and bash exempt from `set -e` "any command executed in a `&&` or `||`
# list except the command following the final `&&` or `||`". huck applied that
# at top level and lost it whenever the list was the LAST command of a compound
# body: the body's status — already adjudicated, and exempted, at the inner
# command — was judged a second time at the enclosing compound, which knew
# nothing of the exemption. Found by the runtime sweep: `/usr/sbin/on_ac_power`
# ends in exactly that shape and returned 1 under huck where bash returns 0.
#
# The rule is one adjudication per failure, at the site that produced it. This
# harness is the matrix that pins it: every construct x every failure shape x
# both consumers (`set -e`, and the ERR trap).
#
# ⚠️ The ERR rows write the trap action to STDERR, and that is load-bearing.
# Counting fires on stdout hid a fire that happened INSIDE the redirect under
# test and read `bash: 0, huck: 1` where the truth was `bash: 1, huck: 2`.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# ⚠️ Status captured BEFORE any pipe — `cmd | sed; echo $?` reports sed's status.
# ⚠️ The program-name prefix is normalised: under `-c` bash says `bash:` and huck
# says its own argv[0], which is the absolute path this harness invoked.
norm() { sed -E 's#^[^:]*: line #SH: line #'; }
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    out=$("$HUCK_BIN" --norc -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── errexit: an EXEMPT failure inside an in-place compound must NOT exit ──────
# Every row here should reach the trailing `echo R`.
check 'if body'          'set -e; if true; then false && true; fi; echo R'
check 'elif body'        'set -e; if false; then :; elif true; then false && true; fi; echo R'
check 'else body'        'set -e; if false; then :; else false && true; fi; echo R'
check 'for body'         'set -e; for i in x; do false && true; done; echo R'
check 'while body'       'set -e; i=0; while [ $i = 0 ]; do i=1; false && true; done; echo R'
check 'until body'       'set -e; i=0; until [ $i = 1 ]; do i=1; false && true; done; echo R'
check 'arith-for body'   'set -e; for ((i=0;i<1;i++)); do false && true; done; echo R'
check 'case body'        'set -e; case x in x) false && true;; esac; echo R'
check 'brace group'      'set -e; { false && true; }; echo R'
check 'nested compound'  'set -e; if true; then { false && true; }; fi; echo R'
check 'deeply nested'    'set -e; for i in x; do if true; then { false && true; }; fi; done; echo R'
# ⚠️ A control, not a bug row: an `||` whose LEFT side fails still ends 0, so
# there is no failing status for the compound to misjudge. Kept as a guard.
check 'or-list, left fails' 'set -e; if true; then false || true; fi; echo R'
check 'bang in body'     'set -e; if true; then ! true; fi; echo R'
check 'dbracket in body' 'set -e; if true; then [[ 1 = 2 ]] && true; fi; echo R'
check 'redirected group' 'set -e; { false && true; } > /dev/null; echo R'
check 'redirected if'    'set -e; if true; then false && true; fi > /dev/null; echo R'

# ── errexit: a status the command OWNS must still exit ───────────────────────
# ⚠️ These are what stop the fix being "never adjudicate a compound". A plain
# inner failure exits through the INNER command's own adjudication, not the
# compound's — which is the point the whole change turns on.
check 'plain inner false'    'set -e; { false; }; echo R'
check 'plain inner in if'    'set -e; if true; then false; fi; echo R'
check 'plain inner in for'   'set -e; for i in x; do false; done; echo R'
check 'plain inner in while' 'set -e; i=0; while [ $i = 0 ]; do i=1; false; done; echo R'
check 'plain inner in case'  'set -e; case x in x) false;; esac; echo R'
check 'subshell exempt'      'set -e; ( false && true ); echo R'
check 'function exempt'      'set -e; f(){ false && true; }; f; echo R'
check 'redirect FAILS'       'set -e; { :; } > /nonexistent/x; echo R'
check 'redirect FAILS on if' 'set -e; if true; then :; fi > /nonexistent/x; echo R'
check 'dbracket alone'       'set -e; [[ 1 = 2 ]]; echo R'
check 'arith alone'          'set -e; (( 0 )); echo R'
check 'pipeline'             'set -e; false | cat; echo R'
check 'last of and-or'       'set -e; true && false; echo R'
check 'not last of and-or'   'set -e; false && true; echo R'
check 'condition failure'    'set -e; if false; then :; fi; echo R'

# ── errexit OFF: the STATUS itself must not move ─────────────────────────────
check 'status, exempt body'   'if true; then false && true; fi; echo "st=$?"'
check 'status, plain body'    'if true; then false; fi; echo "st=$?"'
check 'status, empty for'     'for i in; do false; done; echo "st=$?"'
check 'status, no case match' 'case x in y) false;; esac; echo "st=$?"'
check 'status, if no branch'  'if false; then false; fi; echo "st=$?"'
check 'status, loop last'     'for i in 1 2; do false; done; echo "st=$?"'

# ── the ERR trap: fire COUNT, action written to STDERR (see the header) ──────
check 'ERR in if body'      'trap "echo ERRFIRE >&2" ERR; if true; then false; fi'
check 'ERR in for body'     'trap "echo ERRFIRE >&2" ERR; for i in x; do false; done'
check 'ERR in brace'        'trap "echo ERRFIRE >&2" ERR; { false; }'
check 'ERR nested braces'   'trap "echo ERRFIRE >&2" ERR; { { false; }; }'
check 'ERR exempt body'     'trap "echo ERRFIRE >&2" ERR; if true; then false && true; fi'
check 'ERR subshell'        'trap "echo ERRFIRE >&2" ERR; ( false )'
check 'ERR function'        'trap "echo ERRFIRE >&2" ERR; f(){ false; }; f'
check 'ERR redirected grp'  'trap "echo ERRFIRE >&2" ERR; { false; } > /dev/null'
check 'ERR redirected if'   'trap "echo ERRFIRE >&2" ERR; if true; then false; fi > /dev/null'
check 'ERR redirect FAILS'  'trap "echo ERRFIRE >&2" ERR; { :; } > /nonexistent/x'
check 'ERR bang bang'       'trap "echo ERRFIRE >&2" ERR; ! ! { false; }'
check 'ERR with errexit'    'set -e; trap "echo ERRFIRE >&2" ERR; if true; then false; fi'
check 'ERR action exits'    'set -e; trap "exit 9" ERR; if true; then false; fi; echo R'

harness_summary
