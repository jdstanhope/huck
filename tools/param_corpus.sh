#!/usr/bin/env bash
# Parameter-expansion execution corpus (#635, v360 Task 1).
#
# `tools/parse_sweep.sh` runs both shells with -n, so it proves a parser change
# did not alter what PARSES — it cannot see a change in what an expansion
# EVALUATES TO. This runs a corpus of `${…}` forms for real and prints one row
# per form, so two builds of huck can be diffed against each other:
#
#     tools/param_corpus.sh <huck-binary> > before.tsv
#     …change parse_param_expansion…
#     tools/param_corpus.sh target/debug/huck > after.tsv
#     diff before.tsv after.tsv
#
# Identical output is the inertness gate for a refactor of that function. It is
# deliberately a huck-vs-huck comparison, NOT huck-vs-bash: the point is "this
# refactor changed nothing", and forms where huck already diverges from bash
# must stay exactly as divergent as they were.
#
# Output: fragment <TAB> stdout <TAB> stderr <TAB> rc, newlines escaped so each
# row stays on one line. Every run is capped (`ulimit -v`, `timeout`) — an
# unbounded probe fragment has OOM-killed this box before.
set -u

HUCK="${1:-target/debug/huck}"
[ -x "$HUCK" ] || { echo "usage: $0 <huck-binary>   (not executable: $HUCK)" >&2; exit 2; }

# Preamble every fragment runs under, so operands have something to expand.
PREAMBLE='x=abc; e=; u_unset_marker=1; unset u; a=(p q r); declare -A h=([k]=v); n=5; s="a b"; IFS=" "'

