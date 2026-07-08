#!/usr/bin/env bash
# Byte-identical bash<->huck harness: heredocs STARTED inside a command
# substitution `$( … )` or backtick `` `…` `` whose close delimiter is adjacent
# to (or shares a line with) the heredoc close-delimiter text.
#
# bash's command-substitution here-document scanner uses a PREFIX delimiter
# compare (unlike the top-level exact-match scanner): a body line that STARTS
# WITH the delimiter terminates the heredoc, leaving the rest of the line for the
# enclosing `)` / `` ` `` to close. This exercises the corpus files
# comsub-eof0/1/4 and the former backtick PANIC (comsub-eof1). timeout-guarded.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# NB: stdout + exit-code parity only. bash emits "here-document ... delimited by
# end-of-file" / "unterminated here-document" WARNINGS to stderr for the eof-
# adjacency cases; huck intentionally does not (a known, kept divergence), so
# stderr is deliberately not compared here.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>/dev/null; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# comsub-eof1: heredoc inside a BACKTICK (the former `unreachable!` panic).
check "backtick heredoc"          $'foo=`cat <<EOF\nhi\nEOF`\necho $foo'
# comsub-eof0: `$()` with `EOF )` (delimiter, space, then `)`).
check "dollarparen EOF space )"   $'foo=$(cat <<EOF\nhi\nEOF )\necho $foo'
# comsub-eof4: `$()` with `EOF)` (no space before the `)`).
check "dollarparen EOF)"          $'e=$(cat <<EOF\ncontents\nEOF)\necho $e'
# Multi-line body inside `$()`.
check "multiline body"            $'x=$(cat <<EOF\nline1\nline2\nline3\nEOF\n)\necho "$x"'
# Expansion parity: a `$var` inside an expanding heredoc body substitutes.
check "expanding body var"        $'name=world\ng=$(cat <<EOF\nhello $name\nEOF\n)\necho "$g"'
# Expansion parity inside a backtick body.
check "backtick body var"         $'name=bt\ng=`cat <<EOF\nhi $name\nEOF`\necho "$g"'
# Literal (`<<\HD`) heredoc inside `$()` — no expansion, adjacent `)`. `\H` is not
# a `$'...'` escape, so `<<\\HD` yields the literal `<<\HD` delimiter word.
check "literal heredoc HD)"       $'x=$(cat <<\\HD\nno $expand here\nHD)\necho "$x"'
# Proper delimiter line then a separate `)` line (exact match still works).
check "proper delim then )"       $'x=$(cat <<EOF\nbody\nEOF\n)\necho "$x"'
# Backtick with a proper delimiter line then trailing `` ` `` on next line.
check "backtick proper delim"     $'x=`cat <<EOF\nbody\nEOF\n`\necho "$x"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
