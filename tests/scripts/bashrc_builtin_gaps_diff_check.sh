#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v107: three bashrc builtin gaps —
#   * `[[ -o optname ]]` — test a `set -o` option's state inside `[[ ]]`
#     (known names report on/off; an unknown name is silently off / rc 1).
#   * `declare -g` — force a declaration into the GLOBAL scope from inside a
#     function (so it outlives the call), vs a plain `declare` which is local.
#   * `unset -f` / `unset -v` — remove a function / a variable by kind.
# Each fragment's combined stdout+stderr+exit is compared verbatim.
#
# Notes on the `unset -f` fragment: function existence is probed with
# `declare -F g` (silent, rc 1, no output on a missing function in both
# shells), NOT `type g`. `type` emits its "not found" diagnostic to the real
# stderr in huck (pre-existing M-90 divergence) which would bypass the 2>&1
# capture and break the byte-diff; `declare -F` compares cleanly.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# 1: [[ -o optname ]] — a known option (emacs is off when non-interactive)
check "[[ -o emacs ]]" '[[ -o emacs ]] && echo on || echo off'

# 2: [[ -o pipefail ]] — reflects a just-enabled option
check "[[ -o pipefail ]]" 'set -o pipefail; [[ -o pipefail ]] && echo on || echo off'

# 3: [[ -o bogusname ]] — unknown option is silently off (rc 1)
check "[[ -o bogusname ]]" '[[ -o bogusname ]] && echo on || echo off'

# 4: declare -g escapes the function scope (global G survives)
check "declare -g global" 'f() { declare -g G=5; }; f; echo "[${G-}]"'

# 5: plain declare stays local (L is unset after the call)
check "declare local" 'f() { declare L=5; }; f; echo "[${L-}]"'

# 6: unset -f removes a function (declare -F probes existence silently)
check "unset -f function" 'g() { echo hi; }; unset -f g; declare -F g >/dev/null && echo found || echo gone'

# 7: unset -v removes a variable
check "unset -v variable" 'v=1; unset -v v; echo "[${v-}]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
