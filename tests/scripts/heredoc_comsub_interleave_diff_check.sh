#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the heredoc<->command-substitution
# interleaving cluster (parse-gaps-round2). Fixed here:
#   * a heredoc delimiter word with a `\<newline>` line continuation
#     (`<<\EOT\` + newline forms delimiter `EOT4`) — comsub4.sub block 2;
#   * `)`/`(` terminate the (unquoted) heredoc delimiter word so a heredoc
#     opened inside `$( … )` closes the substitution correctly — heredoc7.sub
#     `echo $(cat <<EOF)`;
#   * a heredoc opened INSIDE a `$( … )` whose `)` closes before the body was
#     collected takes its body from the lines following the ENCLOSING command
#     line (delayed heredoc across the comsub boundary).
# Each fragment is run from a temp FILE (heredoc EOF-termination is sensitive to
# batch-vs-interactive) and compared on full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

check() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" > "$TMP/f.sh"
    b=$(bash --norc --noprofile "$TMP/f.sh" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$TMP/f.sh" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- comsub4.sub: quoted-delimiter heredoc inside $( ), literal backslash body
check "quoted-delim heredoc in comsub" 'x=$( cat <<\EOT
d \
g
EOT
)
echo "$x"'

# delimiter formed across a `\<newline>` continuation: <<\EOT\ + NL + 4 => EOT4
check "delim line-continuation" 'x=$( cat <<\EOT\
4
d \
g
EOT4
)
echo "$x"'

check "single-quoted delim in comsub" "x=\$(cat <<'EOT'
d \\
g
EOT
)
echo \"\$x\""

# --- heredoc7.sub: heredoc opened inside a comsub that closes on the same line;
#     body collected from the lines after the enclosing command line.
check "heredoc in comsub, body after close" 'echo $(cat <<EOF)
foo
bar
EOF
after'

check "assign comsub heredoc body after" 'x=$(cat <<EOF)
one
two
EOF
echo "[$x]"'

# heredoc body collected WITHIN the comsub (regression guard: must not change)
check "heredoc body within comsub" 'x=$(cat <<EOF
inner1
inner2
EOF
)
echo "[$x]"'

# --- ) / ( terminate an unquoted heredoc delimiter word
check "paren-terminated delim subshell" '(cat <<EOF)
hi
EOF'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
