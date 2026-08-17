#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v126: a bare assignment's $? = the last
# command substitution in its RHS (or 0). File-arg execution (L-27).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# ⚠️ Both shells run under a `timeout`. The #661 rows below include loops whose
# condition is a command that expands to nothing, and while that bug was live
# they NEVER TERMINATED — running this harness against a pre-fix binary times out
# rather than failing. A regression here must make the sweep RED, not hang it, so
# a timeout turns the hang into a diff (`EXIT:124` on one side only).
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(timeout 10 bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    compare "$label" "$b" "$h"
}

check "false sub"        'x=$(false); echo $?'
check "exit7 sub"        'x=$(exit 7); echo $?'
check "plain zero"       'x=5; echo $?'
check "two assigns last" 'x=$(false) y=$(exit 2); echo $?'
check "two subs one rhs" 'x="$(false)$(exit 5)"; echo $?'
check "dollarq snapshot" 'false; x=$?; echo $x'
check "local keeps 0"    'f(){ local v=$(exit 9); echo $?; }; f'
check "prefix keeps cmd" 'x=$(exit 3) true; echo $?'
check "append sub"       'x=a; x+=$(exit 4); echo $?'

# ── #661: the same rule for a command that expands to NOTHING ─────────────────
# Added here rather than in a new file because it is the SAME status and the two
# rules interact: `run_assignment_list` resets `last_cmd_sub_status` so only its
# own RHS substitutions count, which is exactly what used to clobber the status
# the WORD expansion had recorded. The rows below pin both sides of that.
#
# huck returned 0 for all of these. That 0 was not cosmetic — see the loop rows.
check "bare sub exit3"     '$(exit 3); echo $?'
check "bare backtick"      '`exit 3`; echo $?'
check "bare sub notfound"  '$(nosuch_xyz) 2>/dev/null; echo $?'
check "last of two wins"   '$(exit 4) $(exit 3); echo $?'
check "last of two wins b" '$(exit 3) $(exit 4); echo $?'
check "assign prefix"      'x=1 $(exit 3); echo $?'
check "with a redirect"    '$(exit 3) >/dev/null; echo $?'
check "unset word first"   '$UNSET_XYZ $(exit 3); echo $?'
check "empty IFS"          'IFS=; $(exit 3); echo $?'
check "only THIS command"  'v=$(exit 3); $(exit 5); echo $?'
check "sub in a default"   'unset q; ${q:-$(exit 3)}; echo $?'
# An assignment RHS substitution WINS over the words', including when it
# returned 0 — which is why the fix keys on "did the assignment run one at all",
# not on "is the assignment status 0".
check "assign zero wins"   'x=$(true) $(exit 3); echo $?'
check "assign five wins"   'x=$(exit 5) $(exit 3); echo $?'
# No substitution ran => 0, NOT the previous command's status.
check "no sub resets"      'false; $UNSET_XYZ; echo $?'
check "no sub after 7"     '(exit 7); $UNSET_XYZ; echo $?'
check "empty var command"  'x=; $x; echo $?'
check "quoted empty word"  '"" 2>/dev/null; echo $?'
check "successful sub"     '$(true); echo $?'
check "whitespace sub"     '$(echo "   "); echo $?'
# A surviving word means the COMMAND's status, not the substitution's.
check "word survives"      '$(exit 3) echo hi; echo $?'

# ── the reason it mattered: a false condition must be false ───────────────────
# With status 0 these looped forever; the sweep's per-script timeout was the
# only thing that stopped them.
check "while cond"         'while `nosuch_xyz` 2>/dev/null; do echo LOOP; done; echo $?'
check "while sub exit1"    'while $(exit 1); do echo LOOP; done; echo $?'
check "until cond"         'until `true`; do echo LOOP; break; done; echo $?'
check "if cond"            'if `nosuch_xyz` 2>/dev/null; then echo T; else echo F; fi'
check "and-or"             '`nosuch_xyz` 2>/dev/null && echo AND || echo OR'
check "errexit exits"      'set -e; `nosuch_xyz` 2>/dev/null; echo SURVIVED'

harness_summary
