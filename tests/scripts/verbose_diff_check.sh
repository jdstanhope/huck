#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v89: set -v verbose mode (M-08e).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
# NOTE: `eval`/trap re-parsed lines are NOT echoed by huck under -v (bash echoes
# them) — a documented M-08e divergence; excluded from byte-diffing here.
check "v echo basic"        $'set -v\necho hi'
check "v enable not echoed" $'echo a\nset -v\necho b\nset +v\necho c'
check "v multiline if"      $'set -v\nif true\nthen echo x\nfi'
check "v comment+blank"     $'set -v\n# a comment\n\necho done'
check "v dollar-dash"       $'set -v\ncase $- in *v*) echo hasv;; *) echo nov;; esac'
check "v off by default"    $'echo no-verbose'
harness_summary
