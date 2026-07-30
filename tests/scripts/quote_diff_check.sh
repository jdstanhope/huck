#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the quoting/word-expansion roots fixed
# in v346 (#334): R4 empty-field generation in ${x:+word} alternates, R3 IFS as
# a set variable, R1/R2 backtick \<newline> line-continuation removal. Uses an
# arg-counter (set -- ; printf) rather than the suite's external `recho` helper,
# so it needs no C compiler. Each fragment runs through `bash -c` and `huck -c`;
# stdout+stderr+exit must match byte-for-byte.
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
AC='printf "n=%d:" "$#"; printf "<%s>" "$@"; echo'

# ── R4: quoted-empty field generation in ${x:+word} / ${x:-word} alternates ──
check "R4 leading-space empty"   "x=x; e=; set -- \${x:+ \"\"}; $AC"
check "R4 two empties spaced"    "x=x; e=; set -- \${x:+\"\$e\" \"\$e\"\"\"}; $AC"
check "R4 nonempty+space+empty"  "x=x; set -- \${x+ab \"\$y\"}; $AC"
check "R4 nonempty+space+sqempty" "x=x; set -- \${x+ab ''}; $AC"
check "R4 cmdsub empties"        "x=x; set -- \${x:+\"\$(:)\" \"\$(:)\"\"\"}; $AC"
check "R4 default form"          "unset u; set -- \${u:- \"\"}; $AC"
# KEEP (already correct — must not gain spurious fields)
check "R4 keep single dq empty"  "x=x; set -- \${x:+\"\"}; $AC"
check "R4 keep adjacent empties" "x=x; e=; set -- \${x:+\"\$e\"\"\$e\"\"\"}; $AC"
check "R4 keep sq empty"         "x=x; set -- \${x:+''}; $AC"

# ── R3: IFS is a SET variable with the default value ──
check "R3 IFS is set"            'echo ${IFS+SET}'
check "R3 IFS default value"     'printf "[%s]" "$IFS" | od -An -c'
check "R3 IFS- shows value"      'echo "${IFS-UNSET}" | od -An -c'
check "R3 IFS+ in dquote nested" 'printf "%s\n" "foo ${IFS+"b   c"} baz"'
check "R3 custom IFS still sets" 'IFS=:; echo "${IFS+set}=${IFS}"'
check "R3 unset IFS still splits" 'unset IFS; a="a b c"; set -- $a; echo $#'

# ── R1/R2: backtick \<newline> line-continuation removed (even in single quotes) ──
check "R1 backtick bslash-nl sq" $'echo `echo \'foo\\\nbar\'`'
check "R1 backtick bslash-nl dq" $'echo `echo "foo\\\nbar"`'
check "R1 backtick keeps \\\\"    'echo `echo foo\\bar`'
check "R1 backtick \$ unescape"   'echo `echo \$HOME`'

# ── regressions: normal splitting/quoting unchanged ──
check "regr normal split"        'v="a b c"; set -- $v; echo $#'
check "regr adjacent split"      'p="a b"; q="c d"; set -- $p$q; echo "$#=$1=$2=$3"'
check "regr trailing-ws split"   'x="ab "; y=cd; set -- $x$y; echo "$#=$1=$2"'
check "regr top-level empty arg" 'set -- a "" b; echo $#'
check "regr IFS colon"           'IFS=:; v="a::b"; set -- $v; echo "$#:$1:$2:$3"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
