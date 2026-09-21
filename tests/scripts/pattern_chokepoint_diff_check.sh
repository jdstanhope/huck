#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #717/#303 (hub 5 of #765): every
# pattern consumer — `[[ == ]]`, `case`, `${#…}`/`${/…}`/`${^^…}`, completion's
# `-X` filter and pathname expansion — applies bash's bracket rules through one
# chokepoint. An unmatched `[` is an ordinary character (`sm_loop.c`
# BRACKMATCH): `[x` matches `[x`, `[*` matches `[` then anything, `[]` matches
# `[]`. And group detection runs BEFORE bracket handling (PATSCAN): a `[` that
# never closes inside `@(…)` swallows the `)`, so the group is not one.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$tmp" && bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}
m() { echo "[[ $1 == $2 ]] && echo M || echo N"; }

# --- [[ == ]] --------------------------------------------------------------
check '[[ "[x" == [x'          "$(m '"[x"' '[x')"
check '[[ "[b" == [ab'         "$(m '"[b"' '[ab')"
check '[[ "[ab" == [ab'        "$(m '"[ab"' '[ab')"
check '[[ "[]" == []'          "$(m '"[]"' '[]')"
check '[[ "[" == ['            "$(m '"["' '[')"
# (`x[` as a bare word is #75's lexer gap — an unterminated `name[` — not
# a pattern question, so it is not rowed here.)
check '[[ "[*" == [*'          "$(m '"[*"' '[*')$(printf '; '; m '"[anything"' '[*')$(printf '; '; m '"x"' '[*')"
check '[[ "[!a" == [!a'        "$(m '"[!a"' '[!a')"
check '[[ "[^a" == [^a'        "$(m '"[^a"' '[^a')"
check '[[ "[a-" == [a-'        "$(m '"[a-"' '[a-')"
check '[[ "[" == $p'           'p="["; [[ "[" == $p ]] && echo M || echo N; [[ "[" == "$p" ]] && echo M2 || echo N2'
check '[[ x == [[:alpha:]'     "$(m '"x"' '[[:alpha:]')$(printf '; '; m '"[x"' '[[:alpha:]')$(printf '; '; m '"[:alpha:"' '[[:alpha:')"
check '[[ matched classes'     "$(m '"]"' '[]a]')$(printf '; '; m '"b"' '[!]a]')$(printf '; '; m '"-"' '[a-]')$(printf '; '; m '"b"' '[a-c]')$(printf '; '; m '"["' '[[]')"

# --- case ------------------------------------------------------------------
check 'case [ in [)'           'case "[" in [) echo M;; *) echo N;; esac'
check 'case a[b in a[b)'       'case "a[b" in a[b) echo M;; *) echo N;; esac'
check 'case [x in [*)'         'case "[x" in [*) echo M;; *) echo N;; esac; case "x" in [*) echo M2;; *) echo N2;; esac'
check 'case via $p'            'p="["; case "[" in $p) echo M;; *) echo N;; esac'

# --- ${…} ------------------------------------------------------------------
check '${v#[} family'          'v="[abc"; echo "${v#[}"; v="a[b"; echo "${v/[/X}"; v="x[y"; echo "${v#*[}"'
check '${v%…} family'          'v="[abc"; echo "${v%[abc}"; echo "${v//[/X}"; echo "${v/#[/X}"; echo "${v/%c/X}"'
check '${v//[/X} multiple'     'v="a[b[c"; echo "${v//[/X}"; echo "${v##*[}"; echo "${v%%[*}"'
check '${v^^[…}'               'v="a[b"; echo "${v^^[}"; echo "${v^^[b}"; echo "${v,,[A}"'
# `${v/…}` bounds its search to MATCHLEN characters when the pattern has no
# `*` outside a bracket: `[*` counts two, so exactly two are replaced.
check '${v/[*/X}'              'v="a[bc"; echo "${v/[*/X}"; echo "${v/#a[*/X}"; echo "${v//[*/X}"; echo "${v/%[*/X}"'
check '${v/[*c/X}'             'v="a[bcd"; echo "${v/[*c/X}"; echo "${v/[b*/X}"; echo "${v/[?/X}"'
check '${v/%[*/X}'             'v="a[bc"; echo "${v/%[*/X}"; v="a[b"; echo "${v/%[*/X}"; echo "${v/%b/X}"'
check '${v%[*} unbounded'      'v="a[bc"; echo "${v%[*}"; echo "${v%%[*}"; echo "${v#*[}"'
check '[a- never matches'      "$(m '"[a-"' '[a-')$(printf '; '; m '"x"' '[a-')"'; v="[a-"; echo "${v/[a-/X}"; echo "${v#[a-}"'

# --- pathname expansion ----------------------------------------------------
files='touch "[" "a[b" "[x" "[[" "]"'
check 'glob [ a[b [x'           "$files; echo [ a[b [x"
check 'glob [*'                 "$files; echo [*"
check 'glob a[*'                "$files; echo a[*"
check 'glob [[*'                "$files; echo [[*"
check 'glob ]'                  "$files; echo ]"
check 'glob [[]*'               "$files; echo [[]*"
check 'glob [[:alpha:]*'        "$files; echo [[:alpha:]*"
check 'glob [!a]*'              "$files; echo [!a]*"

# --- completion -X (#303) --------------------------------------------------
check 'compgen -X a[b'          'compgen -W "a[b [x abc" -X "a[b"'
check 'compgen -X [[:alpha:]]*' 'compgen -W "a[b [x abc" -X "[[:alpha:]]*"'
check 'compgen -X [[:alpha:]*'  'compgen -W "a[b [x abc" -X "[[:alpha:]*"'
check 'compgen -X [*'           'compgen -W "a[b [x abc" -X "[*"'

# --- extglob: a group is decided before brackets (PATSCAN) ------------------
x='shopt -s extglob; '
check 'extglob @([x)'           "$x$(m '"[x"' '@([x)')$(printf '; '; m '"@([x)"' '@([x)')"
check 'extglob @(a|[b)'         "$x$(m '"[b"' '@(a|[b)')$(printf '; '; m '"a"' '@(a|[b)')"
check 'extglob [@(a|b)'         "$x$(m '"[a"' '[@(a|b)')"
check 'extglob !([a)'           "$x$(m '"b"' '!([a)')$(printf '; '; m '"!([a)"' '!([a)')"
check 'extglob @(a)[a'          "$x$(m '"[a"' '@(a)[a')$(printf '; '; m '"a[a"' '@(a)[a')"
check 'extglob ${v/@([)/X}'     "${x}"'v="a[b"; echo "${v/@([)/X}" "${v/[/X}"'
check 'extglob @(x)['           "$x$(m '"x["' '@(x)[')$(printf '; '; m '"[a"' '[[:alpha:]')"

# --- the pattern text is bash's form: `\c` for a quoted char (#589) --------
check 'xtrace quoted pattern'     'set -x; [[ abc == "a*" ]]; [[ ab == "a"* ]]; x="a*"; [[ ab == "$x" ]]; [[ a == \a ]]; [[ "*" == "*" ]]'
check 'quoted glob chars'         'p="\*"; [[ "*" == $p ]] && echo M || echo N; [[ "a" == $p ]] && echo M2 || echo N2; case "*" in "*") echo C;; esac; case "a" in "*") echo C2;; *) echo N4;; esac'
check 'escaped class members'     "$(m '"]"' '[\]]')$(printf '; '; m '"-"' '[a\-z]')$(printf '; '; m '"b"' '[a\-z]')"'; v="a-b"; echo "${v/[\-]/X}" "${v/[a\-z]/X}"'
check 'trailing backslash'        'case "ab\\" in ab\\) echo M;; *) echo N;; esac; case "ab\\" in "ab\\") echo M2;; *) echo N2;; esac'
check 'quoted extglob chars'      'shopt -s extglob; [[ "a|b" == @("a|b") ]] && echo M; [[ "a" == @("a|b") ]] && echo M2 || echo N2; [[ "(x)" == "(x)" ]] && echo M3'
check 'globbing quoted metas'     'touch "a*b" "aXb" "a\\b"; echo "a*b" a*b "a\\b" '"'"'a\b'"'"''
check '${v/…} quoted metas'       'v="a*b"; echo "${v/\*/X}" "${v/"*"/X}" "${v/*/X}" "${v#*}" "${v#\*}" "${v#a\*}"'

harness_summary
