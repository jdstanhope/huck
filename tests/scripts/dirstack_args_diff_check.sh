#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `pushd`/`popd`/`dirs` ARGUMENT
# handling (#519).
#
# These three are deliberately NOT on the shared `builtin_opts` scanner and must
# not be: they take `+N`/`-N` rotation arguments, so bash does not bundle their
# flags at all. Measured on bash 5.2.21 — each argument is matched WHOLE:
#
#   -c/-l/-p/-v (dirs), -n (pushd/popd)  ... a known option
#   --                                   ... terminator
#   +N / -N  (digits only)               ... a rotation index
#   anything else starting with `-`      ... `NAME: <whole token>: invalid number`
#
# The whole-token part is the giveaway: bash reports `dirs -cl` as `-cl`, not as
# `-c`. That is why these keep hand-rolled parsing.
#
# Both shells run with an EXPLICIT $0 ("huck5") so the error prologue matches and
# this is a plain byte comparison, with no normalisation to hide a real prologue
# divergence.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$("$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# ── a bad argument is an invalid NUMBER, not an invalid option ──
check "dirs -Q"            'dirs -Q'
check "dirs -foo"          'dirs -foo'
check "pushd -Q"           'pushd -Q'
check "pushd -foo"         'pushd -foo'
check "popd -Q"            'popd -Q'
check "popd -foo"          'popd -foo'

# ── the whole token is reported, so a bundled-looking arg is one bad number ──
check "dirs -cl not bundled" 'dirs -cl'
check "dirs -lv not bundled" 'dirs -lv'

# ── `pushd -Q` used to misroute to `cd` and print CD's usage string ──
check "pushd -Q names pushd" 'pushd -Q 2>&1 | head -1'

# ── popd validates its argument BEFORE the stack-empty check ──
check "popd -Q on empty"   'popd -Q'

# ── real options and rotation indices must keep working ──
check "dirs -c"            'dirs -c; echo rc=$?'
check "dirs -v"            'dirs -v'
check "dirs -p"            'dirs -p'
check "dirs +0"            'dirs +0'
check "dirs -0"            'dirs -0'
check "dirs --"            'dirs --'
check "pushd --"           'pushd --'
check "popd --"            'popd --'
check "pushd -- dir"       'cd /tmp; pushd -- /usr; pwd'
check "pushd dir/popd"     'cd /tmp; pushd /usr >/dev/null; dirs; popd >/dev/null; dirs; pwd'
check "pushd rotate +1"    'cd /tmp; pushd /usr >/dev/null; pushd +1 >/dev/null; dirs'
check "pushd missing dir"  'pushd /nonexistent; echo rc=$?'
check "bare pushd"         'pushd; echo rc=$?'
check "bare popd"          'popd; echo rc=$?'

harness_summary
