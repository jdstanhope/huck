#!/usr/bin/env bash
# Byte-identical bash<->huck harness: a BARE reference to an associative array
# (`${m}`, `${m:off:len}`, `${#m}`, `${m/p/r}`, `${m^^}`, `${m:-def}`, `${m#p}`,
# `${m@A}`, …) means `${m[0]}` — the element whose KEY is the string "0" — exactly
# like `${a}`≡`${a[0]}` for an indexed array. huck used to return "" for ANY bare
# associative reference (scalar_view hardcoded empty), diverging from bash whenever
# a "0" key existed. Compares stdout+rc; control bytes shown via `cat -v`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  h=$("$HUCK_BIN" -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- bare associative reference resolves to the "0"-key element ---
check "bare \${m}"          'declare -A m; m[0]=hello; echo "[${m}]"'
check "substring"          'declare -A m; m[0]=uvwx; echo "[${m:0:2}]"'
check "substring off"      'declare -A m; m[0]=hello; echo "[${m:1:3}]"'
check "length \${#m}"       'declare -A m; m[0]=hello; echo "${#m}"'
check "pattern-sub"        'declare -A m; m[0]=hello; echo "${m/l/L}"'
check "case ^^"            'declare -A m; m[0]=hello; echo "${m^^}"'
check "case ,,"            'declare -A m; m[0]=HELLO; echo "${m,,}"'
check "prefix-strip #"     'declare -A m; m[0]=hello; echo "${m#h}"'
check "suffix-strip %"     'declare -A m; m[0]=hello; echo "${m%o}"'
check "default set :-"     'declare -A m; m[0]=hi; echo "${m:-def}"'
check "alt set :+"         'declare -A m; m[0]=hi; echo "${m:+alt}"'
check "@Q quote"           'declare -A m; m[0]=hi; echo "${m@Q}"'
check "@A reconstruct"     'declare -A m=([0]=x [a]=y); echo "${m@A}"'

# --- control bytes (the nquote1 case): m[0]=$'\1\1' etc. ---
check "control-byte 0-key" 'e=$'\''uv\001\001wx'\''; declare -A m=([0]=$e); echo "[${m:0:4}]"'
check "control-byte bare"  'e=$'\''uv\001\001'\''; declare -A m=([0]=$e); echo "[${m}]"'

# --- empty when there is no "0" key (must stay empty) ---
check "no 0-key bare"      'declare -A m; m[k]=v; echo "[${m}]"'
check "no 0-key substring" 'declare -A m; m[k]=hello; echo "[${m:0:2}]"'
check "no 0-key length"    'declare -A m; m[k]=hello; echo "${#m}"'
check "no 0-key default"   'declare -A m; m[k]=v; echo "${m:-def}"'
check "@A no 0-key"        'declare -A m=([p]=x); echo "${m@A}"'
check "empty assoc"        'declare -A m; echo "[${m}]${#m}"'

# --- indexed arrays and scalars must be UNCHANGED ---
check "indexed bare"       'a=(x y z); echo "[${a}] ${#a} [${a:0:1}]"'
check "indexed no [0]"     'a=([2]=q); echo "[${a}]"'
check "scalar"             's=hello; echo "[${s}] ${#s} [${s:1:3}]"'

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
