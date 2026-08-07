#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v152: LINENO.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check_c() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check_file() {
    local label="$1" body="$2" f b h
    f=$(mktemp); printf '%s' "$body" > "$f"
    b=$(bash "$f" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "rc=$?")
    rm -f "$f"
    compare "$label" "$b" "$h"
}
# sourced case: write a sub-script to a temp file and source it by PATH (deterministic).
check_sourced() {
    local label="$1" main="$2" sub="$3" mf sf b h
    sf=$(mktemp); printf '%s' "$sub" > "$sf"
    mf=$(mktemp); printf "$main" "$sf" > "$mf"   # %s in main is replaced with the sub path
    b=$(bash "$mf" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$mf" 2>&1; echo "rc=$?")
    rm -f "$sf" "$mf"
    compare "$label" "$b" "$h"
}
check_c "consecutive"  $'echo $LINENO\necho $LINENO\necho $LINENO'
check_c "in function"  $'f(){\n  echo $LINENO\n}\necho before $LINENO\nf'
check_c "if cond+body" $'if [ $LINENO -ge 0 ]; then echo $LINENO; fi'
check_c "while body"   $'i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done'
check_c "nested func"  $'g(){ echo g$LINENO; }\nf(){\n  g\n  echo f$LINENO\n}\nf'
check_file "script"    $'echo $LINENO\nf(){ echo $LINENO; }\nf\necho $LINENO\n'
check_sourced "sourced" $'echo main $LINENO\nsource %s\necho after $LINENO\n' $'echo sub $LINENO\necho sub2 $LINENO\n'
harness_summary
