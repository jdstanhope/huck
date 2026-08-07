#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #380: a word-start `~` in the
# REPLACEMENT half of `${x/pat/rep}` is a tilde prefix. It expands even when
# the whole `${…}` is double-quoted — unlike the `:-` value operand, which
# stays literal there — while a directly-quoted or backslash-escaped `~` is
# literal in both.
#
# HOME is pinned so the expected text is stable regardless of who runs this.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(HOME=/h/u bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(HOME=/h/u "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# --- the replacement expands a word-start tilde -----------------------------
check "unquoted"          'x=a/b; echo ${x/a/~}'
check "enclosing quotes"  'x=a/b; echo "${x/a/~}"'
check "replace all"       'x=aXa; echo ${x//a/~}'
check "anchored prefix"   'x=ab; echo ${x/#a/~}'
check "anchored suffix"   'x=ab; echo ${x/%b/~}'
check "tilde then path"   'x=a; echo ${x/a/~/sub}'
check "tilde then path q" 'x=a; echo "${x/a/~/sub}"'
check "~+ is PWD"         'cd /tmp; x=a; echo ${x/a/~+}'
check "~- is OLDPWD"      'cd /tmp; cd /; x=a; echo ${x/a/~-}'
check "empty replacement" 'x=ab; echo "[${x/a/}]"'

# --- but only at word start, and only unquoted ------------------------------
check "mid-word tilde"    'x=abc; echo ${x/b/x~y}'
check "trailing tilde"    'x=a; echo ${x/a/b~}'
check "inner quotes"      'x=a; echo "${x/a/"~"}"'
check "backslash escape"  'x=a; echo "${x/a/\~}"'
check "single quotes"     "x=a; echo \${x/a/'~'}"
# The PATTERN half keeps its own rule (unanchored only, unquoted only).
check "pattern half"      'x=~; echo "${x/#~/T}"'

# --- the neighbouring operands are unchanged --------------------------------
check ":- quoted stays"   'unset u; echo "${u:-~}"'
check ":- unquoted"       'unset u; echo ${u:-~}'
check "substring tilde"   'x=abcdef; echo "[${x:1:~1}]"'   # ~ is arithmetic NOT here
check "substring offset"  'x=abcdef; echo "[${x: ~1}]"'
check "substring plain"   'x=abcdef; echo "[${x:2:3}]"'
# A `~` coming from an expansion is never a tilde prefix.
check "tilde from var"    'x=a; t="~"; echo ${x/a/$t}'
check "HOME from var"     'x=a; r=~; echo ${x/a/$r}'

harness_summary
