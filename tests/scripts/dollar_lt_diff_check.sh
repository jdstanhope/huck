#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `$(< file)` command substitution.
# bash special-cases a command substitution whose body is JUST a stdin
# read-only redirect: it reads the file's contents directly as the
# substitution output (like `$(cat file)`). Outside a capture context,
# `< file` alone produces no output. Each fragment runs through `bash -c`
# and `huck -c`; stdout+exit must match. (The open-error DIAGNOSTIC wording
# diverges, so the error case suppresses stderr inside the substitution.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "reads multiline file"     'f=$(mktemp); printf "l1\nl2\nl3\n" > "$f"; x=$(< "$f"); echo "len=${#x} [$x]"; rm -f "$f"'
check "no trailing newline"      'f=$(mktemp); printf "no-newline" > "$f"; echo "[$(< "$f")]"; rm -f "$f"'
check "missing file -> empty"    'echo "[$(< /nonexistent_xyz 2>/dev/null)]"'
check "outside capture no out"   '< /etc/hostname; echo "after"'
check "in var assignment"        'f=$(mktemp); printf "abc\n" > "$f"; v=$(< "$f"); echo "v=$v"; rm -f "$f"'
check "empty file -> empty"      'f=$(mktemp); : > "$f"; x=$(< "$f"); echo "len=${#x}"; rm -f "$f"'
check "expanded path target"     'd=$(mktemp -d); printf "P\n" > "$d/g"; n=g; echo "[$(< "$d/$n")]"; rm -rf "$d"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
