#!/usr/bin/env bash
# v210: bash-diff harness for ${var@A}/@K/@k/@a transforms.
# Asserts byte-identical stdout between bash -c and huck -c.
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

# === @A on scalars ===
check 'A-scalar-plain'        's=hello; echo "${s@A}"'
check 'A-scalar-quote'        'q="it'\''s"; echo "${q@A}"'
check 'A-scalar-empty'        's=; echo "[${s@A}]"'
check 'A-scalar-unset'        'echo "[${u@A}]"'
check 'A-scalar-exported'     'declare -x ev=42; echo "${ev@A}"'
check 'A-scalar-readonly'     'declare -r r=42; echo "${r@A}"'
check 'A-scalar-integer'      'declare -i n=5; echo "${n@A}"'
check 'A-scalar-multi'        'declare -irx mix=7; echo "${mix@A}"'

# === @A on indexed arrays ===
check 'A-indexed-at'          'a=(x y z); echo "${a[@]@A}"'
check 'A-indexed-no-sub'      'a=(x y z); echo "${a@A}"'
check 'A-indexed-i-sub'       'a=(x y z); echo "${a[1]@A}"'
check 'A-indexed-empty'       'declare -a e=(); echo "${e[@]@A}"'

# === @A on assoc arrays — pipe through sort for L-44 (c) order ===
check 'A-assoc-at-sorted'     'declare -A m=([k]=v1 [j]=v2); echo "${m[@]@A}" | tr "[" "\n" | sort'
check 'A-assoc-no-sub'        'declare -A m=([k]=v1 [j]=v2); echo "${m@A}"'
check 'A-assoc-empty'         'declare -A em=(); echo "${em[@]@A}"'

# === @K and @k ===
check 'K-indexed-at'          'a=(x y); echo "${a[@]@K}"'
check 'k-indexed-at'          'a=(x y); echo "${a[@]@k}"'
check 'k-indexed-for-loop'    'a=(x y); for w in "${a[@]@k}"; do echo "<$w>"; done'
check 'K-assoc-sorted'        'declare -A m=([k]=v); echo "${m[@]@K}"'

# === @a attribute flags ===
check 'a-scalar-no-attrs'     's=hello; echo "[${s@a}]"'
check 'a-scalar-integer'      'declare -i n=5; echo "${n@a}"'
check 'a-scalar-exported'     'declare -x e=1; echo "${e@a}"'
check 'a-scalar-multi'        'declare -irx mix=7; echo "${mix@a}"'
check 'a-indexed'             'a=(x); echo "${a@a}"'
check 'a-assoc'               'declare -A m=([k]=v); echo "${m@a}"'
check 'a-unset'               'echo "[${u@a}]"'

# === Subscript + nameref edges (review follow-ons) ===
check 'A-indexed-i-sub-bash'  'a=(x y z); echo "${a[1]@A}"'
check 'A-assoc-k-sub'         'declare -A m=([k]=v1); echo "${m[k]@A}"'
check 'A-via-nameref'         'target=hello; declare -n ref=target; echo "${ref@A}"'

# === Combined / round-trip via eval ===
check 'A-round-trip'          'a=(x y); s="${a[@]@A}"; unset a; eval "$s"; echo "${a[@]}"'

if [ $FAIL -ne 0 ]; then
    echo "array_transforms_diff_check FAILED" >&2
    exit 1
fi
echo "array_transforms_diff_check OK"
