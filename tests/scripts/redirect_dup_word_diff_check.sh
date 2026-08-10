#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the DUP-REDIRECT WORD (#542): how
# `>&WORD` / `<&WORD` / `N>&WORD` classify the expanded word.
#
# bash's rule, measured against bash 5.2.21 — expand the word, then for the
# single resulting field `s`:
#
#   all ASCII digits  -> dup that fd; if not open, `<label>: Bad file descriptor`
#   `-`               -> close the target
#   empty             -> `<label>: Bad file descriptor`
#   anything else, OUTPUT dup on fd 1 -> filename branch, a synonym for `&>s`
#                                        (BOTH stdout and stderr to that file)
#   anything else, any other target or `<&` -> `s: ambiguous redirect`
#
# `<label>` is the EXPLICIT left-hand fd when one was written (`9>&`), else the
# SOURCE word. Note the asymmetry: `Bad file descriptor` names the source word,
# `ambiguous redirect` names the EXPANSION. A word expanding to != 1 field is
# `<source-word>: ambiguous redirect` regardless of direction.
#
# The filename-branch rows are indistinguishable from a dup by output alone —
# `>&"+7"` silently wrote to fd 7 in huck while bash created a file named `+7`,
# both rc 0 — so every case runs in a FRESH directory and the resulting files
# (name + contents) are compared too.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-dupword.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# DRIVER: piped stdin (the same top-level reader coproc_name_diff_check.sh
# pins). Fragments carry their own `rc=$?` so the status is compared as text
# rather than fished out of PIPESTATUS through the subshell below.
run_in() {
    local kind="$1" frag="$2" d
    d=$(mktemp -d "$TMPROOT/case.XXXXXX")
    if [[ "$kind" == bash ]]; then
        ( cd "$d" && printf '%s\n' "$frag" | timeout 10 bash --norc --noprofile 2>&1 ) \
            | sed -E 's/^bash: (line [0-9]+: )?//'
    else
        ( cd "$d" && printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 ) \
            | sed -E 's#^[^:]*/?huck: (line [0-9]+: )?##'
    fi
    # What landed on disk is the only thing separating a dup from the `&>file`
    # fallback. `./$f` keeps a name like `-5` from being read as an option.
    ( cd "$d" && find . -mindepth 1 -maxdepth 1 -type f -printf '%P\n' 2>/dev/null \
        | LC_ALL=C sort \
        | while IFS= read -r f; do printf 'FILE %s [%s]\n' "$f" "$(cat -- "./$f")"; done )
    rm -rf "$d"
}

check() {
    local label="$1" frag="$2" b h
    b=$(run_in bash "$frag")
    h=$(run_in huck "$frag")
    compare "$label" "$b" "$h"
}

# --- The silent one: `+7` is not all-digits, so bash writes a FILE named `+7`
#     while huck dup'd to fd 7. Same rc 0 on both sides, no diagnostic. ---
check "plus-fd open target"  'exec 7>fd7.out; x=+7; echo PAYLOAD >&"$x"; echo "rc=$?"'
check "plus-fd closed"       'x=+7; echo hi >&"$x"; echo "rc=$?"'
check "minus-fd closed"      'x=-5; echo hi >&"$x"; echo "rc=$?"'

# --- Empty expansion: a bad fd, NOT a filename and NOT `bad fd: …`. ---
check "empty out dup"        'x=; echo hi >&"$x"; echo "rc=$?"'
check "empty in dup"         'x=; read r <&"$x"; echo "rc=$?"'
check "empty exec out"       'x=; exec 9>&"$x"; echo "rc=$?"'
check "empty literal out"    'echo hi >&""; echo "rc=$?"'
check "empty literal in"     'read r <&""; echo "rc=$?"'
# The row coproc_name_diff_check.sh hits once the coprocess is reaped.
check "empty unset array"    'read junk; read r <&"${NOPE[0]}"; echo "rc=$?"'

