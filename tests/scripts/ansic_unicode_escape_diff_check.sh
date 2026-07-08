#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the $'\u'/$'\U' Unicode-escape encoder.
#
# Focus: bash accepts out-of-range \u/\U codepoints (surrogates, > U+10FFFF) at
# PARSE time and emits bytes using the classic *extended* UTF-8 scheme (1-6
# bytes for values <= 0x7FFFFFFF; nothing above that). huck previously raised a
# syntax error (LexError::AnsiCInvalidCodepoint). This harness proves:
#
#   (A) representable values are byte-identical to bash — every valid Unicode
#       scalar (<= U+10FFFF, non-surrogate) AND the > 0x7FFFFFFF range (empty).
#   (B) the extreme surrogate / 0x110000..=0x7FFFFFFF values no longer PARSE-ERROR
#       (both shells exit 0). huck stores words as valid-UTF-8 Rust `String`s (the
#       same architectural reason $'\xff' yields the two bytes c3 bf rather than
#       the raw byte ff), so it cannot reproduce bash's *invalid*-UTF-8 bytes for
#       these values and substitutes U+FFFD — a bounded, documented residual
#       divergence. Parity here is on EXIT STATUS (the closed parse gap), not bytes.
#
# Bytes are captured through `od -An -tx1` so sequences compare exactly.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# (A) byte-identical stdout for representable values.
checkbytes() {  # label ; escape (e.g. '\U0041')
    local label="$1" esc="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-uni.XXXXXX")
    printf "printf '%%s' \$'%s'\n" "$esc" > "$tmp"
    b=$(bash "$tmp" 2>/dev/null | od -An -tx1; echo "EXIT:${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" "$tmp" 2>/dev/null | od -An -tx1; echo "EXIT:${PIPESTATUS[0]}")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# (B) parse-gap parity: both shells must exit 0 (no syntax error). Bytes are
# allowed to differ (documented residual for invalid-UTF-8 codepoints).
checkparse() {  # label ; escape
    local label="$1" esc="$2" tmp brc hrc
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-uni.XXXXXX")
    printf "printf '%%s' \$'%s'\n" "$esc" > "$tmp"
    bash "$tmp" >/dev/null 2>&1; brc=$?
    "$HUCK_BIN" "$tmp" >/dev/null 2>&1; hrc=$?
    rm -f "$tmp"
    if [[ "$brc" == 0 && "$hrc" == 0 ]]; then printf 'PASS: %s (parse-gap closed, both rc 0)\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$brc" "$hrc"; FAIL=$((FAIL+1)); fi
}

# --- (A) valid Unicode scalars: byte-identical to bash across all widths ---
checkbytes "U ascii 1-byte"      '\U00000041'
checkbytes "U 2-byte (é)"        '\U000000e9'
checkbytes "U 3-byte"            '\U00000800'
checkbytes "U emoji 4-byte"      '\U0001F600'
checkbytes "U max scalar"        '\U0010ffff'
checkbytes "u 4-hex ascii"       'A'
checkbytes "u 4-hex 2-byte"      'é'
checkbytes "u 4-hex 3-byte max"  '￿'
# valid scalar abutting literal text stays exact
checkbytes "U + literal suffix"  '\U000000e9XY'

# --- (A) > 0x7FFFFFFF: bash emits NOTHING; huck matches exactly (empty) ---
checkbytes "U fffffffe -> empty" '\Ufffffffe'
checkbytes "U ffffffff -> empty" '\Uffffffff'
checkbytes "U 80000000 -> empty" '\U80000000'

# --- (B) extreme invalid-UTF-8 codepoints: parse gap closed (rc 0), residual bytes ---
checkparse "U just-over-max"     '\U00110000'
checkparse "U surrogate d800"    '\Ud800'
checkparse "U surrogate dfff"    '\Udfff'
checkparse "u surrogate d800"    '\ud800'
checkparse "U 5-byte range"      '\U00200000'
checkparse "U 6-byte max"        '\U7fffffff'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
