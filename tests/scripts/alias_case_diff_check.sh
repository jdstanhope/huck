#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v232: command-position-aware
# alias expansion (case patterns, reserved words, for-lists, [[ ]]).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliascase.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

# The regression: an aliased name used as a case pattern must not break parsing.
checkf "case pattern after pipe" \
  'shopt -s expand_aliases; alias ls="ls --color"; x=ls; case "$x" in use | ls | list) echo HIT ;; *) echo MISS ;; esac'
checkf "case subject not expanded" \
  'shopt -s expand_aliases; alias ll="echo BAD"; case ll in ll) echo OK ;; *) echo NO ;; esac'
checkf "case body command expands" \
  'shopt -s expand_aliases; alias ll="echo LL"; case x in x) ll ;; esac'
checkf "nested case patterns" \
  'shopt -s expand_aliases; alias ls="echo BAD"; case a in a) case b in ls) echo IN ;; *) echo X ;; esac ;; esac'
checkf "expand after then" \
  'shopt -s expand_aliases; alias g="echo G"; if true; then g; fi'
checkf "expand after do" \
  'shopt -s expand_aliases; alias g="echo G"; for i in 1 2; do g; done'
checkf "for-list words not expanded" \
  'shopt -s expand_aliases; alias one="echo BAD"; for w in one two; do echo "$w"; done'
checkf "double bracket interior" \
  'shopt -s expand_aliases; alias ll="echo BAD"; if [[ ll == ll ]]; then echo OK; fi'
checkf "reserved word slot" \
  'shopt -s expand_aliases; alias then="echo BAD"; if true; then echo OK; fi'

harness_summary
