#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the alias-expansion roots fixed in
# v345 (#329): R1 array literal as the leading command word of an alias body,
# R2 alias expansion for a command word after leading redirections (order-
# sensitive vs assignment prefixes), R3 an alias expanding to a leading `#`
# starts a comment. Each fragment runs through `bash -c` and `huck -c`;
# stdout+stderr+exit must match byte-for-byte.
#
# NOTE: bash has a "same-line alias-timing" rule — an alias defined AND used on
# the same logical line does NOT expand. So every fragment defines its alias on
# its own line (embedded newlines) BEFORE the line that uses it.
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

# ── R1: array literal as the leading command word of an alias body ──
check "R1 leading array in alias" $'shopt -s expand_aliases\nalias foo="a=(1 2 3); echo ${a[@]}"\nfoo'
check "R1 leading append array"   $'shopt -s expand_aliases\na=(1)\nalias foo="a+=(2 3); echo ${a[@]}"\nfoo'
check "R1 non-leading array regr" $'shopt -s expand_aliases\nalias foo="echo x; a=(1 2); echo ${a[@]}"\nfoo'
check "R1 eval array regr"        $'eval "a=(1 2 3); echo ${a[@]}"'
check "R1 scalar in alias regr"   $'shopt -s expand_aliases\nalias foo="x=5; echo $x"\nfoo'

# ── R2: alias for the command word after leading redirection(s) ──
check "R2 alias after < redir"    $'shopt -s expand_aliases\nalias foo=echo\n< /dev/null foo bar'
check "R2 alias in eval w/ redir" $'shopt -s expand_aliases\nalias e=echo\neval "</dev/null e ok 3"'
check "R2 assign-prefix + alias"  $'shopt -s expand_aliases\nalias e=echo\na=true e ok 4'
# order-sensitivity: redirect BEFORE assignment → expands; assignment then redirect → not
check "R2 redir-before-assign"    $'shopt -s expand_aliases\nalias foo=echo\n< /dev/null a=1 foo bar'
# suppress case: foo is NOT expanded → command-not-found. Assert the RC only,
# not the error text (whose program-name prefix differs by how the shell is
# invoked — a non-behavioral $0 artifact when piping via stdin).
check "R2 assign-then-redir supp" $'shopt -s expand_aliases\nalias foo=echo\na=1 < /dev/null foo bar 2>/dev/null\necho "rc=$?"'
check "R2 no-leading regr"        $'shopt -s expand_aliases\nalias foo=echo\nfoo x'

# ── R3: alias expanding to a leading `#` starts a comment ──
check "R3 alias is hash"          $'shopt -s expand_aliases\nalias comment=#\ncomment\necho done'
check "R3 alias hash w/ trailing" $'shopt -s expand_aliases\nalias lc="# for x in "\nlc text after\necho k'
check "R3 literal hash regr"      $'echo a#b'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
