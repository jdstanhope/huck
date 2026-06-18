#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v186: `case … esac` statements inside
# `$( … )`. A bare case-pattern `)` must not close the substitution; `case` as an
# argument is a plain word. Kernel mlxsw_lib.sh hit this. rc 0 in bash → compare
# full stdout+exit.
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

check "case in cmdsub"        'y=a; echo "$(case $y in a) echo hit;; *) echo no;; esac)"'
check "case as arg"           'echo "$(echo case)"'
check "if then case"          'y=a; echo "$(if true; then case $y in a) echo T;; esac; fi)"'
check "nested case"           'y=a; echo "$(case $y in a) case $y in a) echo deep;; esac;; esac)"'
check "alternation pattern"   'y=b; echo "$(case $y in a|b) echo alt;; esac)"'
check "fallthrough ;;&"       'y=a; echo "$(case $y in a) echo A;;& *) echo B;; esac)"'
check "parenthesized pattern" 'y=a; echo "$(case $y in (a) echo p;; esac)"'
check "case word after pipe"  'echo "$(echo x | grep case || echo none)"'
check "mlxsw real shape"      'C=spectrum2; echo "$(case $C in spectrum) echo 1;; spectrum*) echo ${C#spectrum};; esac)"'
check "case clause has cmdsub" 'y=a; echo "$(case $y in a) echo $(echo inner);; esac)"'
check "control no case"       'echo "$(echo a; echo b)"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
