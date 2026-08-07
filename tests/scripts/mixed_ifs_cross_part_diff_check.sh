#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #335: under a MIXED IFS (whitespace
# and non-whitespace), a separator run that spans two word parts is still ONE
# delimiter. POSIX: a whitespace-IFS run plus AT MOST ONE non-whitespace IFS
# char is a single delimiter — so `a="a "; b=":b"` splits into 2 fields, while
# `a="a:"; b=":b"` genuinely has two non-whitespace delimiters and yields the
# empty field between them.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
# Every row prints the field COUNT and each field, so a spurious empty shows up.
S='printf "n=%d:" "$#"; printf "<%s>" "$@"; echo'

# --- the cross-part delimiter (the bug) -------------------------------------
check "ws end + nonws start"  "IFS=\" :\"; a=\"a \"; b=\":b\"; set -- \$a\$b; $S"
check "ws end + lone nonws"   "IFS=\" :\"; a=\"x \"; b=\":\"; c=\"y\"; set -- \$a\$b\$c; $S"
check "ws end + nonws+ws"     "IFS=\" :\"; a=\"a \"; b=\": b\"; set -- \$a\$b; $S"
check "ws end + ws + nonws"   "IFS=\" :\"; a=\"a \"; b=\" :b\"; set -- \$a\$b; $S"
check "three parts"           "IFS=\" :\"; a=\"a \"; b=\":\"; c=\":c\"; set -- \$a\$b\$c; $S"
check "all-ws middle part"    "IFS=\" :\"; a=\"a \"; b=\" \"; c=\":c\"; set -- \$a\$b\$c; $S"

# --- delimiters that DO produce an empty field ------------------------------
check "nonws end + nonws"     "IFS=\" :\"; a=\"a:\"; b=\":b\"; set -- \$a\$b; $S"
check "nonws end + ws + nonws" "IFS=\" :\"; a=\"a:\"; b=\" :b\"; set -- \$a\$b; $S"
check "two nonws in one part" "IFS=\" :\"; v=\"a::b\"; set -- \$v; $S"
check "nonws only IFS"        "IFS=:; a=\"x:\"; b=\":y\"; set -- \$a\$b; $S"

# --- unchanged neighbours ---------------------------------------------------
check "default IFS adjacency" "a=\"x \"; b=\"y\"; set -- \$a\$b; $S"
check "trailing ws only"      "IFS=\" :\"; a=\"a \"; set -- \$a; $S"
check "trailing nonws only"   "IFS=\" :\"; a=\"a:\"; set -- \$a; $S"
check "leading nonws"         "IFS=\" :\"; a=\":a\"; set -- \$a; $S"
check "quoted part after"     "IFS=\" :\"; a=\"a \"; set -- \$a\"q\"; $S"
check "empty IFS"             "IFS=; a=\"a \"; b=\":b\"; set -- \$a\$b; $S"
check "single part mixed"     "IFS=\" :\"; v=\"a : b\"; set -- \$v; $S"
check "for-loop context"      "IFS=\" :\"; a=\"a \"; b=\":b\"; n=0; for w in \$a\$b; do n=\$((n+1)); done; echo \$n"

harness_summary
