#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v120: printf %q (M-73) + set -f/noglob
# (M-08). File-arg execution (L-27: huck history-expands piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "q plain"       'printf "%q\n" plain'
check "q space"       'printf "%q\n" "a b"'
check "q squote"      'printf "%q\n" "c'"'"'d"'
check "q dollar glob" 'printf "%q\n" "a$b" "*" "?" "[a]"'
check "q empty"       'printf "[%q]\n" ""'
check "q safe"        'printf "%q\n" p/q-r.s_t=u@v%w'
check "q tilde hash"  'printf "%q\n" "~a" "a~" "#a" "a#"'
check "q control"     'printf "%q\n" "$(printf '"'"'a\tb'"'"')"'
check "q cycle"       'printf "%q\n" one two three'
check "q width"       'printf "[%6q]\n" "a b"'
check "q capture"     'printf -v x "%q" "a b"; echo "$x"'
check "noglob -f"     'd=$(mktemp -d); touch "$d"/x.txt; cd "$d"; set -f; echo *.txt; set +f; echo *.txt; rm -rf "$d"'
check "noglob -o"     'd=$(mktemp -d); touch "$d"/y.md; cd "$d"; set -o noglob; echo *.md; set +o noglob; echo *.md; rm -rf "$d"'
check "noglob opt"    'set -f; [[ -o noglob ]] && echo ON || echo OFF; set +f; [[ -o noglob ]] && echo ON || echo OFF'
check "noglob hasf"   'set -f; case "$-" in *f*) echo HASF;; *) echo no;; esac'
check "noglob pathonly" 'set -f; case abc in a*) echo CY;; esac; s=a1b; echo "${s//[0-9]/_}"; [[ x == ? ]] && echo BY'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
