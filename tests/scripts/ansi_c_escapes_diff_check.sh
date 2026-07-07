#!/usr/bin/env bash
# Byte-identical bash<->huck harness for $'…' ANSI-C quoting escape sequences.
# Complements dollar_quote_forms_diff_check.sh (which covers the $'…'/$"…" FORMS
# and \t/\n) by exercising the fuller escape alphabet: octal, greedy octal stop,
# \c control chars, \U 8-hex unicode, unknown escapes, escaped quote, empty.
#
# File mode (checkf: write fragment to a temp file, run `bash "$tmp"` vs
# `"$HUCK_BIN" "$tmp"`) is used deliberately so the fragment's backslashes reach
# each shell verbatim — no harness-level double-escaping. Output is captured
# through `od -An -c` so control/NUL-free byte sequences compare exactly.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ansic.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1 | od -An -c; echo "EXIT:${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" "$tmp" 2>&1 | od -An -c; echo "EXIT:${PIPESTATUS[0]}")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Octal: \NNN (1-3 octal digits) → byte.
checkf "octal HI"          "printf '%s\\n' \$'\\110\\151'"
# Greedy octal stop: \1 consumes at most one more octal digit; '8' is not octal,
# so this is byte 0x01 followed by the literal '8'.
checkf "octal greedy stop" "echo \$'\\18'"

# \cX control chars: \cA → 0x01, \cZ → 0x1A.
checkf "control chars"     "echo \$'\\cA\\cZ'"

# \U with 8 hex digits → UTF-8 encoded code point (😀 = U+1F600).
checkf "8-hex unicode"     "echo \$'\\U0001F600'"

# Unknown escape: backslash is preserved verbatim (\q → \q).
checkf "unknown escape"    "echo \$'\\q'"

# Escaped backslash then escaped single quote: \\ → \ , \' → ' .
checkf "escaped quote"     "echo \$'\\\\\\''"

# Empty ANSI-C string prints nothing but the echo newline.
checkf "empty"             "echo \$''"

# --- reinforcing coverage (fuller escape alphabet, still byte-identical) ---
# \e escape, plus \a \b \f \v \r single-char controls.
checkf "escape + controls" "echo \$'\\e[\\a\\b\\f\\v\\r'"
# Mixed \xHH and \NNN in one string → 'A' (0x41) then 'B' (octal 102).
checkf "mixed hex+octal"   "echo \$'\\x41\\102'"
# One reinforcing concatenation case: literal text abutting a $'…' escape.
checkf "concat pre/post"   "echo pre\$'\\tX'post"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
