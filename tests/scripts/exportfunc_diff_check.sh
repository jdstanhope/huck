#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the exportfunc roots fixed in v348
# (#339): R1 import a hyphen-named exported function; R4 reject exporting a
# function whose name can't be env-encoded; R3 HEREDOC_MAX=16 (CVE-2014-7186);
# R2 eval EOF line-continuation line number. Each fragment runs through
# `bash -c` and `huck -c` (each shell re-invokes ITSELF as the child so the
# import round-trip is same-shell); stdout+stderr+exit must match byte-for-byte
# after normalizing the program-name prefix on error lines.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# $2 runs under both shells; `$SH` inside the fragment is the shell under test.
check() {
    local label="$1" frag="$2" b h
    b=$(SH=bash bash --norc --noprofile -c "$frag" 2>&1 | sed 's#^[^:]*:#SH:#'; echo "EXIT:${PIPESTATUS[0]}")
    h=$(SH="$HUCK_BIN" "$HUCK_BIN" -c "$frag" 2>&1 | sed 's#^[^:]*:#SH:#'; echo "EXIT:${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# ── R1: import a hyphen-named exported function (round-trip through the env) ──
check "R1 hyphen func import"   'foo-a() { echo ok2; }; export -f foo-a; $SH -c foo-a'
check "R1 normal func import"   'myfn() { echo hi; }; export -f myfn; $SH -c myfn'
# shellshock guard: a smuggled trailing command must NOT run in the child
check "R1 shellshock guarded"   'env "BASH_FUNC_x%%=() { :; }; echo VULN" $SH -c x 2>/dev/null; echo done'

# ── R4: reject exporting a function whose name can't be env-encoded ──
check "R4 reject = name"        "export -f 'foo=bar'; echo rc=\$?"
check "R4 reject / name"        "export -f '/bin/echo'; echo rc=\$?"

# ── R3: HEREDOC_MAX=16 (17th errors) ──
check "R3 16 heredocs ok" 'cat <<A <<B <<C <<D <<E <<F <<G <<H <<I <<J <<K <<L <<M <<N <<O <<P >/dev/null
A
B
C
D
E
F
G
H
I
J
K
L
M
N
O
P
echo under-limit-ok'
check "R3 17 heredocs error" 'cat <<A <<B <<C <<D <<E <<F <<G <<H <<I <<J <<K <<L <<M <<N <<O <<P <<Q
echo after'

# ── R2: eval EOF line-continuation line number ──
check "R2 eval bslash-eof line#" $'eval \'X() { (a)>\\\''

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
