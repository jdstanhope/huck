#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the RETURN trap at the end of a
# SOURCED script (#440). Unlike a function call there is no entry-unset: bash
# runs an INHERITED trap here with or without `functrace`, and `return N`
# partway through fires it too.
#
# The action sees `$?` as the file LEFT it — after `return N` that is the
# status of the last command BEFORE the return, the same rule as #441 for
# functions.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
D=$(mktemp -d); trap 'rm -rf "$D"' EXIT
printf 'trap "echo SRET" RETURN\necho body\n' > "$D/self.sh"
printf 'echo body\n'                          > "$D/plain.sh"
printf 'echo body\nreturn 3\necho unreached\n' > "$D/ret.sh"
printf '(exit 6)\n'                            > "$D/fail.sh"
printf 'echo outer\n. '"$D"'/plain.sh\n'       > "$D/nest.sh"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$D" && timeout 10 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(cd "$D" && timeout 10 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the trap fires at end-of-file -----------------------------------------
check "set inside the file"  '. ./self.sh; echo done'
check "inherited, no -T"     'trap "echo SRET" RETURN; . ./plain.sh; echo done'
check "inherited, with -T"   'set -T; trap "echo SRET" RETURN; . ./plain.sh; echo done'
check "source keyword"       'trap "echo SRET" RETURN; source ./plain.sh; echo done'
check "nested source"        'trap "echo SRET" RETURN; . ./nest.sh; echo done'
check "sourced twice"        'trap "echo SRET" RETURN; . ./plain.sh; . ./plain.sh; echo done'
check "no trap installed"    '. ./plain.sh; echo done'

# --- `return N` partway through --------------------------------------------
check "return N fires"       'trap "echo SRET" RETURN; . ./ret.sh; echo "rc=$?"'
check "return N status"      'trap "echo st=\$?" RETURN; . ./ret.sh; echo "rc=$?"'
check "failing last command" 'trap "echo st=\$?" RETURN; . ./fail.sh; echo "rc=$?"'

# --- a source INSIDE a function ---------------------------------------------
check "in a function, no -T" 'f() { . ./plain.sh; }; trap "echo SRET" RETURN; f; echo done'
check "in a function, -T"    'set -T; f() { . ./plain.sh; }; trap "echo SRET" RETURN; f; echo done'
check "trap set in the file, called from a function" 'f() { . ./self.sh; }; f; echo done'

# --- the action's environment ----------------------------------------------
check "action sees BASH_SOURCE" 'trap "echo src=\${BASH_SOURCE[0]##*/}" RETURN; . ./plain.sh; echo done'
check "reset inside the file"   'trap "echo SRET" RETURN; printf "trap - RETURN\n" > ./r.sh; . ./r.sh; echo done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