# --- Non-numeric: filename branch ONLY for an output dup on fd 1. ---
check "nonnum out fd1"       'x=abc; echo hi >&"$x"; echo "rc=$?"'
check "nonnum out fd1 expl"  'x=abc; echo hi 1>&"$x"; echo "rc=$?"'
check "nonnum out fd2"       'x=abc; echo hi 2>&"$x"; echo "rc=$?"'
check "nonnum out fd3"       'x=abc; echo hi 3>&"$x"; echo "rc=$?"'
check "nonnum in dup"        'x=abc; read r <&"$x"; echo "rc=$?"'
check "nonnum in dup expl"   'x=abc; read r 9<&"$x"; echo "rc=$?"'
check "digits-then-alpha"    'x=7abc; echo hi >&"$x"; echo "rc=$?"'
check "leading space digit"  'x=" 7"; echo hi >&"$x"; echo "rc=$?"'
check "quoted two words in"  'x="a b"; read r <&"$x"; echo "rc=$?"'
check "quoted two words out" 'x="a b"; echo hi >&"$x"; echo "rc=$?"'

# --- `>&file` is `&>file`: stderr follows stdout into the file. ---
check "amp-gt both streams"  'x=out; { echo O; echo E >&2; } >&"$x"; echo "rc=$?"'

# --- Bad-file-descriptor label: EXPLICIT redirector wins over the source word. ---
check "closed fd implicit"   'x=7; echo hi >&"$x"; echo "rc=$?"'
check "closed fd explicit"   'x=7; echo hi 9>&"$x"; echo "rc=$?"'
check "closed fd expl in"    'x=7; read r 9<&"$x"; echo "rc=$?"'
check "octal-looking closed" 'x=007; echo hi >&"$x"; echo "rc=$?"'
# A LITERAL fd number is bash's `[n]>&NUMBER` production: it names its own
# NUMERIC value whatever the target, and quoting defeats the production. These
# rows are the ones a target-only rule gets wrong.
check "literal fd nondefault" 'echo hi 4>&77; echo "rc=$?"'
check "literal fd default"    'echo hi >&77; echo "rc=$?"'
check "literal fd input dflt" 'read r 0<&77; echo "rc=$?"'
check "literal octal nondflt" 'echo hi 4>&007; echo "rc=$?"'
check "quoted num nondefault" "echo hi 4>&'77'; echo \"rc=\$?\""
check "dquoted num nondflt"   'echo hi 4>&"77"; echo "rc=$?"'
check "arith num nondefault"  'echo hi 4>&$((70+7)); echo "rc=$?"'
check "expand num nondefault" 'x=77; echo hi 4>&"$x"; echo "rc=$?"'
check "expand num in nondflt" 'x=77; read r 4<&"$x"; echo "rc=$?"'
check "empty expand nondflt"  'x=; echo hi 4>&"$x"; echo "rc=$?"'
check "empty literal nondflt" 'echo hi 4>&""; echo "rc=$?"'
check "compound literal fd"   '{ :; } 4>&77; echo "rc=$?"'
check "subshell literal fd"   '( : ) 4>&77; echo "rc=$?"'
check "huge fd"              'x=99999; echo hi >&"$x"; echo "rc=$?"'
check "overflow fd"          'x=99999999999999999999; echo hi >&"$x"; echo "rc=$?"'

# --- Field count != 1 is ambiguous and names the SOURCE word (not the
#     expansion), in both directions. ---
check "unquoted empty out"   'x=; echo hi >&$x; echo "rc=$?"'
check "unquoted empty in"    'x=; read r <&$x; echo "rc=$?"'
check "unquoted 2field out"  'x="a b"; echo hi >&$x; echo "rc=$?"'
check "unquoted 2field in"   'x="a b"; read r <&$x; echo "rc=$?"'

# --- Controls: the forms that already worked must keep working. ---
check "valid dup out"        'x=2; echo hi >&"$x" 2>err.txt; echo "rc=$?"'
check "valid dup literal"    'echo hi >&2; echo "rc=$?"'
check "dash closes"          'x=-; echo hi >&"$x"; echo "rc=$?"'
check "dash literal closes"  'exec 9>&-; echo "rc=$?"'

harness_summary
