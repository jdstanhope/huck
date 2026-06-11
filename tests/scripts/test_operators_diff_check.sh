#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v135: M-27 file/fd test operators + M-14b
# -v array elements. Builds real artifacts; compares each fragment bash vs huck.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
D="$(mktemp -d)"; trap 'rm -rf "$D"' EXIT
mkfifo "$D/fifo"
: > "$D/reg"
mkdir "$D/sticky"; chmod +t "$D/sticky"
: > "$D/suid"; chmod u+s "$D/suid"
: > "$D/sgid"; chmod g+s "$D/sgid"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
for form in '[ %s ]' '[[ %s ]]'; do
  T() { printf "$form" "$1"; }
  check "char /dev/null $form"  "$(T '-c /dev/null') && echo T || echo F"
  check "fifo $form"            "$(T "-p $D/fifo") && echo T || echo F"
  check "fifo-on-reg $form"     "$(T "-p $D/reg") && echo T || echo F"
  check "block /dev/null $form" "$(T '-b /dev/null') && echo T || echo F"
  check "sticky $form"          "$(T "-k $D/sticky") && echo T || echo F"
  check "suid $form"            "$(T "-u $D/suid") && echo T || echo F"
  check "sgid $form"            "$(T "-g $D/sgid") && echo T || echo F"
  check "owned $form"           "$(T "-O $D/reg") && echo T || echo F"
  check "group $form"           "$(T "-G $D/reg") && echo T || echo F"
  check "missing-c $form"       "$(T '-c /no/such/v135') && echo T || echo F"
done
check "term fd0 redir"  '[ -t 0 ] </dev/null && echo T || echo F'
check "term bad fd"     '[ -t 99 ] && echo T || echo F'
check "v idx set"       'a=(x y z); [[ -v a[1] ]] && echo T || echo F'
check "v idx unset"     'a=(x y z); [[ -v a[9] ]] && echo T || echo F'
check "v idx arith"     'a=(x y z); i=2; [[ -v a[i] ]] && echo T || echo F'
check "v assoc set"     'declare -A m; m[k]=1; [[ -v m[k] ]] && echo T || echo F'
check "v assoc unset"   'declare -A m; m[k]=1; [[ -v m[x] ]] && echo T || echo F'
check "v builtin idx"   "a=(x y z); [ -v 'a[1]' ] && echo T || echo F"
check "v plain"         'x=1; [ -v x ] && echo T || echo F'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
