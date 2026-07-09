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
checkf_rc() {  # label ; body — assert stdout+exit only, stderr TEXT excluded.
    # Used only for malformed-body cases where bash and huck BOTH correctly
    # reject (same exit code, same — empty — stdout) but the rejection message
    # differs because huck routes errors through its own error-prologue
    # emitter (program-name/format), not bash's. Exact error TEXT is an
    # accepted non-goal (spec §5); this narrows the assertion so a rejection
    # message spelling difference doesn't fail an otherwise-correct case.
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-btick.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>/dev/null; echo "EXIT:$?")
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

# --- L-70 fixes: previously-divergent cases now pass (capture-unescape-relex) ---
checkf 'run3 before X'      'echo `echo \\\X`'        # \\ -> \, \X kept -> \X
checkf 'run4 pure'          'echo `echo \\\\`'         # \\\\ -> \\, inner `echo \\` -> \
checkf 'esc-bt dbl-bslash'  'echo `echo \\\`lit\\\``'  # \\ -> \, \` -> literal ` -> `lit`
checkf 'run2 before dollar' 'X=1; echo `echo \\\$X`'   # \\ -> \, \$ -> $ -> literal $X

# --- backslash-run parity: N backslashes before X (file mode, exact bytes) ---
for n in 0 1 2 3 4 5 6 7 8; do
  bs=$(printf '%*s' "$n" '' | tr ' ' '\\')
  checkf "run$n-X" "echo \`echo ${bs}X\`"
  # "run$n-close": a backslash run right before the CLOSING backtick. For odd n
  # the trailing backslash escapes the close, so both bash and huck correctly
  # reject as an unterminated substitution — but the rejection message text
  # differs (huck's error-prologue vs bash's), so this uses checkf_rc
  # (stdout+exit only) uniformly; for even n neither side errors, so
  # checkf_rc's narrower comparison is equally exact there.
  checkf_rc "run$n-close" "echo \`echo a${bs}\`"
done

# --- quote-blind close: bash closes at a backtick inside quotes (both ERROR;
#     exit codes match but the rejection message text differs per the
#     error-prologue divergence — see checkf_rc above) ---
checkf_rc 'sq hides nothing'  "echo \`echo '\`' hi\`"
checkf_rc 'literal bt in sq'  "x=\`printf '%s' 'a\`b'\`; echo \"[\$x]\""

# --- $() inside a backtick body ---
checkf 'dollarparen inbt'  'echo `echo $(echo hi)`'
checkf 'dollarparen tail'  'echo `echo $(echo X)Y`'

# --- nesting depth 2-3 (must remain correct) ---
checkf 'nest depth2'       'echo `echo \`echo inner\``'
checkf 'nest depth3'       'echo `echo \`echo \\\`echo deep\\\`\``'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
