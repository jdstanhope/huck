#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v116: [^...] bracket negation in glob
# patterns (M-113) — ${}/case/[[ ]]/pathname. [!...] + literal-^ regressions.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# Run each fragment as a FILE-ARG script (not piped stdin) for both shells. A
# `[!...]` fragment contains `!` which huck history-expands on piped stdin (a
# separate divergence; bash disables histexpand on non-interactive stdin too).
# File-arg execution is the true non-interactive path (matches scripts/source)
# and isolates this harness to the [^...] bracket-negation feature under test.
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    compare "$label" "$b" "$h"
}

check "subst negated"         'v=abc123; echo "${v//[^0-9]/}"'
check "remove-prefix negated" 'v=abc123; echo "${v#[^0-9]}"'
check "remove-suffix negated" 'v=x9y; echo "${v%[^0-9]}"'
check "case negated"          'case A in [^0-9]) echo letter;; *) echo other;; esac'
check "case negated digit"    'case 5 in [^0-9]) echo letter;; *) echo other;; esac'
check "dbracket negated"      '[[ A == [^0-9] ]] && echo Y || echo N'
check "dbracket negated neg"  '[[ 5 == [^0-9] ]] && echo Y || echo N'
check "dbracket neq negated"  '[[ A != [^0-9] ]] && echo Y || echo N'
check "bang still negates"    'v=abc123; echo "${v//[!0-9]/}"'
check "caret literal"         'v=a^bc; echo "${v//[a^b]/}"'
check "non-negated class"     'v=abc123; echo "${v//[0-9]/}"'
check "pathname negated"      'd=$(mktemp -d); touch "$d"/afile "$d"/bfile "$d"/cfile; cd "$d"; echo [^a]file; rm -rf "$d"'

harness_summary
