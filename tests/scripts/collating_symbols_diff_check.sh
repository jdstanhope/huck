#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v337: POSIX.2 collating symbols
# (`[.name.]`/`[.c.]`) inside bracket expressions, in `case`, `[[ == ]]`, and
# `${x#…}` pattern contexts, plus collating-symbol range endpoints.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# single-char + named collating elements
checkf "case single-char a"   'case a in [[.a.]]) echo m;; *) echo no;; esac'
checkf "case hyphen name"     'case - in [[.hyphen.]]) echo m;; *) echo no;; esac'
checkf "case space name"      "case ' ' in [[.space.]]) echo m;; *) echo no;; esac"
checkf "case grave-accent"    'case '\''`'\'' in [[.grave-accent.]]) echo m;; *) echo no;; esac'
# ranges with collating endpoints
checkf "range a-z"            'case p in [[.a.]-[.z.]]) echo m;; *) echo no;; esac'
checkf "range hyphen-9"       'case - in [[.hyphen.]-9]) echo m;; *) echo no;; esac'
checkf "range single-hyphen"  'case 4 in [[.-.]-9]) echo m;; *) echo no;; esac'
# reversed / invalid -> no match (not error)
checkf "reversed range"       'case p in [[.a.]-[.Z.]]) echo bad;; *) echo ok;; esac'
checkf "invalid endpoint"     'case c in [[.yyz.]-[.z.]]) echo bad;; *) echo ok;; esac'
checkf "trailing extra char"  'case p in [[.a.]-[.zz.]p]) echo m;; *) echo no;; esac'
# negation + mixed with a class
checkf "negated collsym"      'case a in [![.a.]]) echo bad;; *) echo ok;; esac'
checkf "mixed with class"     'case 5 in [[:digit:][.hyphen.]]) echo m;; *) echo no;; esac'
# param-expansion + [[ ]] contexts
checkf "param-exp prefix"     'x=abc; echo "${x#[[.a.]]}"'
checkf "dbracket hyphen"      '[[ - == [[.hyphen.]] ]] && echo yes || echo no'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
