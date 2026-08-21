#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a `$` that survives into arithmetic
# (#707).
#
# Every caller expands `$`-forms BEFORE the arithmetic parser runs, so a `$`
# still present at that point is one the user ESCAPED or QUOTED. bash has no `$`
# token in its arithmetic grammar at all and reports `syntax error: operand
# expected` on it. huck's tokenizer had a `$name` arm and read the variable — so
# the escape was a no-op and huck produced a VALUE where bash refuses:
#
#     x=5; echo $(( 1+\$x ))      bash: syntax error …      huck: 6
#     x=5; [[ '$x' -eq 5 ]]       bash: syntax error, rc 1  huck: rc 0
#
# COMPARED: the diagnostic with the `$0`/`line N:` prefix stripped, plus stdout
# and status.
#
# NOT compared here (both PRE-EXISTING, verified against the pre-fix binary, and
# reached by `@` exactly as by `$`):
#   - `[[ … ]]` and a `for (( … ))` header: huck's arithmetic diagnostic omits
#     the expression echo and the error-token clause, and `[[ ]]` exits 2 where
#     bash exits 1 (#711). Plain `(( … ))` and `$(( … ))` are unaffected and are
#     compared in full below.
#   - `declare -i v=@` / `v='$x'`: huck swallows the arithmetic error, assigns 0
#     and exits 0 where bash reports and refuses (#712).
#   - `$[ 1+\$x ]`: the legacy form's two-pass model keeps the backslash and then
#     expands, so its body is `1+\5` (#709).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Diagnostic minus the `$0`/`line N:` prefix, plus stdout and status.
check() {
    local label="$1" frag="$2" b h
    b=$( printf '%s\n' "$frag" | bash --norc --noprofile 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?" )
    h=$( printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?" )
    compare "$label" "$b" "$h"
}

# Status and stdout only — for shapes whose MESSAGE is #711.
check_rc() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Succeeded-vs-failed plus stdout, for shapes whose exact STATUS is #711.
check_fails() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; [ $? -eq 0 ] && echo OK || echo FAILED)
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; [ $? -eq 0 ] && echo OK || echo FAILED)
    compare "$label" "$b" "$h"
}

# --- an escaped `$` is refused, not expanded ---
check 'escaped $ in $(( ))'   'x=5; echo $(( 1+\$x ))'
check 'escaped $ alone'       'x=5; echo $(( \$x ))'
check 'escaped $ in "…"'      'x=5; echo $(( 1+"\$x" ))'
check 'bare $ operand'        'echo $((1 + $))'
check 'bare $ at start'       'echo $(($))'
check '$ in (( ))'            'x=5; (( 1+\$x )); echo "rc=$?"'
check '$ in let'              'x=5; let "v = \$x"; echo "rc=$?"'
check_rc '$ in a for header'  'x=5; for (( i=\$x; i<1; i++ )); do echo BODY; done'

# --- a quoted `$` reaching `[[ ]]` now FAILS instead of silently passing.
#     Compared as succeeded-vs-failed: the STATUS itself is #711 (1 vs 2). ---
check_fails 'quoted $ in [[ -eq ]]' 'x=5; [[ '"'"'$x'"'"' -eq 5 ]]'
check_fails 'quoted $ in [[ -lt ]]' 'x=5; [[ '"'"'$x'"'"' -lt 9 ]]'
check_fails 'regress: [[ @ -eq ]]'  '[[ @ -eq 5 ]]'

# --- an UNESCAPED `$` is expanded before the parser and still works ---
check 'regress: $x operand'    'x=5; echo $(( $x + 1 ))'
check 'regress: bare name'     'x=5; echo $(( x + 1 ))'
check 'regress: ${x} operand'  'x=5; echo $(( ${x} + 1 ))'
check 'regress: $((nested))'   'x=5; echo $(( $(( $x )) * 2 ))'
check 'regress: $x subscript'  'x=1; a=(7 8); echo ${a[$x]}'
check 'regress: [[ $x -eq ]]'  'x=5; [[ $x -eq 5 ]]; echo "rc=$?"'
check 'regress: $x in (( ))'   'x=5; (( $x == 5 )); echo "rc=$?"'
check 'regress: $ in a string' 'echo "costs $5"'

harness_summary
