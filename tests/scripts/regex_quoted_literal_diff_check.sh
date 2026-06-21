#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v199 (L-23): a QUOTED span of an `=~`
# regex operand matches LITERALLY — quoting escapes the regex metacharacters in
# that span (bash 3.2+). An UNQUOTED span (incl. an unquoted `$var`) stays an
# active regex. We compare the exit status (`echo $?`) of each [[ =~ ]] test.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# --- fully-quoted operand: metachars literal ---
check "quoted dot no-match"  '[[ axb =~ "a.b" ]]; echo $?'   # . literal -> no match (1)
check "quoted dot match"     '[[ a.b =~ "a.b" ]]; echo $?'   # literal dot matches (0)
check "quoted plus literal"  '[[ "a+b" =~ "a+b" ]]; echo $?' # + literal -> match (0)
check "quoted plus no-match" '[[ aaab =~ "a+b" ]]; echo $?'  # + literal -> no match (1)
check "quoted star literal"  '[[ "a*b" =~ "a*b" ]]; echo $?'
check "quoted parens"        '[[ "(x)" =~ "(x)" ]]; echo $?'
check "quoted bracket"       '[[ "[x]" =~ "[x]" ]]; echo $?'
check "quoted anchors"       '[[ "^a$" =~ "^a$" ]]; echo $?'
# --- unquoted operand: metachars active (unchanged behavior) ---
check "unquoted dot active"  '[[ axb =~ a.b ]]; echo $?'     # . active -> match (0)
check "unquoted plus active" '[[ aaab =~ a+b ]]; echo $?'    # + active -> match (0)
# --- partial quoting: only the quoted span is literal ---
check "partial dot literal"  '[[ axb =~ a"."b ]]; echo $?'   # quoted . -> no match (1)
check "partial dot match"    '[[ a.b =~ a"."b ]]; echo $?'   # quoted . -> match (0)
check "active anchor quoted" '[[ a.b =~ ^"a.b"$ ]]; echo $?' # ^/$ active, a.b literal -> match
check "active star q-mid"    '[[ aXXb =~ a".*"b ]]; echo $?' # quoted .* literal -> no match (1)
# --- var holding a regex: quoted = literal, unquoted = active (bash 3.2+) ---
check "var unquoted active"  're="a.b"; [[ axb =~ $re ]]; echo $?'   # active -> match (0)
check "var quoted literal"   're="a.b"; [[ axb =~ "$re" ]]; echo $?' # literal -> no match (1)
check "var quoted match"     're="a.b"; [[ a.b =~ "$re" ]]; echo $?' # literal -> match (0)
# --- no-meta sanity ---
check "plain literal match"  '[[ hello =~ "hello" ]]; echo $?'
check "empty-ish quoted"     '[[ a =~ a"" ]]; echo $?'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
