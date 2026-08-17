#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `trap SIGSPEC` — the reset form with no
# action and no `-` (#654). huck rejected it as a usage error, so the classic
# cleanup idiom `trap 'rm -f $tmp; trap 0; exit' 0` printed a spurious usage line
# every time the trap fired. Found by the runtime sweep in mawk's `hical` example.
#
# The rule is NOT what POSIX's wording ("if the first operand is an unsigned
# decimal integer") suggests, and every row below was measured against bash
# 5.2.21 before being asserted:
#
#   trap 0          reset EXIT            a lone spec, numeric
#   trap INT        reset INT             a lone spec, NAME — so the integer
#                                         rule is not the whole story
#   trap 1 2        reset BOTH            first operand is a signal NUMBER
#   trap 65 2       action `65` on INT    65 is out of range, so it is an action
#   trap EXIT INT   action `EXIT` on INT  a NAME first operand is an action
#
# ⚠️ Rows use `trap -p SIGNAL`, never bare `trap -p`: a bare listing also shows
# dispositions INHERITED from the environment (a SIGTSTP ignored by the harness's
# own parent shows up as `trap -- '' SIGTSTP`), which would make these rows
# depend on how the sweep was launched.
#
# ⚠️ No row uses a signal above 31. huck has no real-time signals (#405), and
# because the discriminator above is the NUMBER's validity, `trap 64 2` is read
# as an action there — a real divergence, but that issue's, not this one's.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# ⚠️ The status is captured BEFORE the normalizing pipe. `cmd | sed; echo $?`
# reports SED's status, which is always 0 — so the EXIT line would assert nothing
# and every rc divergence below would pass silently.
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out"; echo "EXIT:$rc")
    out=$("$HUCK_BIN" -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | sed "s|^$HUCK_BIN: |bash: |"; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── the lone-spec reset ───────────────────────────────────────────────────────
check "lone 0 resets EXIT"      'trap "echo E" 0; trap 0; echo done'
check "lone EXIT resets"        'trap "echo E" EXIT; trap EXIT; echo done'
check "lone name resets"        'trap "echo I" INT; trap INT; trap -p INT; echo done'
check "lone number resets"      'trap "echo T" 15; trap 15; trap -p 15; echo done'
check "lone DEBUG resets"       'trap "echo D" DEBUG; trap DEBUG; echo done'
check "lone spec after --"      'trap "echo E" 0; trap -- 0; echo after'
# The idiom that found it: reset the EXIT trap from inside the action so the
# cleanup cannot run twice.
check "reset from the action"   'trap "echo cleanup; trap 0; exit" 0'

# ── the POSIX integer rule: a signal NUMBER first means ALL operands reset ────
check "number first resets all" 'trap "echo A" 1 2; trap 1 2; trap -p 1; trap -p 2; echo end'
check "number first, mixed"     'trap "echo A" 2; trap 15 2; trap -p 2; echo end'

# ── and what is still an ACTION ───────────────────────────────────────────────
check "name first is an action" 'trap EXIT INT; trap -p INT'
check "big number is an action" 'trap 65 2; trap -p 2; echo end'
check "huge number is action"   'trap 999 2; trap -p 2; echo end'
check "empty action ignores"    'trap "" 0; trap -p 0'

# ── invalid specs ─────────────────────────────────────────────────────────────
# A LONE bad operand is ambiguous (action-with-no-signal, or bad signal?) and
# bash resolves it as the usage error, rc 2 — not "invalid signal specification".
check "lone bad name"           'trap NOSUCHSIG'
check "lone bad number"         'trap 999'
check "lone empty string"       'trap ""'
# With a valid NUMBER first, every operand IS a condition, so a bad one here
# does reach the invalid-signal message (rc 1). Before #654 this row passed for
# the WRONG reason: `0` was read as the action and `NOSUCHSIG` as its signal.
check "bad spec after number"   'trap 0 NOSUCHSIG'

harness_summary
