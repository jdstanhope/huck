#!/usr/bin/env bash
# v209: Bash-diff harness for per-element parameter-expansion modifiers
# applied across whole arrays. Runs each fragment through bash and huck;
# stdout must match byte-for-byte.
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --workspace --bin huck >/dev/null 2>&1
HUCK=target/debug/huck
if [ ! -x "$HUCK" ]; then
    echo "FAIL: huck binary not found at $HUCK" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK" -c "$frag" 2>&1)
    if [ "$b" != "$h" ]; then
        echo "FAIL [$label]"
        echo "  bash: $b"
        echo "  huck: $h"
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# === Case modification (M-127 literal scope) ===
check 'case-upper-all'        'a=(foo bar baz); echo "${a[@]^^}"'
check 'case-upper-first'      'a=(foo bar baz); echo "${a[@]^}"'
check 'case-lower-all'        'a=(FOO BAR); echo "${a[@],,}"'
check 'case-lower-first'      'a=(FOO BAR); echo "${a[@],}"'
check 'case-pattern-arg'      'a=(hello world); echo "${a[@]^^[hl]}"'
check 'case-star-join'        'a=(foo bar); echo "${a[*]^^}"'
check 'case-empty-array'      'a=(); echo "[${a[@]^^}]"'
check 'case-assoc-array'      'declare -A m=([k]=foo [j]=bar); for v in "${m[@]^^}"; do echo "<$v>"; done | sort'

# === Per-element prefix / suffix / substitute ===
check 'suffix-shortest'       'a=(foo.txt bar.md baz.txt); echo "${a[@]%.*}"'
check 'prefix-longest'        'a=(foo.txt bar.md); echo "${a[@]##*.}"'
check 'substitute-first'      'a=(foo bar baz); echo "${a[@]/a/X}"'
check 'substitute-all'        'a=(foo bar baz); echo "${a[@]//[ao]/X}"'
check 'substitute-star'       'a=(foo bar); echo "${a[*]/o/_}"'
check 'suffix-assoc'          'declare -A m=([k]=foo.txt [j]=bar.md); for v in "${m[@]%.*}"; do echo "<$v>"; done | sort'

# === Per-element Transform (the per-element @OP subset) ===
check 'transform-upper'       'a=(foo BAR baz); echo "${a[@]@U}"'
check 'transform-lower'       'a=(foo BAR baz); echo "${a[@]@L}"'
check 'transform-upper-first' 'a=(foo BAR baz); echo "${a[@]@u}"'
check 'transform-quote'       'a=(foo "bar baz"); printf "%s\n" "${a[@]@Q}"'
check 'transform-lower-assoc' 'declare -A m=([k]=Foo [j]=Bar); for v in "${m[@]@L}"; do echo "<$v>"; done | sort'

# === Edge cases ===
check 'sparse-indexed'        'a=([0]=foo [5]=bar [10]=baz); echo "${a[@]^^}"'
check 'empty-element'         'a=(foo "" bar); printf "[%s]\n" "${a[@]^^}"'
check 'single-element'        'a=(foo); echo "${a[@]^^}"'
check 'field-discipline'      'a=(foo bar); for x in "${a[@]^^}"; do echo "<$x>"; done'

if [ $FAIL -ne 0 ]; then
    echo "array_modifiers_diff_check FAILED" >&2
    exit 1
fi
echo "array_modifiers_diff_check OK"
