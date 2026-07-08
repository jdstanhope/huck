#!/usr/bin/env bash
# Byte-identical bash<->huck harness: an UNTERMINATED here-document at end-of-input
# is delimited by EOF (bash warns on stderr but PARSES + RUNS the body collected so
# far; rc 0). huck matches — closing the heredoc at EOF for a top-level BATCH parse
# (whole file / -c string / piped-stdin program), while the interactive REPL keeps
# prompting (that path is covered by unit tests on `classify`).
#
# bash prints a `warning: here-document … delimited by end-of-file` line to STDERR
# that huck need not emit, so this harness compares STDOUT + EXIT ONLY (like the
# other heredoc harnesses' spirit), never stderr.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# checkf LABEL BODY [trailing_nl]
# Runs BODY as a script FILE through bash and huck, comparing stdout+exit only.
# trailing_nl=1 appends a final newline to the file; default (0) writes BODY as-is
# (the file ends mid-heredoc with no trailing newline).
checkf() {
    local label="$1" body="$2" trailing="${3:-0}" tmp b br h hr
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-hdeof.XXXXXX")
    if [[ "$trailing" == 1 ]]; then printf '%s\n' "$body" > "$tmp"; else printf '%s' "$body" > "$tmp"; fi
    b=$(bash "$tmp" 2>/dev/null); br=$?
    h=$("$HUCK_BIN" "$tmp" 2>/dev/null); hr=$?
    rm -f "$tmp"
    if [[ "$b" == "$h" && "$br" == "$hr" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$br" "$hr"
        diff <(printf '%s' "$b") <(printf '%s' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# checkc LABEL BODY : same via `-c STRING` instead of a file.
checkc() {
    local label="$1" body="$2" b br h hr
    b=$(bash -c "$body" 2>/dev/null); br=$?
    h=$("$HUCK_BIN" -c "$body" 2>/dev/null); hr=$?
    if [[ "$b" == "$h" && "$br" == "$hr" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$br" "$hr"
        diff <(printf '%s' "$b") <(printf '%s' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# checkp LABEL BODY : same via piped stdin (`printf … | huck`).
checkp() {
    local label="$1" body="$2" b br h hr
    b=$(printf '%s' "$body" | bash 2>/dev/null); br=$?
    h=$(printf '%s' "$body" | "$HUCK_BIN" 2>/dev/null); hr=$?
    if [[ "$b" == "$h" && "$br" == "$hr" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$br" "$hr"
        diff <(printf '%s' "$b") <(printf '%s' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# --- FILE mode: open heredoc at EOF -------------------------------------------
# E1: single-line body, no trailing newline (file ends right after the body char).
checkf "E1 single-line no-nl"        $'cat <<EOF\nhi'
# E2: single-line body, trailing newline (still no close-delimiter line).
checkf "E2 single-line trailing-nl"  $'cat <<EOF\nhi' 1
# E3: multi-line body.
checkf "E3 multi-line"               $'cat <<EOF\nline one\nline two'
# E4: expanding body — $() / var expansion runs.
checkf "E4 expanding body"           $'x=world\ncat <<EOF\nhi $x and $(echo sub)'
# E5: <<- strips leading tabs, closed by EOF.
checkf "E5 <<- tab-strip"            $'cat <<-EOF\n\tindented\n\talso'
# E6: quoted delimiter -> literal body (no expansion), closed by EOF.
checkf "E6 quoted delim literal"     $'cat <<\'EOF\'\n$HOME literal'
# E7: empty body (bare newline then EOF).
checkf "E7 empty body"               $'cat <<EOF\n' 1
# E8: a preceding command runs, then the trailing heredoc is EOF-closed.
checkf "E8 preceding command"        $'echo before\ncat <<EOF\nhi'

# --- CONTROL: a properly-closed heredoc is unchanged --------------------------
checkf "C1 closed control"           $'cat <<EOF\nhi\nEOF\necho after' 1
checkf "C2 closed <<- control"       $'cat <<-EOF\n\thi\n\tEOF\necho after' 1

# --- -c STRING mode -----------------------------------------------------------
checkc "S1 -c single-line"           $'cat <<EOF\nhi'
checkc "S2 -c multi-line expanding"  $'cat <<EOF\nhi $HOME'
checkc "S3 -c closed control"        $'cat <<EOF\nhi\nEOF\necho ok'

# --- piped-stdin mode ---------------------------------------------------------
checkp "P1 piped single-line"        $'cat <<EOF\nhi\n'
checkp "P2 piped no-trailing-nl"     $'cat <<EOF\nhi'
checkp "P3 piped preceding command"  $'echo before\ncat <<EOF\nhi\n'
checkp "P4 piped closed control"     $'cat <<EOF\nhi\nEOF\necho ok\n'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
