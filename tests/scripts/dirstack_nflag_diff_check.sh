#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `pushd -n` / `popd -n` and the rest of
# the two builtins' argument handling (#530).
#
# `-n` is not "skip the chdir": the stack manipulation itself changes shape.
#
#   pushd -n DIR   inserts DIR BELOW the current directory, unresolved and
#                  unvalidated (no chdir means nothing can fail), and prints
#   pushd -n +N    rotates the stack but leaves $PWD alone — so the entry that
#                  would have become the new $PWD is dropped and `dirs` shows a
#                  duplicate — and prints NOTHING
#   pushd -n       a silent no-op, not the usual swap, not even the
#                  "no other directory" complaint
#   popd -n        removes the entry BELOW $PWD, which is why `popd -n`,
#                  `popd -n +0` and `popd -n +1` all remove the same one
#
# huck rejected `-n` outright (`pushd: -n: invalid number`), so every row here
# failed on the message.
#
# The same argument loop carries four more divergences measured while pinning
# `-n`: `pushd DIR EXTRA` is "too many arguments" in bash and a silent push in
# huck; `popd DIR` is an invalid ARGUMENT with a usage line and status 2, even
# on an empty stack, where huck said "directory stack empty"; a `+N` past the
# end of an EMPTY stack is reported as an empty stack, not a bad index; and
# options may follow a spec (`pushd +1 -n`) but not a directory
# (`pushd /var -n` — the loop stopped at the directory, so `-n` is a second
# operand).
#
# Every fragment starts from a known cwd and prints `dirs` and `pwd`, since a
# stack change with no chdir is invisible in either alone.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
PRE='cd /tmp; '
P2='pushd /usr >/dev/null; pushd /etc >/dev/null; '

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$PRE$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$PRE$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- pushd -n DIR: inserted below $PWD, no chdir, stack printed ---
check "pushd -n dir"        'pushd -n /usr; dirs; pwd'
check "pushd -n twice"      'pushd -n /usr >/dev/null; pushd -n /var; dirs'
check "pushd -n onto stack" 'pushd /usr >/dev/null; pushd -n /etc; dirs; pwd'
check "pushd -n missing"    'pushd -n /nonexistent; echo "rc=$?"; dirs'
check "pushd -n relative"   'pushd -n sub; dirs; dirs -l'
check "pushd -n empty arg"  'pushd -n ""; echo "rc=$?"; dirs'
check "pushd -n then pushd" 'pushd -n /usr >/dev/null; pushd; dirs; pwd'
check "pushd -n then popd"  'pushd -n /usr >/dev/null; popd; dirs; pwd'
check "pushd -n extra args" 'pushd -n /usr /var; echo "rc=$?"; dirs'

# --- pushd -n +N: rotates, keeps $PWD, prints nothing ---
check "pushd -n +1"         "$P2"'pushd -n +1; echo "rc=$?"; dirs -v; pwd'
check "pushd -n +2"         "$P2"'pushd -n +2; dirs -v; pwd'
check "pushd -n -0"         "$P2"'pushd -n -0; dirs -v; pwd'
check "pushd -n -1"         "$P2"'pushd -n -1; dirs -v; pwd'
check "pushd -n +0"         "$P2"'pushd -n +0; echo "rc=$?"; dirs -v; pwd'
check "spec then -n"        "$P2"'pushd +1 -n; echo "rc=$?"; dirs; pwd'
check "pushd -n +1 twice"   'pushd -n /usr >/dev/null; pushd -n +1; dirs -l'

# --- bare pushd -n: a silent no-op ---
check "bare -n empty stack" 'pushd -n; echo "rc=$?"; dirs'
check "bare -n with stack"  "$P2"'pushd -n; echo "rc=$?"; dirs; pwd'
check "bare -n twice"       "$P2"'pushd -n -n; echo "rc=$?"; dirs'
check "bare -n then --"     "$P2"'pushd -n --; echo "rc=$?"; dirs'

