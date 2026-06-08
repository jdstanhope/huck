#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v110: the last 2 mise-activation leaks.
#   * M-90 combined `>file 2>&1` — a builtin's error follows a redirected stdout
#     file (mise line 29 `declare -p chpwd_functions >/dev/null 2>&1`); the v109
#     fix only covered `2>file`/`2>>file` and a bare `2>&1`, leaving the
#     combined case leaking. Now `prepare_builtin_stderr` dups the redirected
#     stdout file's fd onto fd 2.
#   * M-105 — an unquoted `${x+alt}` that expands to nothing emits NO field
#     (was a spurious empty arg, which broke `mise hook-env` with an empty `''`).
#     A QUOTED empty still emits one field; the v109 set-array idiom is
#     unchanged.
# Each fragment's combined stdout+stderr+exit is compared verbatim.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- M-90 combined `>file 2>&1` ---

# 1: combined >/dev/null 2>&1 suppresses the builtin error (both print only ok)
check "combined >/dev/null 2>&1 suppress" 'declare -p NOPE >/dev/null 2>&1; echo ok'

# 2: bare 2>&1 routes the builtin error into the pipe (grep -c counts the line)
check "bare 2>&1 into pipe" '{ declare -p NOPE 2>&1; } | grep -c NOPE'

# 3: the v109 file path (2>/dev/null alone) still suppresses
check "file 2>/dev/null still suppress" 'declare -p NOPE 2>/dev/null; echo ok'

# 4: combined redirect still preserves rc 1 (the || branch is taken)
check "combined rc preserved" 'declare -p NOPE >/dev/null 2>&1 && echo yes || echo no'

# --- M-105 unquoted ${x+alt} spurious empty field ---

# 5: unquoted +alt on an unset var contributes no field ($# == 2, not 3)
check "unquoted +alt no spurious field" 'set -- ${u+X} a b; echo $#'

# 6: the mise idiom shape — empty array + ${arr[@]+"${arr[@]}"} -> nothing
check "empty-array +idiom no spurious" 'f=(); set -- ${f[@]+"${f[@]}"} -s bash; echo $#'

# 7: a QUOTED empty is still one field ($# == 2)
check "quoted empty still one field" 'set -- "${u+x}" a; echo $#'

# 8: a QUOTED empty field is actually present (printf prints <>)
check "quoted empty printf field" 'printf "<%s>" "${u+x}"; echo'

# 9: the v109 set-array idiom is unchanged (yields the elements)
check "set array idiom unchanged" 'a=(1 2); printf "<%s>" "${a[@]+"${a[@]}"}"; echo'

# 10: unquoted -default that expands to empty still vanishes (Value path, $# == 2)
check "unquoted -default still vanishes" 'set -- ${u-} a b; echo $#'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
