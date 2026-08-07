#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v323 (#256): a DEBUG<->ERR (or
# DEBUG<->RETURN) mutual-trigger no longer stack-overflows huck. The old
# single-slot `firing_trap` recursion guard only checked the innermost
# in-flight signal, so a DEBUG action firing ERR (or vice versa) alternated
# the slot and neither same-signal guard ever fired -> unbounded recursion
# -> SIGABRT. Fixed by tracking the SET of active trap signals and guarding
# on membership.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "err+debug mutual (crash repro)" 'trap false ERR; trap false DEBUG; echo x'
check "debug action fails w/ err trap" 'd=0;e=0; trap '"'"'d=$((d+1)); false'"'"' DEBUG; trap '"'"'e=$((e+1)); false'"'"' ERR; echo start; echo "d=$d e=$e"'
check "debug during err, no err reentry" 'n=0; trap '"'"'n=$((n+1)); echo "D:$n"'"'"' DEBUG; trap '"'"'echo ERR; false; false'"'"' ERR; echo hi; false; echo "n=$n"'
check "return+debug under functrace" 'set -T; f(){ echo A; false; }; trap false DEBUG; trap false RETURN; f; echo done'
check "lone debug unchanged" 'n=0; trap '"'"'n=$((n+1))'"'"' DEBUG; :; :; echo "n=$n"'
check "lone err once per failure" 'e=0; trap '"'"'e=$((e+1))'"'"' ERR; false; true; false; echo "e=$e"'

harness_summary