# --- popd -n: the entry below $PWD ---
check "popd -n"             "$P2"'popd -n; echo "rc=$?"; dirs; pwd'
check "popd -n +0"          "$P2"'popd -n +0; dirs; pwd'
check "popd -n +1"          "$P2"'popd -n +1; dirs; pwd'
check "popd -n +2"          "$P2"'popd -n +2; dirs; pwd'
check "popd -n -0"          "$P2"'popd -n -0; dirs; pwd'
check "popd -n -1"          "$P2"'popd -n -1; dirs; pwd'
check "popd -n -2"          "$P2"'popd -n -2; dirs; pwd'
check "popd -n to empty"    "$P2"'popd -n; popd -n; popd -n; echo "rc=$?"; dirs'
check "popd -n empty stack" 'popd -n; echo "rc=$?"'
check "popd -n then dash"   "$P2"'popd -n --; dirs'
check "spec then -n popd"   "$P2"'popd +1 -n; dirs; pwd'

# --- the rest of the argument loop ---
check "pushd two dirs"      'pushd /usr /var; echo "rc=$?"; dirs; pwd'
check "pushd three dirs"    'pushd /usr /var /run; echo "rc=$?"; dirs'
check "pushd dir then spec" 'pushd /usr +0; echo "rc=$?"; dirs'
check "pushd dir then -n"   'pushd /usr -n; echo "rc=$?"; dirs; pwd'
check "pushd two specs"     "$P2"'pushd +1 +2; dirs; pwd'
check "pushd after ddash"   'pushd -- -n; echo "rc=$?"; dirs'
check "pushd ddash two"     'pushd -- /usr /var; echo "rc=$?"; dirs'
check "popd word arg"       'popd /usr; echo "rc=$?"'
check "popd word on stack"  'pushd /usr >/dev/null; popd /usr; echo "rc=$?"; dirs'
check "popd -n word arg"    'pushd /usr >/dev/null; popd -n /usr; echo "rc=$?"'
check "pushd -nn"           'pushd -nn /usr; echo "rc=$?"; dirs'
check "pushd -q with -n"    'pushd -n -q /usr; echo "rc=$?"'

# --- out of range, empty and not, both builtins ---
check "pushd +9 empty"      'pushd +9; echo "rc=$?"'
check "pushd -9 empty"      'pushd -9; echo "rc=$?"'
check "pushd +0 empty"      'pushd +0; echo "rc=$?"; dirs'
check "pushd -0 empty"      'pushd -0; echo "rc=$?"'
check "pushd +9 nonempty"   'pushd /usr >/dev/null; pushd +9; echo "rc=$?"'
check "pushd -n +9 empty"   'pushd -n +9; echo "rc=$?"'
check "pushd -n +5 stack"   'pushd /usr >/dev/null; pushd -n +5; echo "rc=$?"'
check "popd +9 empty"       'popd +9; echo "rc=$?"'
check "popd -n +5 stack"    'pushd /usr >/dev/null; popd -n +5; echo "rc=$?"'
check "popd -n -5 stack"    'pushd /usr >/dev/null; popd -n -5; echo "rc=$?"'

# --- controls: the no-`-n` forms must not have moved ---
check "plain pushd dir"     'pushd /usr; dirs; pwd'
check "plain popd"          'pushd /usr >/dev/null; popd; dirs; pwd'
check "plain rotate"        "$P2"'pushd +1; dirs; pwd'
check "plain popd +1"       "$P2"'popd +1; dirs; pwd'
check "bare pushd swap"     "$P2"'pushd; dirs; pwd'
check "bare pushd empty"    'pushd; echo "rc=$?"'
check "pushd ddash bare"    'pushd --; echo "rc=$?"'
check "popd empty"          'popd; echo "rc=$?"'
check "dirs -c after -n"    'pushd -n /usr >/dev/null; dirs -c; dirs; pwd'

harness_summary
