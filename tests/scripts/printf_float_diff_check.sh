#!/usr/bin/env bash
# Byte-identical bash<->huck harness for printf dynamic field width/precision
# (`%*`, `%.*`) and the floating-point conversions (`%f %F %e %E %g %G`).
# Each fragment runs through `bash -c` and `huck -c`; stdout+exit must match.
#
# NOT byte-compared: the "invalid number" DIAGNOSTIC wording diverges (huck
# backtick-quotes the arg), so invalid-arg cases suppress stderr and assert
# only stdout + exit status (which DO match bash).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# stdout + exit only (stderr suppressed so diagnostic wording doesn't matter).
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- dynamic width / precision (`*`) ---
check "star width"             "printf '%*d\n' 6 42"
check "star left-align flag"   "printf '%-*s|\n' 8 hi"
check "negative star width"    "printf '[%*d]\n' -6 42"
check "star precision float"   "printf '%.*f\n' 2 3.14159"
check "star precision string"  "printf '%.*s\n' 3 abcdef"
check "both star"              "printf '%*.*f\n' 10 3 3.14159"
check "negative star prec"     "printf '%.*f\n' -1 3.14159"

# --- float conversions ---
check "f default prec"         "printf '%f\n' 3.14"
check "f explicit prec"        "printf '%.2f\n' 3.14159"
check "f width.prec"           "printf '%5.2f\n' 3.14159"
check "f plus flag"            "printf '%+.1f\n' 3.0"
check "f zero pad"             "printf '%08.2f\n' 3.14"
check "f space flag"           "printf '% .2f\n' 3.0"
check "F uppercase"            "printf '%F\n' 1.5"
check "e exponent"             "printf '%e\n' 12345.678"
check "E uppercase"            "printf '%E\n' 12345.678"
check "g small"                "printf '%g\n' 0.0001"
check "g large"                "printf '%g\n' 1000000"
check "g strips zeros"         "printf '%g\n' 3.14000"
check "G uppercase"            "printf '%G\n' 1000000"
check "f integer arg"          "printf '%f\n' 42"
check "f negative"             "printf '%f\n' -2.5"
check "f exponent input"       "printf '%f\n' 1e3"

# --- mixed: existing %d/%s still work alongside floats ---
check "mixed d and f"          "printf '%d %f\n' 5 2.5"
check "mixed s and g"          "printf '%s=%g\n' x 0.5"
check "cycle over args"        "printf '%f\n' 1.0 2.0 3.0"

# --- invalid float arg: stdout+exit match (stderr wording diverges) ---
check "f invalid arg"          "printf '%f\n' abc"
check "f trailing garbage"     "printf '%f\n' 3.14xyz"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
