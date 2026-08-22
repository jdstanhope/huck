#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a NUMERIC LITERAL the base cannot use
# (#720).
#
# bash reads a numeric literal as ONE token spanning `[A-Za-z0-9#@_]*`, so a
# character the base cannot use does not end the number — it makes the whole run
# invalid, reported as `value too great for base` naming the ENTIRE run:
#
#     $((12abc))   12abc: value too great for base (error token is "12abc")
#
# huck stopped the number at the first such character and let the parser trip
# over the remainder separately, giving `syntax error in expression` with only
# the tail (`abc`) as the error token — and had its own wording, with an EMPTY
# error token, for a bad octal (`08`) and a digitless hex (`0xg`).
#
# The explicit-base form (`2#12`, `16#zz`) already agreed; it is the model the
# other three literal kinds now follow.
#
# COMPARED: the diagnostic with the `$0`/`line N:` prefix stripped, plus stdout
# and status.
#
# A literal that IS valid for its base but too large for 64 bits WRAPS, as C's
# `intmax_t` does — `$((99999999999999999999))` is 7766279631452241919 and
# `$((0xFFFFFFFFFFFFFFFF))` is -1 (#725). huck used to refuse those, except for
# the hex case, which briefly answered a silent 0.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

check() {
    local label="$1" expr="$2" b h frag
    frag="echo \$(( $expr ))"
    b=$( bash --norc --noprofile -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         bash --norc --noprofile -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         "$HUCK_BIN" -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    compare "$label" "$b" "$h"
}

# --- decimal with trailing run characters ---
check 'dec: 12abc'        '12abc'
check 'dec: 99abc'        '99abc'
check 'dec: 1a'           '1a'
check 'dec: 5x'           '5x'
check 'dec: 1e5'          '1e5'
check 'dec: 12_ '         '12_'
check 'dec: 12@'          '12@'
check 'dec: 0b'           '0b'

# --- octal: a digit outside the base is the same error ---
check 'oct: 08'           '08'
check 'oct: 09'           '09'
check 'oct: 0778'         '0778'

# --- hex ---
check 'hex: 0x1g'         '0x1g'
check 'hex: 0xg'          '0xg'
check 'hex: 0X1G'         '0X1G'
# `0x` with no digits is 0, and the whole prefix is consumed.
check 'hex: bare 0x'      '0x'
check 'hex: 0x plus 1'    '0x + 1'

# --- explicit base: the shape that already agreed ---
check 'base: 2#12'        '2#12'
check 'base: 16#zz'       '16#zz'
check 'base: 65#1'        '65#1'

# --- a SPACE ends the run, so the tail is a separate token ---
check 'space: 12 abc'     '12 abc'

# --- too large for 64 bits: WRAP, do not refuse (#725) ---
check 'wrap: decimal'     '99999999999999999999'
check 'wrap: exactly 2^64' '18446744073709551616'
check 'wrap: 2^63'        '9223372036854775808'
check 'wrap: i64 min'     '-9223372036854775808'
check 'wrap: hex all Fs'  '0xFFFFFFFFFFFFFFFF'
check 'wrap: hex 17 Fs'   '0xFFFFFFFFFFFFFFFFF'
check 'wrap: base 16'     '16#FFFFFFFFFFFFFFFF'
check 'wrap: base 2'      '2#1111111111111111111111111111111111111111111111111111111111111111'
check 'wrap: octal'       '0777777777777777777777777'
check 'wrap: in an expr'  '0xFFFFFFFFFFFFFFFF + 1'

# --- valid literals are unaffected ---
check 'ok: decimal'       '123'
check 'ok: octal'         '010'
check 'ok: hex'           '0x1f'
check 'ok: base 2'        '2#11'
check 'ok: base 16'       '16#ff'
check 'ok: expression'    '0x10 + 010 + 2#11'
check 'ok: zero'          '0'

harness_summary
