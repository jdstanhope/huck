#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v181: the two bash `$`-quote forms.
#   `$'…'`  ANSI-C quoting — special ONLY outside double quotes; inside `"…"`
#           the `$` is a literal char (huck used to crash: "unterminated quote").
#   `$"…"`  locale-translation quoting — identity here (no message catalog), so
#           `$"…"` ≡ `"…"` (huck used to leak a leading `$`).
# All cases print clean stdout, rc 0 in bash → compare full stdout+exit.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

check() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# $' inside double quotes — literal `$` + `'` (was a crash).
check "dq dollar-squote"        'echo "$'\''"'
check "dq a dollar-squote b"    'echo "a$'\''b"'
check "dq regex-anchor end"     'echo "ping6)$'\''"'
check "assign dq dollar-squote" 'x="cost $'\''n"; echo "$x"'
check "nested quote-switch"     'echo '\''a "b'\''"'\''c)$'\''"'\''d'\'''

# $"…" locale translation = identity (drop the leading `$`).
check "locale plain"            'echo $"hello"'
check "locale with expansion"   'x=Z; echo $"a $x b"'
check "locale empty"            'echo $""'
check "locale escaped dquote"   'echo $"with \"escaped\" and $x"'

# Controls — unquoted ANSI-C still decodes; plain quotes unaffected.
check "unquoted ANSI-C escapes" 'echo $'\''a\tb\nc'\'''
check "plain dquote"            'echo "x"'
check "plain squote"            'echo '\''y'\'''

harness_summary
