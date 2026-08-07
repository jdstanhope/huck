#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #284: a NON-interactive piped-stdin
# session aborts the rest of the input on a syntax error (exit 2), like a
# script file — it does not recover and keep running. A runtime non-zero
# status does NOT abort, and an `eval` syntax error stays local.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Feed the fragment through piped stdin; normalize the program-name prefix.
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #; s#^(bash|.*/huck|huck): (eval: )#SH: \2#"; }
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%b' "$frag" | bash 2>&1 | norm; echo "EXIT:${PIPESTATUS[0]}")
    h=$(printf '%b' "$frag" | "$HUCK_BIN" 2>&1 | norm; echo "EXIT:${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}

# Syntax error mid-stream aborts the remainder (exit 2).
check "abort after err"   'echo a\nfor()\necho b\n'
check "abort first line"  'for()\necho after\n'
check "abort last line"   'echo a\nfor()\n'
check "clean multi runs"  'echo a\necho b\n'
# A runtime non-zero status does NOT abort.
check "runtime 2 continues" 'f() { return 2; }\nf\necho still-here\n'
check "false continues"   'false\necho keep-going\n'
# eval's syntax error stays local (does not abort the outer stdin session).
check "eval err local"    'eval "for()"\necho after-eval\n'
# A different syntax error (unexpected `)`) also aborts the remainder.
check "bad paren aborts"  'echo a\n)\necho b\n'
# NOTE: an unterminated-quote-at-EOF also aborts (the #284 behavior this
# harness targets), but its piped-stdin error WORDING diverges from bash
# ("unexpected end of input" vs "unexpected EOF while looking for matching")
# — a separate pre-existing divergence tracked in its own issue, not here.

harness_summary
