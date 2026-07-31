#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `read` last-field IFS-splitting root
# fixed in v350 (#2, `ifs-posix` category): `read name1 name2 …` with a
# non-whitespace (or mixed-class) IFS assigns the LAST variable per bash's
# read.def rule — extract one more word; if it exhausts the line the trailing
# delimiter is DROPPED, otherwise the raw remainder is kept with only trailing
# IFS-whitespace stripped (interior / multiple trailing non-ws delimiters kept).
# Each fragment feeds INPUT on stdin to `bash -c` and `huck -c`; stdout+stderr+
# exit must match byte-for-byte.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# $1 label, $2 input (fed on stdin), $3 shell fragment
check() {
    local label="$1" inp="$2" frag="$3" b h
    b=$(printf '%s\n' "$inp" | bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$inp" | "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

XY='read x y; echo "[$x][$y]"'
XYZ='read x y z; echo "[$x][$y][$z]"'

# --- Root: last-field trailing delimiter (IFS=":") ---
check "sole trailing dropped"   "a:b:"    "IFS=: $XY"    # [a][b]
check "double trailing kept"    "a:b::"   "IFS=: $XY"    # [a][b::]
check "kept after interior"     "a:b:c:"  "IFS=: $XY"    # [a][b:c:]
check "empty last delim dropped" "a::"    "IFS=: $XY"    # [a][]
check "double empty kept"       "a:::"    "IFS=: $XY"    # [a][::]
check "leading+double empty"    ":::"     "IFS=: $XY"    # [][::]
check "3vars trailing"          ":a:b:"   "IFS=: $XYZ"   # [][a][b]
check "3vars middle empty"      "a:b::"   "IFS=: $XYZ"   # [a][b][]
check "3vars two empty"         "a:::"    "IFS=: $XYZ"   # [a][][]
check "leading-ws-in-last(:)"   "a: :b"   "IFS=: $XY"    # [a][ :b]

# --- Mixed-class IFS=": " (whitespace + non-whitespace) ---
check "mixed sole trailing"     "a:b: "   'IFS=": " '"$XY"   # [a][b]
check "mixed collapse trailing" "a:  :b"  'IFS=": " '"$XY"   # [a][:b]
check "mixed empty"             "::"      'IFS=": " '"$XY"   # [][]
check "mixed leading colon"     ":a:"     'IFS=": " '"$XY"   # [][a]
check "mixed spaced delims"     "a : b : " 'IFS=": " '"$XY"  # [a][b]

# --- Whitespace-first IFS keeps interior ws run in the last field ---
check "ws-first interior kept"  "a  b  c" 'IFS=" :" '"$XY"   # [a][b  c]

# --- KEEP guards (must be unchanged) ---
check "default ws read trims"   "  a  b  " "$XY"            # default IFS: [a][b]
check "single name whole line"  "a:b:"    'IFS=: read x; echo "[$x]"'  # [a:b:]
check "read -a unbounded"       "a:b:"    'IFS=: read -a arr; echo "${#arr[@]}:${arr[0]}:${arr[1]}"'
check "more vars than fields"   "a"       "IFS=: $XY"       # [a][]

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
