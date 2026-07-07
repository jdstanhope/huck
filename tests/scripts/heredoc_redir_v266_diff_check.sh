#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v266: heredoc + redirection edge cases.
# FILE MODE (heredocs are multi-line): each fragment is written to a temp file and
# run as `bash "$tmp"` vs `"$HUCK_BIN" "$tmp"`, comparing stdout+stderr+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-hdr.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- Heredocs -------------------------------------------------------------
# H1: expanding (unquoted) body runs $(...) substitution.
checkf "H1 expanding \$() body"        $'cat <<EOF\n$(echo hi)\nEOF'
# H2: any quoting/escaping of the delimiter makes the WHOLE body literal.
checkf "H2a quoted delim '\''EOF'\''"       $'cat <<\'EOF\'\n$HOME\nEOF'
checkf "H2b escaped delim \\\\EOF"       $'cat <<\\EOF\n$HOME\nEOF'
checkf "H2c partial-quoted E\"O\"F"      $'cat <<E"O"F\n$HOME\nEOF'
# H3: \$ in an expanding body is a literal dollar (no expansion).
checkf "H3 backslash-dollar literal"   $'cat <<EOF\n\\$HOME\nEOF'
# H4: an unknown backslash escape passes through verbatim in an expanding body.
checkf "H4 backslash passthrough"      $'cat <<EOF\n\\d\nEOF'
# H5: two heredocs on one command line are consumed in order; cat's stdin is the
# LAST fd (<<B), so only bbb prints — but both bodies must be scanned in order.
checkf "H5 two heredocs one line"      $'cat <<A <<B\naaa\nA\nbbb\nB'
# H6: <<- strips leading TABS from body lines and from the terminator line.
checkf "H6 <<- tab dedent"             $'cat <<-EOF\n\tindented\n\tEOF'
# H7: the terminator must match EXACTLY — a line with trailing/leading spaces
# (no <<- dash) is body, not the close delimiter.
checkf "H7a trailing-space not delim"  $'cat <<EOF\nEOF \nreal\nEOF'
checkf "H7b leading-space not delim"   $'cat <<EOF\n  EOF\nreal\nEOF'
# H8: backslash-newline joins lines in an expanding heredoc, but is kept literal
# when the delimiter is quoted.
checkf "H8a line-continuation joins"   $'cat <<EOF\nab\\\ncd\nEOF'
checkf "H8b quoted keeps backslash-nl" $'cat <<\'EOF\'\nab\\\ncd\nEOF'

# --- Redirections ---------------------------------------------------------
# R1: &> combines stdout+stderr into the target.
checkf "R1 &> combine"                 $'f=${TMPDIR:-/tmp}/huckR1_$$\n{ echo out; echo err >&2; } &> "$f"\ncat "$f"\nrm -f "$f"'
# R2: &>> combines and APPENDS.
checkf "R2 &>> append-combine"         $'f=${TMPDIR:-/tmp}/huckR2_$$\necho first > "$f"\n{ echo out; echo err >&2; } &>> "$f"\ncat "$f"\nrm -f "$f"'
# R3: the redirect target may itself be a command substitution.
checkf "R3 target is \$()"             $'echo hi > "$(echo ${TMPDIR:-/tmp}/huckR3_$$)"\ncat ${TMPDIR:-/tmp}/huckR3_$$\nrm -f ${TMPDIR:-/tmp}/huckR3_$$'
# R4: fd-prefix vs word distinction. A bare all-digits token immediately before
# `>` is an fd; if the token has non-digits the digit is part of the WORD.
#   echo 3>f   -> fd3 redirect, echo prints empty line, file empty
checkf "R4a bare 3> is fd"             $'f=${TMPDIR:-/tmp}/huckR4a_$$\necho 3>"$f"\necho "content:[$(cat "$f")]"\nrm -f "$f"'
#   echo 3 >f  -> "3" is an arg, stdout redirected -> file has "3"
checkf "R4b 3 space > is arg"          $'f=${TMPDIR:-/tmp}/huckR4b_$$\necho 3 >"$f"\necho "content:[$(cat "$f")]"\nrm -f "$f"'
#   echo x2>f  -> "x2" is a word (has non-digit), stdout redirected -> file "x2"
checkf "R4c x2> digit is word"         $'f=${TMPDIR:-/tmp}/huckR4c_$$\necho x2>"$f"\necho "content:[$(cat "$f")]"\nrm -f "$f"'
#   ls MISSING 2>f -> "2" IS the stderr fd; error text lands in file
checkf "R4d bare 2> is stderr fd"      $'f=${TMPDIR:-/tmp}/huckR4d_$$\nls /no/such/huck_$$ 2>"$f"\necho "err-lines:[$(wc -l < "$f" | tr -d " ")]"\nrm -f "$f"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
