#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v232: command-position-aware
# alias expansion (case patterns, reserved words, for-lists, [[ ]]).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliascase.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
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

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
