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

# ── `-:` must not panic (review finding, post-Task-7): the spec string uses
# ':' as a VALUE marker, but `Getopt::accepts` compared it as if it were a
# real option character, so `-:` was "accepted", handed to the builtin as
# `Opt { ch: ':' }`, and every call site's `_ => unreachable!("spec and
# match must agree")` panicked the process (rc 101, killing the rest of a
# script). One builtin per spec family: `hash`'s spec ("lrp:dt") DOES
# contain a value marker (this was the exact crash repro); `unset`'s spec
# ("fvn") has none, so `-:` was already safe there and this row guards
# against a regression in the other direction.
check "hash -: does not panic"  'hash -:'
check "unset -: does not panic" 'unset -:'

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

# ── mapfile -u/-C/-c are now IMPLEMENTED (#511). They were rejected outright
# while unimplemented, because parsing-and-ignoring them produced wrong data
# with no error (#496 Task 6 review, Critical). Now they byte-match bash, so
# the behaviour itself is pinned here rather than the rejection.
check "mapfile -u missing value"  'mapfile -u'
check "mapfile -C missing value"  'mapfile -C'
check "mapfile -u bad fd"         'mapfile -t -u 9 A; echo rc=$?'
check "mapfile -u non-numeric"    'mapfile -t -u abc A; echo rc=$?'
check "mapfile -u reads that fd"  'printf "a\nb\n" > /tmp/hk511.$$; exec 3</tmp/hk511.$$; mapfile -t -u 3 A; printf "%s|" "${A[@]}"; echo "n=${#A[@]}"; rm -f /tmp/hk511.$$'
check "mapfile -C -c fires"       'printf "a\nb\nc\nd\ne\n" > /tmp/hk511b.$$; mapfile -t -C "echo CB:" -c 2 A < /tmp/hk511b.$$; echo "n=${#A[@]}"; rm -f /tmp/hk511b.$$'
check "mapfile -C default quantum" 'printf "a\nb\nc\n" > /tmp/hk511c.$$; mapfile -t -C "echo CB:" A < /tmp/hk511c.$$; echo "n=${#A[@]}"; rm -f /tmp/hk511c.$$'
check "mapfile -C sees partial"   'printf "a\nb\nc\nd\ne\n" > /tmp/hk511d.$$; mapfile -t -C "echo have=\${#A[@]}" -c 2 A < /tmp/hk511d.$$; rm -f /tmp/hk511d.$$'
check "mapfile clears on empty"   'A=(x y z); mapfile -t A < /dev/null; echo "n=${#A[@]}"'

# ── jobs -x: real, bash-IMPLEMENTED option (substitutes jobspecs with pids
# and execs COMMAND in the shell's place) huck deliberately does not
# implement. (mapfile -u/-C/-c were in this position until #511
# implemented them; the reasoning below is now jobs -x's alone.) Unlike them,
# `-x` takes no getopt value at all, so there is no missing-value shape to
# exploit for a byte-matching row either: EVERY fragment that exercises `-x`
# as a real flag diverges (bare `jobs -x` exits 0 silently in real bash;
# `jobs -x echo hi` prints "hi" rc 0). Pinned instead by a unit test in
# builtins/tests.rs (jobs_x_is_rejected_as_invalid_option) — no harness row
# is possible here without huck actually implementing the feature.

# ── history -anrw: real mutual exclusion, fixed IN this task, not reverted
# (#496 Task 6 review, Important) — bash rejects ANY two of -a/-n/-r/-w
# together, byte-matches once implemented.
check "history -aw mutual exclusion" 'history -aw /tmp/nonexistent_hist_xyz_496'

# ── cd -e: implemented for Task 7 (#496 Task 7 review) — "if -P is given
# and the current working directory cannot be determined successfully,
# exit with a non-zero status." Two DISTINCT bash-verified failure shapes
# feed the same exit code; the review caught that only shape 1 was wired,
# because shape 2 was dismissed in the original report as "not
# exercisable in a byte-diff fragment" — it is, in three lines, below.
# Shape 1 (getcwd(2) itself fails, e.g. a since-deleted cwd) is real but
# its diagnostic TEXT is a pre-existing, unrelated wording divergence
# ("cd: warning: could not read current dir: ..." vs bash's "cd: error
# retrieving current directory: getcwd: ...") — not byte-matchable, and
# not part of this fix, so no harness row for it (would need a separate
# issue to normalize the wording).
check "cd -Pe normal (no failure)" 'cd -Pe /tmp; echo rc=$?'
check "cd -e without -P is ignored" 'cd -e /tmp; echo rc=$?'
# Shape 2: getcwd(2) SUCCEEDS (it walks the kernel's dentry tree, which
# bypasses directory search-permission checks) but a plain NAME-based
# lookup of that same path does not, because an ancestor directory loses
# search (x) permission after the shell is already resident inside it.
# `chmod 755` at the end restores permissions so this row is idempotent
# across repeated runs (bash then huck, same real filesystem).
check "cd -P -e ancestor search-permission failure" \
  'rm -rf /tmp/t9_cdE; mkdir -p /tmp/t9_cdE/sub; cd /tmp/t9_cdE/sub; chmod 000 /tmp/t9_cdE; cd -P -e .; echo rc=$?; chmod 755 /tmp/t9_cdE; rm -rf /tmp/t9_cdE'

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

