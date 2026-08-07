#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #439: bash scopes the DEBUG trap at
# function/source ENTRY, not at the fire site.
#
# THE MODEL (same one #434 established for RETURN and #438 for ERR): entering a
# function or a sourced script with functrace off UNSETS the caller's DEBUG for
# the duration of the body — so the caller's trap is invisible to `trap -p`
# inside it — and `maybe_set_debug_trap` puts the saved action back on the way
# out ONLY IF the body left DEBUG untrapped. Two consequences follow, and both
# were wrong before this fix: a trap a body installs FOR ITSELF fires for the
# body's remaining commands, and a trap the body installs SURVIVES the call.
#
# Every fire is a fixed marker (`D`, `C`) rather than $LINENO, so a row failing
# means the FIRE COUNT diverged, not the line numbering.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# Sourced fixtures live in a per-run dir so a parallel sweep cannot collide.
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

check() {
    local label="$1" frag="$2" b h
    # ⚠️ Both sides are capped: a DEBUG trap that fires per command turns any
    # runaway fragment into unbounded output, which has OOM-killed this box.
    b=$( ulimit -v 800000; timeout 10 bash --norc --noprofile -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    h=$( ulimit -v 800000; timeout 10 "$HUCK_BIN" -c "$frag" 2>&1 | head -c 2000; echo "EXIT:$?" )
    compare "$label" "$b" "$h"
}

printf 'echo s1\necho s2\n'                     > "$TMP/plain.sh"
printf 'trap "echo D" DEBUG\necho s1\necho s2\n' > "$TMP/self.sh"
printf 'trap - DEBUG\necho s1\n'                 > "$TMP/reset.sh"
printf 'trap -p DEBUG\n'                         > "$TMP/show.sh"
printf 'trap "" DEBUG\necho s1\n'                > "$TMP/ignore.sh"

# --- a trap the BODY installs fires for the rest of the body ----------------
check "self-installed fires in body"  'f() { trap "echo D" DEBUG; echo a; echo b; }; f; echo done'
check "self-installed, one command"   'f() { trap "echo D" DEBUG; echo a; }; f; echo done'
check "self-installed, nothing after" 'f() { trap "echo D" DEBUG; }; f; echo done'
check "self-installed then reset"     'f() { trap "echo D" DEBUG; echo a; trap - DEBUG; echo b; }; f; echo done'
check "self-installed is visible"     'f() { trap "echo D" DEBUG; trap -p DEBUG; }; f'

# --- the caller's trap is INVISIBLE inside the body -------------------------
check "caller trap not inherited"     'trap "echo D" DEBUG; f() { echo a; echo b; }; f; echo done'
check "caller trap invisible"         'trap "echo C" DEBUG; f() { trap -p DEBUG; }; f'
check "caller trap, nested calls"     'trap "echo C" DEBUG; f() { g; }; g() { echo a; }; f; echo done'

# --- restore-on-return: only if the body left DEBUG untrapped ---------------
check "body reset, caller restored"   'trap "echo C" DEBUG; f() { trap - DEBUG; }; f; echo after'
check "body trap survives the call"   'trap "echo C" DEBUG; f() { trap "echo D" DEBUG; }; f; echo after'
check "body trap survives, no caller" 'f() { trap "echo D" DEBUG; }; f; echo after'
check "body ignores DEBUG"            'trap "echo C" DEBUG; f() { trap "" DEBUG; }; f; echo after'
check "ignored caller trap stays"     'trap "" DEBUG; f() { trap -p DEBUG; echo a; }; f; echo after'

# --- nesting: each call gets its own entry-unset ----------------------------
check "inner call does not inherit"   'f() { trap "echo D" DEBUG; g; echo b; }; g() { echo g1; }; f'
check "inner installs its own"        'f() { g; echo b; }; g() { trap "echo G" DEBUG; echo g1; }; f; echo done'
check "recursion"                     'f() { trap "echo D" DEBUG; if [ "$1" -gt 0 ]; then f $(( $1 - 1 )); fi; }; f 2; echo done'

# --- functrace ON: the trap IS inherited, nothing is unset ------------------
check "set -T caller inherited"       'set -T; trap "echo D" DEBUG; f() { echo a; }; f; echo done'
check "set -T visible in body"        'set -T; trap "echo C" DEBUG; f() { trap -p DEBUG; }; f'
check "set -T nested"                 'set -T; trap "echo D" DEBUG; f() { g; }; g() { echo a; }; f'
check "extdebug caller inherited"     'shopt -s extdebug; trap "echo D" DEBUG; f() { echo a; }; f; echo done'
check "set -T body overrides"         'set -T; trap "echo C" DEBUG; f() { trap "echo D" DEBUG; echo a; }; f; echo after'

# --- the same rules for a SOURCED script ------------------------------------
check "source: caller not inherited"  "trap 'echo D' DEBUG; . $TMP/plain.sh; echo done"
check "source: caller invisible"      "trap 'echo D' DEBUG; . $TMP/show.sh; echo done"
check "source: self-installed fires"  ". $TMP/self.sh; echo done"
check "source: self over caller"      "trap 'echo C' DEBUG; . $TMP/self.sh; echo after"
check "source: reset restores caller" "trap 'echo C' DEBUG; . $TMP/reset.sh; echo after"
check "source: body ignores DEBUG"    "trap 'echo C' DEBUG; . $TMP/ignore.sh; echo after"
check "source: set -T inherited"      "set -T; trap 'echo D' DEBUG; . $TMP/plain.sh; echo done"
check "source inside a function"      "f() { . $TMP/self.sh; echo b; }; f; echo done"

# --- regression guards: things this change must NOT move --------------------
check "top level unaffected"          'trap "echo D" DEBUG; echo a; echo b'
check "top level reset"               'trap "echo D" DEBUG; echo a; trap - DEBUG; echo b'
check "RETURN unaffected"             'trap "echo R" RETURN; f() { echo a; }; f; echo done'
check "ERR unaffected"                'trap "echo E" ERR; f() { false; }; f; echo done'
check "extdebug skip in function"     'shopt -s extdebug; f() { trap "echo D; false" DEBUG; echo a; }; f; echo done'
check "extdebug return 2"             'shopt -s extdebug; set -T; trap "echo D; exit 2" DEBUG; f() { echo a; echo b; }; f; echo done'
check "DEBUG + RETURN together"       'set -T; trap "echo D" DEBUG; trap "echo R" RETURN; f() { echo a; }; f'
check "trap -p all inside body"       'trap "echo C" DEBUG; trap "echo R" RETURN; f() { trap -p; }; f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
