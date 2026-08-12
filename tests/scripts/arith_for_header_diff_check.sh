#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a malformed C-style `for (( … ))`
# header (#313).
#
# bash names the malformation and then echoes the header AS TYPED, on a second
# line that repeats the `syntax error: ` wrapper:
#
#     for ((i=0; i<3)); do :; done
#     bash: -c: line 1: syntax error: arithmetic expression required
#     bash: -c: line 1: syntax error: `((i=0; i<3))'
#
#     for ((i=0; i<3; i++; 7)); do :; done
#     bash: -c: line 1: syntax error: `;' unexpected
#     bash: -c: line 1: syntax error: `((i=0; i<3; i++; 7))'
#
# huck reported its own one-line structural message ("expected 3 sections
# separated by `;`, got 2") — the section COUNT, which bash never mentions.
#
# The echo is the raw source, not a reconstruction: spacing, quotes and even an
# embedded newline come back exactly as written, which is why the rows below
# vary all three. Sections are counted the same way in both shells, so a header
# with fewer than three is a missing expression and one with more is a stray
# `;` — the empty ones (`for ((;;))`) stay legal.
#
# STATUS is compared too: a `$( )` body carrying this error exits 127 under
# `-c` in bash, where huck's bespoke variant used to give 2 (the #574 split).
#
# NOT here: a BACKTICK body with a bad header, which huck aborts and bash
# reports before carrying on (#576), and a `;` inside QUOTES in the header,
# which huck counts as a section separator and bash does not (#602) — both are
# their own divergences, unchanged by this round.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-affh.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# A script FILE and piped stdin are separate top-level readers in huck, and the
# prologue and status differ between them — both drivers get the same rows.
check_file() {
    local label="$1" frag="$2" b h f="$TMPROOT/f.sh"
    printf '%s\n' "$frag" >"$f"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Piped stdin has no argv[0] to set, so each shell names ITSELF in the
# prologue; only the program name is normalised, never the rest of the line.
check_stdin() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | timeout 10 "$BASH_BIN" --norc --noprofile 2>&1 \
        | sed "s|^$BASH_BIN: |SHELL: |"; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 \
        | sed "s|^$HUCK_BIN: |SHELL: |"; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- too few sections: "arithmetic expression required" ---
check "two sections"        'for ((i=0; i<3)); do :; done'
check "one section"         'for ((i=0)); do :; done'
check "empty header"        'for (( )); do :; done'
check "no header at all"    'for (()); do :; done'
check "one empty section"   'for ((;)); do :; done'
check "trailing semi only"  'for ((i=0;)); do :; done'
check "expansion inside"    'for ((i=$x; i<3)); do :; done'
check "quoted section"      'for ((i="a b"; i<3)); do :; done'
check "odd spacing"         'for ((  i=0 ;   i<3  )); do :; done'

# --- too many sections: "`;' unexpected" ---
check "four sections"       'for ((i=0; i<3; i++; 7)); do :; done'
check "five sections"       'for ((i=0; i<3; i++; 7; 8)); do :; done'
check "four last empty"     'for ((i=0; i<3; i++; )); do :; done'
check "four spaced out"     'for (( i = 0 ; i < 3 ; i ++ ; 7 )); do :; done'

# --- the header echo is the raw text ---
check "header with newline" 'for ((i=0
)); do :; done'
check "body on next line"   'for ((i=0))
do :; done'
check "tab in header"       'for ((i=0;	i<3)); do :; done'

# --- where the error appears: each driver, and a later line ---
check "in a function body"  'f(){ for ((i=0; i<3)); do :; done; }; echo defined'
check "after a command"     'echo first
for ((i=0; i<3)); do :; done'
check "inside eval"         'eval "for ((i=0; i<3)); do :; done"; echo "after=$?"'
check "inside comsub"       'x=$(for ((i=0; i<3)); do :; done); echo after'
check "comsub too many"     'x=$(for ((a;b;c;d)); do :; done); echo after'
check "in a subshell"       '(for ((i=0; i<3)); do :; done); echo after'
check_file  "script file"   'for ((i=0; i<3)); do :; done'
check_file  "script line 2" 'echo first
for ((i=0; i<3; i++; 7)); do :; done'
check_stdin "piped stdin"   'for ((i=0; i<3)); do :; done'
check_stdin "stdin line 2"  'echo first
for ((i=0)); do :; done'

# --- controls: the headers that are LEGAL, and the runtime arith error ---
check "all empty"           'for ((;;)); do break; done; echo ok'
check "empty init"          'for ((; i<3; i++)); do break; done; echo ok'
check "empty cond"          'for ((i=0;; i++)); do break; done; echo ok'
check "empty step"          'for ((i=0; i<3;)); do break; done; echo ok'
check "well formed"         'for ((i=0; i<3; i++)); do echo $i; done'
check "comma sections"      'for ((i=0,j=0; i<3; i++,j++)); do echo $i$j; done'
check "no semicolon before do" 'for ((i=0; i<3; i++ )) do echo $i; done'

harness_summary