# ── `+`-option handling (#521, #515) ──
# `declare` and `compopt` genuinely take `+` options; the scanner deliberately
# does not (bash's internal_getopt has no `+` either), so those two keep their
# own `+` loops — but share ONE emit, or the diagnostics drift apart, which is
# what these rows pin.
check "compopt +invalid"        'compopt +z'
check "compopt +unknown 2"      'compopt +q'
check "compopt +o is real"      'compopt +o nospace'
check "declare +invalid"        'declare +z v=1'
# bash's complete/compgen do NOT parse `+` at all — `+z` is a NAME.
check "complete + is a name"    'complete +z foo; echo rc=$?'
check "complete +o is a name"   'complete +o nospace foo; echo rc=$?'
check "complete empty compspec" 'complete foo; echo rc=$?'
check "complete -- then -o"     'complete -- -o foo; echo rc=$?'

# ── exec (#516). Its scanner is hand-rolled ON PURPOSE — the parse must stay
# pure because it runs twice (once at the restricted-policy check purely to ask
# whether a command word is present, with errors swallowed). Its scanning was
# already bash-correct; what was missing was the usage line, now emitted through
# the same shared helpers and the same `usage_for` table as every other builtin.
check "exec -Q"              'exec -Q'
check "exec -Q with command" 'exec -Q true'
check "exec -a missing value" 'exec -a'
check "exec -cl bundles"     'exec -cl'
check "exec -- terminator"   'exec --'
check "exec lone dash"       'exec -'
check "exec -a attached"     'exec -anm true; echo after'

# ── mapfile numeric-option validation (#513). bash uses a DIFFERENT message
# per option, not one generic string, and exits 1 not 2.
check "mapfile -n non-numeric"  'mapfile -n abc A'
check "mapfile -s non-numeric"  'mapfile -s abc A'
check "mapfile -O non-numeric"  'mapfile -O abc A'
check "mapfile -c non-numeric"  'mapfile -c abc A'
check "mapfile -n negative"     'mapfile -n -1 A'
check "readarray -n names self" 'readarray -n abc A'

# ── wait -p without -n (#514). bash has no "requires -n" rule: it accepts the
# option silently and simply leaves the variable unset.
check "wait -p alone"           'wait -p v; echo "[${v-unset}]"'
check "wait -n -p"              'wait -n -p v'

# ── declare/local `+attr` (#507). bash ACCEPTS `+r` and does nothing — it
# cannot remove readonly, but that is silent. The only error in
# `declare -r w=1; declare +r w=2` is the ASSIGNMENT to a readonly variable.
# And `local` DOES take `+` options; huck reported them as invalid identifiers.
check "declare +r no-op"      'v=1; declare +r v=2; echo "rc=$? [$v]"'
check "declare +r on readonly" 'declare -r w=1; declare +r w=2; echo "rc=$? [$w]"'
check "local +r accepted"     'f(){ local +r x=1; echo "rc=$? [$x]"; }; f'
check "local +x accepted"     'f(){ local +x x=1; echo "rc=$? [$x]"; }; f'
check "local +unknown"        'f(){ local +z x=1; }; f'
check "local - and + interleave" 'f(){ local -r +x y=2; echo "rc=$? [$y]"; }; f'

# ── alias -p with operands (#510): bash prints the WHOLE table, then handles
# the operands normally — so `-p nosuch` prints the table and THEN errors.
check "alias -p with operand" 'alias xx=yy; alias -p xx'
check "alias -p two aliases"  'alias xx=yy zz=ww; alias -p xx'
check "alias -p missing name" 'alias xx=yy; alias -p nosuch'

# ── command -v/-V (#508): the LAST one wins, bundled or separate. huck always
# took -v, which agreed only when -v happened to come last.
check "command -v -V"         'command -v -V ls'
check "command -V -v"         'command -V -v ls'
check "command -vV bundled"   'command -vV ls'
check "command -Vv bundled"   'command -Vv ls'

harness_summary
