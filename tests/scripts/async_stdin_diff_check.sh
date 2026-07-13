#!/usr/bin/env bash
# v287 (#126): an async command with no input redirection must default its stdin
# to /dev/null (non-interactive), so it cannot steal the terminal / an open pipe.
# Each async child prints `readlink /proc/self/fd/0` — its fd0 identity — which
# must match bash byte-for-byte: "/dev/null" for the defaulted cases, the fixture
# path for the inherited cases. (readlink never reads fd0, so these cases can't
# hang.) A final functional guard runs a real `cat` under timeout to assert the
# #126 hang is gone.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'alpha\n' > "$WORK/inA"
printf 'beta\n'  > "$WORK/inB"

# Compare a fragment's output+rc between bash and huck. The shell's own stdin is
# taken from $WORK/inA so an inherited async fd0 resolves to a stable path; the
# mktemp dir is masked so the absolute fixture paths compare equal.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && bash        -c "$frag" < "$WORK/inA" 2>&1; echo "rc=$?")
    h=$(cd "$WORK" && "$HUCK_BIN" -c "$frag" < "$WORK/inA" 2>&1; echo "rc=$?")
    b=${b//$WORK/@WORK@}; h=${h//$WORK/@WORK@}
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

R='readlink /proc/self/fd/0'
check "simple cmd &"             "$R & wait"
check "(subshell) &"             "($R) & wait"
check "{ group; } &"             "{ $R; } & wait"
check "and-or (true && x) &"     "true && $R & wait"
check "explicit input redirect"  "$R < inB & wait"
check "bare pipeline (inherit)"  "$R | cat & wait"
check "subshell-wrapped pipe"    "($R | cat) & wait"
check "3-stage pipeline"         "$R | cat | cat & wait"

# Functional anti-hang guard (#126 direct repro): a real `cat` with no redirect
# must get /dev/null and EOF (rc=0), not block on an open pipe. fd0 is a pipe fed
# by `sleep 30` (produces nothing, stays open); a `cat` that inherited it would
# block until timeout. `< <(...)` is bash-only outer syntax feeding the inner shell.
guard() { timeout 5 "$1" -c '/bin/cat & wait; echo "rc=$?"' < <(sleep 30) 2>&1; }
gb=$(guard bash); gh=$(guard "$HUCK_BIN")
if [[ "$gb" == "rc=0" && "$gh" == "rc=0" ]]; then
    printf 'PASS: cat & wait no-hang guard\n'; PASS=$((PASS+1))
else
    printf 'FAIL: cat & wait no-hang guard (bash=[%s] huck=[%s])\n' "$gb" "$gh"; FAIL=$((FAIL+1))
fi

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
