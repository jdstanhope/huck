#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the LINE a `for`/`select` header's word
# list is attributed to (#656). An error raised while expanding the list belongs
# to the header's own line; huck stamped `current_lineno` only inside the
# per-iteration loop (and, for an invalid variable name, before it), so the
# expansion ran with whatever the PREVIOUS command left behind:
#
#   * a `for` on line 4 preceded by a command on line 3 reported `line 3`;
#   * a `for` on line 1 printed NO `line N:` prefix at all, because nothing had
#     run yet and a stamped 0 suppresses the prefix.
#
# Found by the runtime sweep: 11+ corpus scripts use `for pkg in \`go list std\``
# and every one of them mis-reported the line when `go` was absent.
#
# A script FILE is the driver: the bug is about line attribution, so the fragment
# has to sit at a known line of a real file.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
# ⚠️ Status captured BEFORE the normalizing pipe (`cmd | sed; echo $?` reports
# sed's status, which is always 0).
check() {
    local label="$1" body="$2" b h out rc
    printf '%s\n' "$body" >"$T/s.sh"
    out=$(cd "$T" && bash s.sh 2>&1 </dev/null); rc=$?
    b=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    out=$(cd "$T" && "$HUCK_BIN" s.sh 2>&1 </dev/null); rc=$?
    h=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── the header line, with something before it ─────────────────────────────────
check 'for + comsub, line 4' 'echo a
echo b
echo c
for p in $(nosuch_xyz); do :; done'
check 'for + backtick, line 4' 'echo a
echo b
echo c
for p in `nosuch_xyz`; do :; done'
check 'for + literal then backtick' 'echo a
echo b
echo c
for p in a b `nosuch_xyz`; do :; done'
check 'the corpus idiom' 'echo a
echo b
echo c
for pkg in `nosuch_xyz list std`; do echo $pkg; done'

# ── at the very top of the file, where the missing prefix showed up ───────────
check 'for on line 1' 'for p in `nosuch_xyz`; do :; done'
check 'for on line 2' 'echo a
for p in $(nosuch_xyz); do :; done'

# ── another error kind from the same list ─────────────────────────────────────
check 'set -u in the list' 'set -u
echo a
echo b
for p in $UNSET_XYZ; do :; done'

# ── `select` shares the header shape ──────────────────────────────────────────
check 'select + backtick, line 4' 'echo a
echo b
echo c
select p in `nosuch_xyz`; do :; done'

# ── controls: constructs that were already right must not move ────────────────
check 'case subject'   'echo a
echo b
echo c
case `nosuch_xyz` in *) ;; esac'
check 'plain argument' 'echo a
echo b
echo c
echo x `nosuch_xyz`'
check 'assignment rhs' 'echo a
echo b
echo c
x=`nosuch_xyz`'
# ...and a `for` that works still iterates.
check 'working list'   'for p in a b c; do echo $p; done'
check 'working comsub' 'for p in $(echo x y); do echo $p; done'
check 'no-in form'     'set -- p q; for p; do echo $p; done'

# ⚠️ No `while`/`until` condition row here, deliberately. `while \`nosuch\`; do
# :; done` HANGS huck: a command whose word list expands to nothing via a
# substitution loses the substitution's status, so a false condition reads true
# and the loop never ends (#661). That is a separate, more serious bug found
# while writing this harness; a row for it would hang the sweep.

harness_summary
