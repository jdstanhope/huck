#!/usr/bin/env bash
# Byte-identical bash<->huck harness: `<<-` heredocs strip leading TABS from the
# body/close lines AND from the delimiter WORD, so a quoted tab-indented delimiter
# (`<<-$'\tEND'` / `<<-'<TAB>END'`) matches a tab-stripped close line. huck used to
# strip only the close line, so the stored delim kept its tab and never matched —
# the body ran to EOF. Compares stdout + exit only.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
T=$'\t'   # a real tab

# checkf LABEL BODY : run BODY (may contain real tabs) as a script file in both shells.
checkf() {
    local label="$1" body="$2" tmp b br h hr
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-hddash.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
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

# The regression case: a quoted tab-indented `<<-` delimiter, tab-indented body+close.
checkf "tab-quoted delim '<TAB>END', tabbed body+close" \
    "cat <<-'${T}END'
${T}hello
${T}END
echo AFTER"

# Quoted delimiter with a tab, but the CLOSE line has no tab (still matches: both stripped).
checkf "tab-quoted delim, close line not tab-indented" \
    "cat <<-'${T}END'
body
END
echo AFTER"

# Ordinary `<<-` (unquoted) with multiple leading tabs — unchanged behavior.
checkf "<<-END, double-tab body+close" \
    "cat <<-END
${T}${T}hi
${T}${T}END
echo AFTER"

# Quoted delimiter WITHOUT a tab, tab-indented close — close-line strip still applies.
checkf "<<-'END' (no tab), tab-indented close" \
    "cat <<-'END'
${T}hi
${T}END
echo AFTER"

# Plain `<<` (no dash): tabs are NOT stripped — a tab-indented close must NOT match.
checkf "plain << does not strip tabs (tab-close is body, EOF-delimited)" \
    "cat <<END
hi
${T}END
echo AFTER"

# `<<-` where a tab-indented delimiter is a strict prefix of a longer word: no match.
checkf "<<-END, close 'ENDING' is not the delimiter" \
    "cat <<-END
${T}line
${T}ENDING
${T}END
echo AFTER"

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
