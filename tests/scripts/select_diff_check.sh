#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for v81 `select` loops and the
# M-24a no-`in` `for` positional fix.  Fragments fall into two categories:
#
#   1. select: Program alone on stdin (no menu input) — both bash and huck
#      exhaust stdin reading the program, then `select`'s `read` hits EOF.
#      Both print the menu+PS3 to stderr and exit.  Output is byte-for-byte
#      identical at matching COLUMNS settings.
#
#   2. for no-`in` / empty-`in`: No interactive read involved; output is
#      identical to plain bash with no COLUMNS dependency.
#
# Compare outputs via `COLUMNS=N bash/huck 2>&1 | cat -A` to expose every
# control character (tabs, trailing spaces, etc.) byte-for-byte.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

# check LABEL COLUMNS FRAGMENT
# Run FRAGMENT through bash and huck with COLUMNS set, combine stdout+stderr,
# append EXIT:N, and diff byte-for-byte.
check() {
    local label="$1"
    local cols="$2"
    local fragment="$3"
    local bash_out huck_out bash_exit huck_exit

    bash_out=$(printf '%s\n' "$fragment" | COLUMNS="$cols" bash 2>&1)
    bash_exit=$?
    huck_out=$(printf '%s\n' "$fragment" | COLUMNS="$cols" "$HUCK_BIN" 2>&1)
    huck_exit=$?

    bash_out="${bash_out}
EXIT:${bash_exit}"
    huck_out="${huck_out}
EXIT:${huck_exit}"

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(printf '%s\n' "$bash_out") <(printf '%s\n' "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. Menu layout at COLUMNS=80 (standard width).
check "menu COLUMNS=80" 80 \
    'select x in one two three four five six seven eight nine ten; do echo $x; done'

# 2. Same program at COLUMNS=40 (narrower — forces more rows, fewer columns).
check "menu COLUMNS=40" 40 \
    'select x in one two three four five six seven eight nine ten; do echo $x; done'

# 3. Same program at COLUMNS=110 (very wide — triggers single-column flip
#    because rows==1 would need only 1 row but bash renders as single-column
#    in this regime; huck's ported print_select_list matches the same flip).
check "menu COLUMNS=110 single-column flip" 110 \
    'select x in one two three four five six seven eight nine ten; do echo $x; done'

# 4. Items with mixed widths at COLUMNS=80.
check "mixed widths COLUMNS=80" 80 \
    'select x in aaa bbbbbbbb cc dddddddddddd ee ff; do echo $x; done'

# 5. 12 items (2-digit numbering) at COLUMNS=80 — exercises the wider
#    first-column number format.
check "12 items 2-digit COLUMNS=80" 80 \
    'select x in i1 i2 i3 i4 i5 i6 i7 i8 i9 i10 i11 i12; do echo $x; done'

# 6. Custom PS3 prompt at COLUMNS=80.
check "custom PS3 COLUMNS=80" 80 \
    'PS3="choose> "; select x in a b; do echo $x; done'

# 7. Empty `in` list — no menu printed, no body run, script continues.
check "empty in list" 80 \
    'select x in; do echo never; done; echo after'

# 8. `for x;` no-`in` positionals (M-24a) — iterates `"$@"` (set via set --).
check "for no-in iterates positionals" 80 \
    'set -- a b c; for x; do printf "%s " "$x"; done; echo'

# 9. Explicit empty `in` for-loop — iterates nothing, prints after.
check "for explicit empty in" 80 \
    'set -- a b c; for x in ; do echo never; done; echo after'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
