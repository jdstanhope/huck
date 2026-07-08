#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the array-literal append element
# `[subscript]+=value` (both indexed and associative). huck previously rejected
# the `+=` spelling inside a compound array RHS with
#   syntax error: array element subscript requires '=' after ']'
# (LexError::ArrayLiteralMissingEquals) while accepting the plain `[i]=v` form.
#
# Runtime semantics exercised (all matched to bash 5.2):
#   * fresh `a=(…)` replace: `[i]+=v` appends to the value set by an EARLIER
#     element of the SAME literal (base empty if none) — the old array is discarded.
#   * `a+=(…)` append: `[i]+=v` appends to the PRE-EXISTING element i.
#   * integer-flagged arrays: `+=` is arithmetic addition, not concat.
#   * associative: `a+=([k]+=v)` concatenates onto the existing key; a fresh
#     `declare -A a=([k]=x [k]+=y)` replace treats `+=` as a plain set (bash quirk).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aappend.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- indexed: fresh replace ---
checkf "replace unset elem +=" 'x=(1 2 [2]+=7 4); echo "${x[@]}"'
checkf "replace concat in literal" 'x=([0]=a [0]+=b [0]+=c); echo "${x[0]}"'
checkf "replace discards old array" 'x=(9 9 9); x=([0]+=B); echo "${x[0]}"'
checkf "replace lone += unset" 'x=([2]+=7); echo "${x[2]}"'

# --- indexed: append context (a+=(…)) ---
checkf "append onto existing elem" 'x=(a b c); x+=([1]+=Z); echo "${x[@]}"'
checkf "append beyond existing" 'x=(a b); x+=([5]+=Z); echo "${x[5]}|${x[@]}"'

# --- integer arrays: arithmetic += ---
checkf "integer array append" 'declare -ia n=(5 [0]+=3); echo "${n[@]}"'
checkf "integer array append unset" 'declare -ia n=([2]+=3); echo "${n[2]}"'

# --- associative ---
checkf "assoc append existing" 'declare -A a=([one]=one); a+=([one]+=more); echo "${a[one]}"'
checkf "assoc replace += is set" 'declare -A a=([k]=x [k]+=y); echo "[${a[k]}]"'
checkf "assoc append onto base" 'declare -A a=([k]=base); a+=([k]+=y); echo "[${a[k]}]"'
checkf "assoc append fresh key" 'declare -A a=([one]=1); a+=([two]+=2); echo "${a[two]}"'

# --- mixed & expansion in subscript/value ---
checkf "arith subscript append" 'i=1; x=([i+1]+=v); echo "${x[2]}"'
checkf "expansion in += value" 'v=X; x=([0]=a [0]+=$v); echo "${x[0]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
