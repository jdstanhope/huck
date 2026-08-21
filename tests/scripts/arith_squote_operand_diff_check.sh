#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a SINGLE-QUOTED operand in arithmetic
# (#660). bash expands an arithmetic body under double-quote rules: `"` is
# removed, `'` is NOT, so a single-quoted run reaches the arithmetic tokenizer
# with its quotes intact and is rejected. huck used to strip the quotes and
# evaluate, so it computed a value for expressions bash refuses — an ACCEPTANCE
# divergence, which is what these rows pin.
#
# COMPARED: stdout and the exit status, with stderr dropped. Both shells
# diagnose every row below; the diagnostic's leading `$0` diverges as it does
# for every builtin. What matters here is that huck no longer produces a VALUE.
#
# The `body:` rows compare the whole diagnostic minus that `$0`/`line N:`
# prefix — the echoed expression body AND the message tail, which agree since
# #659. The line NUMBER is excluded: for a body spanning lines bash names the
# line it ENDS on and huck the line it starts on (the #649/#644 family).
#
# NOT compared here:
#   - `${arr[ '1' ]}`: an array SUBSCRIPT is not lexed in arithmetic mode, so it
#     still evaluates a single-quoted operand (#699).
#   - backslash escapes other than `\`+newline inside a single-quoted span: bash
#     collapses `\\` to `\` in the echoed body, huck keeps both (#700).
#   - `$(( '))' ))`: huck reports an unterminated-quote EOF where bash reports
#     the operand error — a delimiting difference that predates this change.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# stdout + status only; stderr dropped (see the note above).
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# The echoed expression body: everything between `line N: ` and the message.
# Isolates the part of the diagnostic both shells agree on.
checkbody() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //')
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //')
    compare "$label" "$b" "$h"
}

# Same as checkbody, but drives a SCRIPT FILE instead of piped stdin, and keeps
# the whole diagnostic minus the `$0`/`line N:` prefix and the message tail.
# Needed for a body that spans lines: huck's stdin reader joins a `\`+newline
# before the lexer sees it (#701), so only the file driver exercises the arith
# scanner's own handling of it.
checkbody_file() {
    local label="$1" b h tmp
    tmp=$(mktemp)
    cat > "$tmp"
    # `sed -z` so the prefix strip sees the whole capture as one record: the
    # body (and bash's error token, which repeats it) spans lines.
    b=$(bash --norc --noprofile "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //')
    h=$("$HUCK_BIN" "$tmp" 2>&1 >/dev/null | sed -z 's/^[^ ]*: line [0-9]*: //')
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

# --- a single-quoted operand is refused, not evaluated ---
check "\$((: quoted operand"     "echo \$(( '5' + 1 ))"
check "\$((: quoted rhs"         "echo \$(( 1 + '2' ))"
check "\$((: quoted var name"    "x=5; echo \$(( 'x' ))"
check "\$((: empty quotes"       "echo \$(( '' ))"
check "\$((: whole expression"   "echo \$(( '1 + 2' ))"
check "\$[: quoted operand"      "echo \$[ '5' + 1 ]"
check "((: quoted operand"       "(( '5' )); echo \"rc=\$?\""
check "for: quoted init"         "for (( i='0'; i<2; i++ )); do echo \$i; done"
check "let: quoted operand"      "a=1; let \"a = '2'\"; echo \"rc=\$? a=\$a\""
check "\$((: adjacent text"      "echo A\$(( '1' ))B"

# --- DOUBLE quotes are still removed, and ordinary arithmetic is unaffected ---
check "regress: dquoted expr"    "echo \$(( \"1 + 2\" ))"
check "regress: dquoted operand" "echo \$(( \"1\" + 2 ))"
check "regress: plain arith"     "echo \$(( 1 + 2 * 3 ))"
check "regress: var operand"     "x=5; echo \$(( x + 1 ))"
check "regress: nested \$(( ))"  "echo \$(( \$(( 2 + 3 )) * 2 ))"
check "regress: \$[ plain ]"     "echo \$[ 2 + 3 ]"
check "regress: (( )) true"      "(( 1 + 1 )); echo \"rc=\$?\""
check "regress: for header"      "for (( i=0; i<2; i++ )); do echo \$i; done"

# --- the echoed body agrees, including `\`+newline inside the span, where bash
#     strips the backslash but does NOT join the lines ---
checkbody "body: quoted operand"  "echo \$(( '5' + 1 ))"
checkbody "body: quoted rhs"      "echo \$(( 1 + '2' ))"
checkbody "body: empty quotes"    "echo \$(( '' ))"
checkbody "body: \$[ operand"     "echo \$[ '5' + 1 ]"
# A `\`+newline inside a single-quoted span: bash strips the backslash but does
# NOT join the lines, so the echoed body carries a real newline.
checkbody_file "body: backslash-nl" <<'FRAG'
echo $(( '1 \
2' ))
FRAG

harness_summary
