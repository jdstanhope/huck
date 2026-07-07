#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 C: shopt expand_aliases in file mode.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliasx.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "def then use"     'shopt -s expand_aliases; alias foo="echo HELLO"; foo'
checkf "alias with arg"   'shopt -s expand_aliases; alias ll="echo LL"; ll /usr'
checkf "no shopt = literal" 'alias foo="echo HELLO"; foo'
checkf "unalias then use" 'shopt -s expand_aliases; alias foo="echo HI"; foo; unalias foo; foo'
checkf "trailing space"   'shopt -s expand_aliases; alias a="b "; alias b="echo"; a hi'
checkf "redefine"         'shopt -s expand_aliases; alias g="echo one"; g; alias g="echo two"; g'
checkf "set -v echo raw"  'set -v; shopt -s expand_aliases; alias ll="echo LL"; ll /usr'

# v239 T6: cross-unit def-then-use — alias defined on line N, used on line N+1.
# The live lexer's between-unit set_aliases refresh makes the new alias visible
# to the parser of the NEXT unit. Same-unit (semicolon) must still NOT expand.
checkf "cross-unit def-then-use" $'shopt -s expand_aliases\nalias greet=\'echo hi\'\ngreet'
checkf "cross-unit same-unit no-expand" $'shopt -s expand_aliases\nalias greet=\'echo hi\'; greet'
checkf "cross-unit unalias mid-run" $'shopt -s expand_aliases\nalias g=\'echo GO\'\ng\nunalias g\ng'

# v266: alias bodies carrying word-part openers ($()/backtick/${}). The input-
# source stack lexes the body inline as atoms, so the openers flow to the parser
# instead of spinning a standalone re-lexer (the old batch-tokenize spin case).
checkf "body has \$()"     $'shopt -s expand_aliases\nalias now=\'echo $(echo 2)\'\nnow'
checkf "body has backtick" $'shopt -s expand_aliases\nalias b=\'echo `echo bt`\'\nb'
checkf "body has \${}"     $'shopt -s expand_aliases\nX=home\nalias q=\'echo ${X:+yes}\'\nq'
# v266: recursion guard spans the whole body frame (a ;-reuse of NAME in its own
# body is guarded), and a mutual a->b->a chain terminates.
checkf "self-recursion once" $'shopt -s expand_aliases\nalias ls=\'ls -a\'\ntype ls'
checkf "body \$() with arg"  $'shopt -s expand_aliases\nalias g=\'grep -n\'\nprintf \'a\\nb\\n\' | g b'

# v266: piped-stdin (non-interactive) alias expansion must honor
# `shopt -s expand_aliases` exactly like file/-c mode — the REPL/piped-stdin
# reader (repl.rs `do_alias`) previously gated only on `is_interactive`, so
# `printf … | huck` never expanded aliases even with the shopt set. These run
# each fragment through STDIN (not a temp file) to exercise that path.
checkstdin() {
    local label="$1" body="$2" b h
    b=$(printf '%s' "$body" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s' "$body" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Positive cases (clean stdout, byte-comparable). The negative "no shopt = no
# expand" case is verified by hand to match bash's BEHAVIOR (no expansion) but is
# omitted here: it yields a `command not found` error whose program-name prefix
# and line-number differ from bash (the known L-13/L-16 error-text class), not an
# alias-behavior divergence.
checkstdin "stdin shopt then def then use" $'shopt -s expand_aliases\nalias now=\'echo 2\'\nnow'
checkstdin "stdin cmdsub body via shopt"   $'shopt -s expand_aliases\nalias now="echo $(echo 2)"\nnow'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
