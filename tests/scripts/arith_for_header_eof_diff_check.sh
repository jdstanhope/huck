#!/usr/bin/env bash
# Byte-identical bash<->huck harness for input that ends inside a C-style
# `for (( … ))` HEADER (#625).
#
# bash reports it like any other open delimiter — which one ran out, at the line
# it opened on. huck replaced it with its own generic wording at the input's LAST
# line:
#
#     echo a
#     for ((i=0;i<3
#     bash: line 2: unexpected EOF while looking for matching `)'
#     huck: line 3: syntax error: unexpected end of file
#
# The header shares `scan_step_arith` with `$((`/`$[`/`((`, so the lex error
# already knew the delimiter and its opening line; `parse_arith_for_clause`
# mapped every lex error to `UnterminatedLoop` and threw both away. The rows
# below therefore cover the same delimiters those constructs report: the `))`
# itself, a quote span inside the header (#621), a backtick, and a `$( )` — which
# bash reports at the EOF line rather than its opening line, so a fix that
# reported "where it opened" for everything would break it.
#
# Controls, all of which already agreed and must keep agreeing: a header that
# CLOSES and then runs out (`do`/`done` missing) is the generic end-of-file shape
# in both, a four-section header is a near-token error, and `for ( (` is not an
# arith header at all.
#
# NOT here, both pre-existing and each its own issue: an unterminated `${…}`
# inside the header, where bash names the arith `)` and huck names `}` (#627) —
# it is the same in `$((`, so it is that issue's shape, not this one's (the
# QUOTED form `"a${x` does agree and is a row below); and a header closed by a
# SINGLE `)` (`for ((i=0;i<3)`), which bash reports as a near-token error at the
# header's line (#628). That one arrives as `ArithBail` rather than a lex error,
# so it does not travel the path this harness covers.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-afh.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# The fragment goes THIRD in a 4-line file, so a line number that is right for
# the wrong reason (first line, last line, one-past-EOF) still shows up.
check_file() {
    local label="$1" frag="$2" b h
    printf 'echo a\necho b\n%s\necho c\n' "$frag" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# A whole file verbatim, for headers broken across lines.
check_raw() {
    local label="$1" src="$2" b h
    printf '%s\n' "$src" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check_stdin() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | timeout 10 "$BASH_BIN" --norc --noprofile 2>&1 \
        | sed "s|^$BASH_BIN: |SHELL: |"; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | timeout 10 "$HUCK_BIN" 2>&1 \
        | sed "s|^$HUCK_BIN: |SHELL: |"; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the header runs out: which delimiter, and where it opened ---
check_file "one section"        'for ((i=0'
check_file "two sections"       'for ((i=0;i<3'
check_file "three sections"     'for ((i=0;i<3;i++'
check_file "bare opener"        'for (('
check_file "trailing semi"      'for ((i=0;'
check_file "nested paren"       'for ((i=0;i<(1+2'
# The delimiters the shared arith scanner reports for a span inside the body.
check_file "dquote in header"   'for ((i=0;i<"'
check_file "squote in header"   "for ((i=0;i<'"
check_file "backtick in header" 'for ((i=0;i<`cmd'
check_file "comsub in header"   'for ((i=0;i<$(cmd'
check_file "brace inside a quote" 'for ((i=0;i<"a${x'
check_file "closed quote, header open" 'for ((i=0;i<"3"'

# --- broken across lines: the header's own line, not the file's last ---
check_raw "header over 3 lines" 'echo a
for ((i=0;
i<3;
i++'
check_raw "quote opens on a later line" 'echo a
for ((i=0;
i<3;
"abc'
check_raw "header then blank lines" 'echo a
for ((i=0;i<3

'

# --- piped stdin: the other top-level reader ---
check_stdin "stdin two sections"  'for ((i=0;i<3'
check_stdin "stdin dquote"        'for ((i=0;i<"'
check_stdin "stdin over two lines" 'for ((i=0;
i<3'

# --- controls: shapes that are NOT an unterminated header ---
check_file "closed header, no do"   'for ((i=0;i<3;i++))'
check_file "no done"                'for ((i=0;i<3;i++)); do :'
check_file "body left open"         'for ((i=0;i<3;i++)); do echo $i'
check_file "four sections"          'for ((a;b;c;d)); do :; done'
check_file "not an arith header"    'for ( (i=0'
check_file "complete loop"          'for ((i=0;i<2;i++)); do echo $i; done'
check_file "complete, one line"     'for ((;;)); do break; done'
check_stdin "stdin complete loop"   'for ((i=0;i<2;i++)); do echo $i; done'
check_stdin "stdin no done"         'for ((i=0;i<3;i++)); do :'

harness_summary
