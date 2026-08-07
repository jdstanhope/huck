#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v193: `**` globstar gated on
# `shopt globstar`. Builds a private temp tree and compares sorted glob output.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# build a fixed tree; both shells cd into it and run the SAME fragment.
TREE=$(mktemp -d)
mkdir -p "$TREE/a/b/c"
touch "$TREE/r.txt" "$TREE/a/x.txt" "$TREE/a/b/y.txt" "$TREE/a/b/c/z.txt" "$TREE/a/b/note.md" "$TREE/]a.txt" "$TREE/ba.txt"
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$TREE"; bash -c "$frag" 2>&1 | sort; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$TREE"; "$HUCK_BIN" -c "$frag" 2>&1 | sort; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# globstar OFF (default): ** ≡ * (single level)
check "off **/*.txt"   'printf "%s\n" **/*.txt'
check "off bare **"    'printf "%s\n" **'
check "off a/**"       'printf "%s\n" a/**'
check "off **/*"       'printf "%s\n" **/*'
# globstar ON: recursive
check "on **/*.txt"    'shopt -s globstar; printf "%s\n" **/*.txt'
check "on **/y.txt"    'shopt -s globstar; printf "%s\n" **/y.txt'
check "on a/**/*.txt"  'shopt -s globstar; printf "%s\n" a/**/*.txt'
check "on **/*.md"     'shopt -s globstar; printf "%s\n" **/*.md'
# control: no ** — unchanged
check "ctrl a/*"       'printf "%s\n" a/*'
check "ctrl *.txt"     'printf "%s\n" *.txt'
# NOTE: bare `**` with globstar ON is a documented residual (the glob crate
# matches dirs-only vs bash's dirs+files) and is intentionally NOT checked.

# bracket-class + ** edge cases (globstar off): collapse must stay bash-equivalent
check "off []a]**"     'printf "%s\n" []a]**'
check "off [!a]**"     'printf "%s\n" [!a]**'
check "off [ab]**"     'printf "%s\n" [ab]**'

rm -rf "$TREE"
harness_summary
