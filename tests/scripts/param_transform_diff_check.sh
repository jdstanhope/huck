#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v96: ${var@OP} parameter transforms (M-??).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# @U / @L / @u case transforms (ASCII only — non-ASCII inherits a documented
# Rust to_uppercase Unicode divergence, e.g. ß->SS).
check "upper"        'v=hello; echo "${v@U}"'
check "lower"        'v=HeLLo; echo "${v@L}"'
check "upper first"  'v=hello; echo "${v@u}"'
# @Q shell-quote
check "quote word"   'v=hello; echo "${v@Q}"'
check "quote space"  "v='a b'; echo \"\${v@Q}\""
check "quote squote" "v=\"a'b\"; echo \"\${v@Q}\""
check "quote empty"  'v=; echo "${v@Q}"'
check "quote unset"  'unset v; echo "[${v@Q}]"'
# @E backslash-escape expansion (deterministic escapes only)
check "escape tab"   'v='"'"'a\tb'"'"'; echo "${v@E}"'
check "escape nl"    'v='"'"'a\nb'"'"'; echo "${v@E}"'
check "escape unknown" 'v='"'"'a\qb'"'"'; echo "${v@E}"'
# @P prompt expansion (only \n — \u/\h/\w/\$ vary by user/host/cwd/uid)
check "prompt nl"    'v='"'"'x\ny'"'"'; echo "${v@P}"'
# v340 (#314): positional ${@<op>}/${*<op>} per-element transforms.
check "at unq subst"   'set aXa bXb cXc; for w in ${@/X/-};   do printf "<%s>" "$w"; done; echo'
check "at q   subst"   'set aXa bXb cXc; for w in "${@/X/-}"; do printf "<%s>" "$w"; done; echo'
check "star unq subst" 'set aXa bXb cXc; for w in ${*/X/-};   do printf "<%s>" "$w"; done; echo'
check "star q   subst" 'set aXa bXb cXc; for w in "${*/X/-}"; do printf "<%s>" "$w"; done; echo'
check "at q   rmpre"   'set aXa bXb cXc; for w in "${@#?}";   do printf "<%s>" "$w"; done; echo'
check "at q   rmsuf"   'set aXa bXb cXc; for w in "${@%?}";   do printf "<%s>" "$w"; done; echo'
check "at q   case"    'set foo bar baz; for w in "${@^^}";   do printf "<%s>" "$w"; done; echo'
check "at q   quoteQ"  'set "a b" c;     for w in "${@@Q}";   do printf "<%s>" "$w"; done; echo'
check "at q   ctrlA"   'e=$'"'"'uv\001\001wx'"'"'; set "$e" "$e"; for w in "${@/$'"'"'\001'"'"'/A}"; do printf "<%s>" "$w"; done; echo'
check "at empty args"  'set --; for w in "${@/X/-}"; do printf "<%s>" "$w"; done; echo DONE'
check "star q custom-IFS" 'IFS=-; set aXa bXb cXc; printf "<%s>" "${*/X/_}"; echo'
harness_summary