run_one() {
    local frag="$1" out err rc
    out=$( (ulimit -v 500000; timeout 5 "$HUCK" -c "$PREAMBLE
$frag") 2>/tmp/pc_err.$$ )
    rc=$?
    err=$(cat /tmp/pc_err.$$); rm -f /tmp/pc_err.$$
    # Normalise the binary's own path in BOTH streams: `${0}` prints it on
    # stdout, and the two builds being compared necessarily live at different
    # paths, which would otherwise read as a behaviour change.
    printf '%s\t%s\t%s\t%s\n' \
        "$(printf '%s' "$frag" | tr '\n' '~')" \
        "$(printf '%s' "$out" | tr '\n' '~' | sed "s|$HUCK|SHELL|g")" \
        "$(printf '%s' "$err" | tr '\n' '~' | sed "s|$HUCK|SHELL|g")" \
        "$rc"
}

emit() { for f in "$@"; do run_one "$f"; done; }

# ---- plain names, length, indirection ------------------------------------
emit 'echo ${x}' 'echo "${x}"' 'echo ${e}' 'echo "${e}"' 'echo ${u}' 'echo "${u}"' \
     'echo ${#x}' 'echo ${#e}' 'echo ${#u}' 'echo "${#x}"' \
     'r=x; echo ${!r}' 'r=x; echo "${!r}"' 'echo ${!a[@]}' 'echo ${!h[@]}' \
     'echo ${#a[@]}' 'echo ${#a[0]}' 'echo ${#}' 'echo ${0}'

# ---- the operator family, unquoted and quoted, on set/empty/unset --------
for op in ':-' '-' ':=' '=' ':?' '?' ':+' '+'; do
    for v in x e u; do
        emit "echo \${$v${op}D}" "echo \"\${$v${op}D}\"" "echo \${$v${op}}"
    done
done

# ---- pattern operators ---------------------------------------------------
emit 'p=abcabc; echo ${p#a}' 'p=abcabc; echo ${p##a*b}' 'p=abcabc; echo ${p%c}' \
     'p=abcabc; echo ${p%%b*}' 'p=abcabc; echo "${p#a}"' 'p=abcabc; echo "${p##a*b}"' \
     'p=abcabc; echo ${p/b/X}' 'p=abcabc; echo ${p//b/X}' 'p=abcabc; echo ${p/#a/X}' \
     'p=abcabc; echo ${p/%c/X}' 'p=abcabc; echo "${p//b/X}"' 'p=abcabc; echo ${p//b}' \
     'p=aBcD; echo ${p^}' 'p=aBcD; echo ${p^^}' 'p=aBcD; echo ${p,}' 'p=aBcD; echo ${p,,}' \
     'p=aBcD; echo "${p^^}"' 'p=aBcD; echo ${p^^[ac]}' \
     'echo ${x:1}' 'echo ${x:1:1}' 'echo ${x: -2}' 'echo "${x:1:1}"' 'echo ${x:n}' \
     'echo ${x@Q}' 'echo ${x@U}' 'echo ${x@L}' 'echo ${x@a}' 'echo "${x@Q}"'

# ---- arrays and subscripts ----------------------------------------------
emit 'echo ${a[0]}' 'echo ${a[@]}' 'echo ${a[*]}' 'echo "${a[@]}"' 'echo "${a[*]}"' \
     'echo ${a[1]:-D}' 'echo ${a[9]:-D}' 'echo ${a[@]:1}' 'echo ${a[@]:1:1}' \
     'echo ${a[@]/p/X}' 'echo ${a[@]#p}' 'echo "${a[@]^^}"' \
     'echo ${h[k]}' 'echo ${h[nope]:-D}' 'echo "${h[@]}"' 'echo ${a[$((1+1))]}' \
     'echo ${a[n-4]}' 'echo ${#h[@]}'

# ---- nesting and quoting interactions -----------------------------------
emit 'echo ${x:-${e:-D}}' 'echo "${x:-${e:-D}}"' 'echo ${e:-${u:-D}}' \
     'echo ${e:-"q r"}' "echo \${e:-'q r'}" 'echo "${e:-"q r"}"' "echo \"\${e:-'q r'}\"" \
     'echo ${e:-$(echo sub)}' 'echo "${e:-$(echo sub)}"' 'echo ${e:-`echo bt`}' \
     'echo ${e:-$((1+1))}' 'echo "${e:-$((1+1))}"' 'echo ${e:-$x}' 'echo "${e:-$x}"' \
     "echo \${e:-\$'a\\tb'}" "echo \"\${e:-\$'a\\tb'}\"" \
     'echo ${x#$(echo a)}' 'echo ${x/$(echo b)/Y}' 'echo ${e:-${a[0]}}' \
     'echo "pre${x}post"' 'echo pre${x}post' 'echo ${x}${x}' 'echo "${x}${e}${x}"'

# ---- special parameters --------------------------------------------------
emit 'set -- one two; echo ${1}' 'set -- one two; echo ${#@}' 'set -- one two; echo "${@}"' \
     'set -- one two; echo "${*}"' 'set -- one two; echo ${@:1:1}' 'set -- one two; echo ${1:-D}' \
     'set -- one two; echo ${10:-D}' 'echo ${?}' 'echo ${$:+pid}'

# ---- UNTERMINATED at every operand type ---------------------------------
# One row per operand MODE the parser can be inside when input runs out. Each
# such mode is pushed and popped by its own arm, and an arm that pops one mode
# too many takes out the `Command` floor. The first cut of this corpus stopped
# at `${x:-` and `${x`, which left the substitute and substring arms uncovered —
# and those two were exactly the arms that still popped the `ParamExpansion`
# frame after it moved to a single exit, panicking on `echo ${x:1`.
emit 'echo ${x:' 'echo ${x:1' 'echo ${x:1:' 'echo ${x:1:2' 'echo ${x: -' \
     'echo ${x/' 'echo ${x/a' 'echo ${x/a/' 'echo ${x/a/b' 'echo ${x//' \
     'echo ${x//a/' 'echo ${x/#' 'echo ${x/%a' \
     'echo ${x#' 'echo ${x##' 'echo ${x%' 'echo ${x%%' 'echo ${x#a' 'echo ${x%a' \
     'echo ${x^' 'echo ${x^^' 'echo ${x,' 'echo ${x,,' 'echo ${x^a' \
     'echo ${x:-' 'echo ${x:=' 'echo ${x:?' 'echo ${x:+' 'echo ${x:-a' \
     'echo ${x@' 'echo ${!x' 'echo ${#x' 'echo ${a[0]' 'echo ${a[0]:-' \
     'echo "${x:1' 'echo "${x/a/b' 'echo "${x#a' 'echo "${x:-a'

# ---- malformed / error paths (the exits this refactor moves) -------------
emit 'echo ${}' 'echo ${1x}' 'echo ${x!}' 'echo ${x:-' 'echo ${x' 'echo ${#x:-D}' \
     'echo ${!x[@]:-D}' 'echo ${x[0]}' 'echo ${a[}' 'echo ${a[0}' 'echo ${x@Z}' \
     'echo ${x%%%%%}' 'echo ${u?custom message}' 'echo ${u:?}' 'readonly ro=1; echo ${ro:=no}' \
     'echo ${x:-D}; echo after' 'echo ${nope?}; echo after' 'echo ${}; echo after'

# ---- set -u interactions -------------------------------------------------
emit 'set -u; echo ${x}' 'set -u; echo ${u:-D}' 'set -u; echo ${u}' 'set -u; echo ${#u}' \
     'set -u; echo ${u+set}' 'set -u; set -- ; echo ${1:-D}'

# ---- prefix/name listing, deeper nesting, operand quoting corners --------
emit 'pre1=a; pre2=b; echo ${!pre@}' 'pre1=a; pre2=b; echo ${!pre*}' 'pre1=a; echo "${!pre@}"' \
     'echo ${x:-${e:-${u:-deep}}}' 'echo "${x:-${e:-${u:-deep}}}"' \
     'echo ${e:-${a[@]}}' 'echo "${e:-${a[@]}}"' 'echo ${a[@]:-D}' \
     'echo ${e:+${x}}' 'echo "${e:+${x}}"' 'echo ${x:+${e:-inner}}' \
     'echo ${x/${x}/Y}' 'echo ${x#${x}}' 'echo "${x/b/${e:-Z}}"' \
     'echo ${e:-"$x"}' "echo \${e:-\"\$(echo s)\"}" 'echo ${e:-\}}' \
     'echo ${x:0:0}' 'echo "${x:0:0}"' 'echo ${a[@]:0:0}'
