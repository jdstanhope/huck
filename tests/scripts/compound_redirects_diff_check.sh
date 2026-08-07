#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v97: compound-command redirections.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT

check "heredoc while"    $'while read x; do echo "g:$x"; done <<EOF\na\nb\nEOF'
check "herestring while" "while read x; do echo \"[\$x]\"; done <<< 'one two'"
check "for >file"        "for i in 1 2; do echo \$i; done > '$FIX/a'; cat '$FIX/a'"
check "if >file"         "if true; then echo hi; fi > '$FIX/b'; cat '$FIX/b'"
check "brace >file"      "{ echo a; echo b; } > '$FIX/c'; cat '$FIX/c'"
check "subshell >file"   "( echo x ) > '$FIX/d'; cat '$FIX/d'"
check "case >file"       "case z in z) echo m;; esac > '$FIX/e'; cat '$FIX/e'"
check "until >file"      "n=0; until [ \$n -ge 2 ]; do echo \$n; n=\$((n+1)); done > '$FIX/h'; cat '$FIX/h'"
check "append brace"     "echo first > '$FIX/f'; { echo second; } >> '$FIX/f'; cat '$FIX/f'"
check "stderr to file"   "{ echo out; echo err >&2; } 2> '$FIX/g'; cat '$FIX/g'"
check "no-redir for"     "for i in 1 2; do echo \$i; done"
harness_summary
