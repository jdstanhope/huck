#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v109: the three gaps that leaked
# errors when the user's ~/.bashrc sourced `mise activate bash` — clearing
# them makes ~/.bashrc source with ZERO errors:
#   * M-90 — a builtin's error output now honors `2>` / `2>>` / `2>&1`
#     (e.g. mise's `declare -p … 2>/dev/null` no longer leaks to the tty).
#   * `export -a` — `export` accepts leading flags; `-a` is a no-op (bash
#     tolerates it too), so `export -a chpwd_functions` succeeds.
#   * `${arr[@]±word}` — the +/- (and :+/:-) set/unset modifiers on a
#     whole-array `[@]`/`[*]` subscript, including the safe idiom
#     `${arr[@]+"${arr[@]}"}` and associative arrays.
# Each fragment's combined stdout+stderr+exit is compared verbatim.
#
# Fragment notes:
#  * M-90 fragments deliberately use clean comparisons: `2>/dev/null`
#    suppression (both shells print nothing extra) and a `2>&1 | grep -c`
#    that counts the redirected line (the error TEXT differs between shells —
#    `huck:` vs `bash:` — but the COUNT matches). The capture-mode residual
#    `$(builtin 2>&1)` is a known low-impact divergence and is NOT exercised.
#  * `export -a FOO=bar` prints `$FOO` (not `declare -p FOO`): bash makes FOO
#    an array (`declare -ax`), huck keeps it scalar — an intentional, scoped
#    divergence — but the exported VALUE is identical, so `$FOO` compares
#    cleanly.
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

# --- M-90: builtin stderr honors redirections ---

# 1: `2>/dev/null` suppresses a builtin's error (both print only "after")
check "M-90 2>/dev/null suppress" 'declare -p NOPE 2>/dev/null; echo after'

# 2: `2>&1` routes a builtin's error into the pipe (grep -c counts the line)
check "M-90 2>&1 into pipe" '{ declare -p NOPE 2>&1; } | grep -c NOPE'

# 3: suppressed error still yields rc 1 (the && / || branch is taken)
check "M-90 rc preserved" 'declare -p NOPE 2>/dev/null && echo yes || echo no'

# --- export -a (leading flags) ---

# 4: `export -a NAME` (mise shape) — rc 0, no "not a valid identifier"
check "export -a NAME" 'export -a chpwd_functions; echo rc=$?'

# 5: bare `export -a` — rc 0, no output, no full export listing
check "export -a bare" 'export -a; echo done'

# 6: `export -a NAME=val` still exports the value (scalar in huck; value same)
check "export -a assign" 'export -a FOO=bar; printf "%s\n" "$FOO"'

# --- ${arr[@]±word} on whole arrays ---

# 7: `+` on a set array yields the word
check "array + set" 'a=(x y z); printf "<%s>" "${a[@]+SET}"; echo'

# 8: `+` on an unset array yields nothing
check "array + unset" 'unset c; printf "[%s]" "${c[@]+SET}"; echo'

# 9: `-` on a set array yields the elements (separate words)
check "array - set" 'a=(x y z); printf "<%s>" "${a[@]-DEF}"; echo'

# 10: `-` on an empty array yields the word (empty () counts as unset)
check "array - empty" 'b=(); printf "[%s]" "${b[@]-DEF}"; echo'

# 11: the safe-array idiom ${arr[@]+"${arr[@]}"} preserves element boundaries
check "array safe idiom" 'a=(1 2); printf "<%s>" "${a[@]+"${a[@]}"}"; echo'

# 12: associative array `+` set
check "assoc + set" 'declare -A m=([k]=v); printf "<%s>" "${m[@]+SET}"; echo'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
