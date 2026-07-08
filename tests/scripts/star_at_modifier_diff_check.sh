#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the ${*-word} / ${@-word} family
# (bash's own iquote.tests). `$*` / `$@` are "set" for the non-colon `-`/`+`/`=`
# tests iff there is at least one positional parameter — NOT iff their joined
# string is non-empty. huck's `lookup_var` had no `*`/`@` case, so every
# `${*-word}` / `${@-word}` modifier saw them as unset and wrongly substituted
# the default even with positionals present (incl. a single empty or a DEL/0x7f
# element). Compares stdout+rc; control bytes via `cat -v`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  h=$("$HUCK_BIN" -c "$f" 2>&1 | cat -v; echo "EXIT:${PIPESTATUS[0]}")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- ${*-word}: set iff $#>0 (Star joins to one field — fully correct) ---
check "star-def 2 args"      'set -- a b; printf "<%s>" "${*-x}"; echo'
check "star-def single-empty" 'set -- ""; printf "<%s>" "${*-x}"; echo'   # $#=1 → set → empty
check "star-def DEL elem"    'set -- $'\''\177'\''; printf "<%s>" "${*-x}"; echo'
check "star-def no args"     'set --; printf "<%s>" "${*-x}"; echo'       # $#=0 → unset → x
check "star-cdef empty"      'set -- ""; printf "<%s>" "${*:-x}"; echo'   # colon → empty → x
check "star-cdef 2 args"     'set -- a b; printf "<%s>" "${*:-x}"; echo'
check "star-alt set"         'set -- a b; printf "<%s>" "${*+Y}"; echo'
check "star-alt single-empty" 'set -- ""; printf "<%s>" "${*+Y}"; echo'   # set → Y
check "star-alt no args"     'set --; printf "<%s>" "${*+Y}"; echo'       # unset → (nothing)
check "star-calt empty"      'set -- ""; printf "<%s>" "${*:+Y}"; echo'   # colon → empty → (nothing)
check "star-in-word"         'set -- ""; recho() { printf "<%s>" "$@"; echo; }; recho "x${*-D}y"'

# --- ${@-word} SET DETECTION (via echo, which joins for display) ---
check "at-def 2 args"        'set -- a b; echo "-${@-x}-"'
check "at-def single-empty"  'set -- ""; echo "-${@-x}-"'
check "at-def no args"       'set --; echo "-${@-x}-"'
check "at-alt set"           'set -- a b; echo "-${@+Y}-"'
check "at-alt no args"       'set --; echo "-${@+Y}-"'

# --- length / regular positionals must be UNCHANGED ---
check "hash-star 2 args"     'set -- a b; echo "${#*}${#@}"'
check "hash-star no args"    'set --; echo "${#*}${#@}"'
check "pos1 empty default"   'set -- ""; echo "-${1-x}-"'   # $1 set(empty) → no default
check "pos1 unset default"   'set --; echo "-${1-x}-"'      # $1 unset → x
check "named unaffected"     'v=hi; echo "${v-D}/${nope-D}"'

echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ "$FAIL" -eq 0 ]]
