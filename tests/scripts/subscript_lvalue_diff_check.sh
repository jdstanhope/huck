#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v268 T3: subscript_lvalue (D1/D2 fixes).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# checkf: run a script via bash and huck, compare byte-for-byte (uses temp file)
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-subscript.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# checkd: run a script in a temp directory (for cases where file existence matters, like D1 glob)
checkd() {
    local label="$1" body="$2" tmpdir tmp b h
    tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/huck-subscript-dir.XXXXXX")
    tmp="$tmpdir/script.sh"
    printf '%s\n' "$body" > "$tmp"
    b=$(cd "$tmpdir" && bash script.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$tmpdir" && "$HUCK_BIN" script.sh 2>&1; echo "EXIT:$?")
    rm -rf "$tmpdir"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Assignment: basic indexed-array behavior (unchanged)
checkf "a[0]=v; echo" 'a[0]=v; echo "${a[0]}"'
checkf "a[subst]=w; echo" 'a[$(echo 2)]=w; echo "${a[2]}"'
checkf "a[1]=x; a[1]+=y" 'a[1]=x; a[1]+=y; echo "${a[1]}"'

# Associative array
checkf "declare -A m; m[k]=v" 'declare -A m; m[k]=v; echo "${m[k]}"'

# D1: glob expansion in subscripts (no file needed for non-matching globs)
checkf "D1: variable in subscript, no glob file" 'x=2; echo a[$x]'
checkf "D1: command subst in subscript" 'echo a[$(echo 2)]'
checkf "D1: literal glob pattern (no match)" 'echo a[bc]'

# D1: with file existence (requires temp dir for glob match)
checkd "D1: glob with file match" $'touch a2; x=2; echo a[$x]'

# D2: value-tilde expansion in subscript assignments
checkf "D2: tilde in assigned value" 'HOME=/h; a[0]=~/y; echo "${a[0]}"'
checkf "D2: tilde in colon-delimited value" 'HOME=/h; a[0]=p:~/y; echo "${a[0]}"'

# Glob mixed shapes
checkf "glob: a[b] c (separate)" 'echo a[b] c'
checkf "glob: a[b]c (adjacent)" 'echo a[b]c'
checkf "glob: quoted a[b]=c" 'echo "a[b]=c"'

# Task-1 Critical fix: unclosed IDENT[= should NOT create a subscript
checkf "T1: unclosed a[=x" 'echo a[=x'
checkf "T1: unclosed x[+=y" 'echo x[+=y'
checkf "T1: unclosed via printf" $'printf \'%s\\n\' a[=1'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
