#!/usr/bin/env bash
# Byte-identical bash<->huck harness for backtick command-substitution backslash
# rules. Inside `...`, backslash keeps its literal meaning EXCEPT before $, `, or
# \ (there it de-escapes one level before the inner command is re-lexed). Only the
# basics were covered before; this exercises the \$, \\, \n, escaped-backtick, and
# quoted-context interactions.
#
# Fragments are written to a temp file and run in FILE MODE (bash "$tmp" vs
# huck "$tmp") so the harness's own shell quoting never double-escapes the
# backslashes under test — same idiom as alias_expand_diff_check.sh's checkf.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {  # label ; body — assert byte-identical stdout+stderr+exit (file mode)
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-btick.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the core three (\$ / \\ / \n de-escaping inside `...`) ---
checkf 'esc-dollar then expand' 'FOO=bar; echo `echo \$FOO`'   # \$ -> $, then $FOO expands -> bar
checkf 'double-backslash'       'echo `echo \\`'               # \\ -> a single backslash
checkf 'backslash-n literal'    'echo `echo \n`'               # \n is not special -> literal n

# --- controls / adjacent semantics ---
checkf 'plain backtick'         'echo `echo hi`'               # -> hi
checkf 'esc-dollar named var'   'V=hi; echo `echo \$V`'        # -> hi
checkf 'esc-dollar in dquotes'  'V=zz; echo "`echo \$V`"'      # \$ survives outer "" -> zz
checkf 'backtick body in dq'    'echo "`echo hi`"'             # -> hi
checkf 'double-bslash in dq'    'echo "`echo \\`"'             # \\ -> \ inside "`...`"

# --- escaped backtick nests one level ---
checkf 'escaped-backtick nest'  'echo `echo \`echo nested\``'  # \` opens an inner `...` -> nested

# --- backslash before a non-special char is preserved verbatim ---
checkf 'esc-quote literal'      'echo `echo \"quoted\"`'       # \" -> literal "quoted"
checkf 'bslash mid-word'        'echo `echo a\\b`'             # \\ -> \, echo eats it -> ab
checkf 'esc-hash not comment'   'echo `echo \# comment`'       # \# -> literal # comment
checkf 'bslash-n mid-word'      'echo `echo one\ntwo`'         # -> onentwo (n is literal)

# --- multi-command body ---
checkf 'semicolon body'         'echo `echo foo; echo bar`'    # -> foo bar

# DIVERGENCE (reported): `echo \\\$X` inside backticks.
#   FRAGMENT:  X=1; echo `echo \\\$X`
#   bash:      $X            (\\ -> \, \$ -> $, giving inner `echo \$X` -> literal $X)
#   huck:      1\            (mis-orders the de-escape: expands $X and keeps a stray \)
#
# DIVERGENCE (reported): four backslashes inside backticks.
#   FRAGMENT:  echo `echo \\\\`
#   bash:      \             (\\\\ -> \\, inner `echo \\` -> single \)
#   huck:      syntax error: expected a command  (exit 2)
#
# DIVERGENCE (reported): escaped backtick guarded by double backslash.
#   FRAGMENT:  echo `echo \\\`lit\\\``
#   bash:      `lit`         (\\ -> \, \` -> literal `, giving inner `echo \`lit\`` )
#   huck:      syntax error: expected a command  (exit 2)

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
