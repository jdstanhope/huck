#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v90: extglob string matching (M-84).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'shopt -s extglob\n%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf 'shopt -s extglob\n%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
# NOTE: v90 is extglob STRING matching only. Pathname/filesystem globbing with
# extglob (echo +(a|b)) is deferred to v91 (M-84a) and not exercised here. The
# flag-OFF `[[` extglob quirk (bash honors it, huck requires the flag) is a
# documented M-84 divergence, also not exercised. Each fragment sets extglob on
# its OWN prior line (the check() printf) because huck tokenizes the whole
# logical line before executing it, so a same-line `shopt -s extglob` cannot
# affect extglob words later on that line (also an M-84 divergence).
check "dbracket +"     '[[ aab == +(a|b) ]] && echo y || echo n'
check "dbracket @"     '[[ abcd == @(ab|cd) ]] && echo y || echo n'
check "dbracket !"     '[[ foo == !(bar) ]] && echo y || echo n'
check "dbracket ?"     '[[ "" == ?(abc) ]] && echo y || echo n'
check "dbracket nest"  '[[ abbbc == @(a*(b)c) ]] && echo y || echo n'
check "dbracket class" '[[ file.txt == +([a-z]).txt ]] && echo y || echo n'
check "case extglob"   'case hello in +([a-z])) echo lc;; *) echo o;; esac'
check "case alt"       'case cd in @(ab|cd)) echo m;; *) echo o;; esac'
check "pe ## "         'v=aaab; echo "${v##+(a)}"'
check "pe %% "         'v=foobarbar; echo "${v%%+(bar)}"'
check "pe / "          'v=abcabc; echo "${v/+(abc)/X}"'
check "pe # shortest"  'v=aaab; echo "${v#+(a)}"'
check "quoted | literal" '[[ a == @("a|b") ]] && echo y || echo n'
check "quoted | match"   '[[ "a|b" == @("a|b") ]] && echo y || echo n'
check "var in group" 'x="a|b"; [[ ab == +($x) ]] && echo y || echo n'
harness_summary
