#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v143: compgen -d/-f tilde + slash
# prefixes (the building block under bash-completion's _cd / _filedir).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
TMP=$(mktemp -d) || exit 1
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/projects/alpha" "$TMP/projects/beta" "$TMP/pub"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(HOME="$TMP" bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(HOME="$TMP" "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Quoted ~/ => literal, reaches compgen unexpanded (the _filedir case).
check "compgen -d quoted ~/"       'compgen -d -- "~/" | sort'
check "compgen -d var ~/"          'cur="~/"; compgen -d -- "$cur" | sort'
check "compgen -d var ~/pro"       'cur="~/pro"; compgen -d -- "$cur" | sort'
check "compgen -d var ~/projects/" 'cur="~/projects/"; compgen -d -- "$cur" | sort'
check "compgen -f var ~/projects/" 'cur="~/projects/"; compgen -f -- "$cur" | sort'
# Relative slash prefix (already worked; coverage).
check "compgen -d projects/"       'cd ~ && compgen -d -- projects/ | sort'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
