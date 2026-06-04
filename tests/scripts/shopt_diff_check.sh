#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v86: the shopt builtin (M-08d).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# A fixture dir for the glob fragments (a.txt, .hidden, Abc.txt).
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
: > "$FIX/a.txt"; : > "$FIX/.hidden"; : > "$FIX/Abc.txt"

# NOTE: `failglob`'s no-match error text is intentionally NOT diff-checked here:
# bash prints `bash: line N: no match: PAT` while huck uses its `huck: no match:`
# prefix, so the stderr line cannot be byte-identical. failglob's rc + empty
# stdout are covered by tests/shopt_integration.rs (failglob_no_match_aborts_command).
#
# NOTE: the `shopt invalid name` and `shopt set unset excl` fragments are also
# NOT diff-checked here: their error lines differ only by the `huck: shopt:` vs
# `bash: line N: shopt:` prefix, which breaks byte-equality. Their rc is covered
# by tests/shopt_integration.rs (shopt_invalid_name_rc_one,
# shopt_set_and_unset_together_rc_one).

check "bare shopt lists all"   'shopt'
check "shopt -p reinput"       'shopt -p'
check "shopt -s lists on"      'shopt -s'
check "shopt -u lists off"     'shopt -u'
check "shopt -o lists set-o"   'shopt -o'
check "shopt -po reinput"      'shopt -po'
check "set -o table"           'set -o'
check "set +o reinput"         'set +o'
check "shopt -oq posix"        'shopt -oq posix; echo rc=$?'
check "shopt query multi rc"   'shopt -s dotglob; shopt dotglob nullglob; echo rc=$?'
check "shopt -q set then query" 'shopt -s nullglob; shopt -q nullglob; echo $?'
check "nullglob empty"         "cd '$FIX'; shopt -s nullglob; echo no*match"
check "dotglob includes dot"   "cd '$FIX'; shopt -s dotglob; echo *"
check "dotglob off default"    "cd '$FIX'; echo *"
check "nocaseglob match"       "cd '$FIX'; shopt -s nocaseglob; echo a*"
check "nocasematch [[ eq"      'shopt -s nocasematch; [[ ABC == abc ]] && echo m || echo n'
check "nocasematch =~"         'shopt -s nocasematch; [[ ABC =~ ^abc$ ]] && echo m || echo n'
check "nocasematch case"       'shopt -s nocasematch; case ABC in abc) echo m;; *) echo n;; esac'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
