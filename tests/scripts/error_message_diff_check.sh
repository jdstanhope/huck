#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v269 T2: the unified error-message
# emitter's §3 prologue matrix (docs/superpowers/specs/2026-07-07-unified-error-emitter-design.md).
#
# Runtime-error message BODIES already match bash byte-for-byte (cd/readonly
# were converted in earlier iterations), so those cells get a full diff after
# normalizing the <name> field. Syntax/pre-shell-CLI message BODIES are
# intentionally NOT bash-parity (out of scope per the spec's non-goals) — those
# cells assert only the PROLOGUE shape (name/`-c:`/`line N:` segments), not the
# message text.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
BASH_BIN="${BASH_BIN:-bash}"
PASS=0; FAIL=0

# huck's <name> is $0 verbatim (never basenamed) — by default that's its own
# invocation path (argv[0]), which differs textually from bash's own "bash".
# Normalize by substituting huck's invocation string for bash's so the two
# sides' name field lines up. Script-file cells need no normalization: both
# sides are invoked with the identical tmp path as $0.
normalize() {
    printf '%s' "${1//$HUCK_BIN/bash}"
}

# checkdiff: full byte-for-byte comparison (after name normalization) for
# cells whose message BODY is already bash-parity.
checkdiff() {
    local label="$1" b="$2" h="$3"
    h=$(normalize "$h")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(printf '%s\n' "$b") <(printf '%s\n' "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# checkshape: regex assertion against huck's own output — for cells whose
# message body legitimately diverges from bash's wording.
checkshape() {
    local label="$1" text="$2" pattern="$3"
    if [[ "$text" =~ $pattern ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        printf '    expected /%s/ to match:\n    %s\n' "$pattern" "$text"
        FAIL=$((FAIL+1))
    fi
}

checknotshape() {
    local label="$1" text="$2" pattern="$3"
    if [[ ! "$text" =~ $pattern ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        printf '    expected /%s/ to NOT match:\n    %s\n' "$pattern" "$text"
        FAIL=$((FAIL+1))
    fi
}

# ---------------------------------------------------------------------------
# 1. Runtime errors (cd /nope) across -c / script-file / stdin: bash-parity
#    body, `line N:` present, `-c:` NEVER present for runtime errors.
# ---------------------------------------------------------------------------
b=$("$BASH_BIN" -c 'cd /nope' 2>&1; echo "EXIT:$?")
h=$("$HUCK_BIN" -c 'cd /nope' 2>&1; echo "EXIT:$?")
checkdiff "runtime cd /nope: -c mode" "$b" "$h"

tmp=$(mktemp "${TMPDIR:-/tmp}/huck-errmsg.XXXXXX")
printf 'cd /nope\n' > "$tmp"
b=$("$BASH_BIN" "$tmp" 2>&1; echo "EXIT:$?")
h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
rm -f "$tmp"
checkdiff "runtime cd /nope: script-file mode" "$b" "$h"

b=$(printf 'cd /nope\n' | "$BASH_BIN" 2>&1; echo "EXIT:$?")
h=$(printf 'cd /nope\n' | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
checkdiff "runtime cd /nope: stdin mode" "$b" "$h"

# ---------------------------------------------------------------------------
# 2. Syntax errors: `-c:` present iff `-c` mode. Message body wording is out
#    of scope (non-goal) — only the prologue shape is asserted.
# ---------------------------------------------------------------------------
h=$(normalize "$("$HUCK_BIN" -c 'if' 2>&1)")
checkshape "syntax -c 'if': -c: segment present" "$h" '^bash: -c: line [0-9]+: '

tmp=$(mktemp "${TMPDIR:-/tmp}/huck-errmsg.XXXXXX")
printf 'if\n' > "$tmp"
h=$("$HUCK_BIN" "$tmp" 2>&1)
rm -f "$tmp"
checkshape "syntax script-file 'if': line N: present" "$h" '^[^:]+: line [0-9]+: '
checknotshape "syntax script-file 'if': -c: absent" "$h" '^[^:]+: -c: '

b=$(printf 'if\n' | "$BASH_BIN" 2>&1)
h=$(printf 'if\n' | "$HUCK_BIN" 2>&1)
checkshape "syntax stdin 'if': line N: present" "$h" '^[^:]+: line [0-9]+: '
checknotshape "syntax stdin 'if': -c: absent" "$h" '^[^:]+: -c: '
# sanity: confirm bash itself has no -c: for stdin syntax errors (cross-check
# our expectation against the live oracle, not just a hardcoded pattern).
checknotshape "syntax stdin 'if': bash oracle has no -c: either" "$b" '-c: '

# ---------------------------------------------------------------------------
# 2b. Source-under-`-c`: a file with a syntax error sourced from a `-c`
#    command string must NOT leak the `-c:` segment onto the sourced file's
#    own prologue (bash: `<file>: line N: ...`, no `-c:`) — `is_command_string`
#    stays true for the whole `-c` invocation, so the gate additionally
#    requires top-level source depth. Body wording is out of scope (non-goal,
#    same as the rest of §2) — only the prologue shape is asserted.
# ---------------------------------------------------------------------------
tmp=$(mktemp "${TMPDIR:-/tmp}/huck-errmsg.XXXXXX")
printf 'if\n' > "$tmp"
b=$("$BASH_BIN" -c "source $tmp" 2>&1)
h=$("$HUCK_BIN" -c "source $tmp" 2>&1)
rm -f "$tmp"
checkshape "syntax source-under-c: line N: present" "$h" '^[^:]+: line [0-9]+: '
checknotshape "syntax source-under-c: -c: absent" "$h" '-c: '
checknotshape "syntax source-under-c: bash oracle has no -c: either" "$b" '-c: '

# ---------------------------------------------------------------------------
# 3. Custom $0 (`myprog`) with a runtime error: body already bash-parity, and
#    both sides are forced to the SAME literal name, so a full byte match
#    needs no normalization.
# ---------------------------------------------------------------------------
b=$("$BASH_BIN" -c 'cd /nope' myprog 2>&1; echo "EXIT:$?")
h=$("$HUCK_BIN" -c 'cd /nope' myprog 2>&1; echo "EXIT:$?")
checkdiff "custom \$0 (myprog): cd /nope" "$b" "$h"
checkshape "custom \$0 (myprog): begins 'myprog: '" "$h" '^myprog: '

# ---------------------------------------------------------------------------
# 4. Double-prefix regression: a `-c` syntax error must yield exactly ONE
#    prologue segment — no hardcoded literal "huck: <name>:" double prefix
#    (the historical bug: builtins.rs hardcoded "huck: {name}: line {N}: ").
# ---------------------------------------------------------------------------
h_raw=$("$HUCK_BIN" -c 'if' 2>&1)
checknotshape "double-prefix regression: no literal 'huck: ' wrapper" "$h_raw" '^huck: '

# ---------------------------------------------------------------------------
# 5. Sink routing: a 2>&1-captured runtime error lands on stdout — proves the
#    diagnostic still routes through real stderr (honors outer redirection)
#    now that it goes through the thread-local sink / emit_syntax_error path.
# ---------------------------------------------------------------------------
b=$("$BASH_BIN" -c 'readonly x=1; x=2' 2>&1; echo "EXIT:$?")
h=$("$HUCK_BIN" -c 'readonly x=1; x=2' 2>&1; echo "EXIT:$?")
checkdiff "sink routing: readonly under -c 2>&1" "$b" "$h"

# ---------------------------------------------------------------------------
# 6. Pre-shell CLI: bad option -> `<basename>: <msg>` — no line, no `-c:`.
#    bash's own --badoption message is an unrelated multi-line usage dump, so
#    only huck's own prologue shape is asserted (not a bash diff).
# ---------------------------------------------------------------------------
h=$("$HUCK_BIN" --badoption 2>&1)
checkshape "pre-shell CLI --badoption: 'huck: ' prefix" "$h" '^huck: '
checknotshape "pre-shell CLI --badoption: no line number" "$h" 'line [0-9]+:'

# ---------------------------------------------------------------------------
# 7. Builtin bare-redirect capture regression (v269 T3b, the sh_error_to!
#    writer-based emitter). A bare builtin's `2>&1` inside `$(...)` must land
#    in the SAME writer the executor's in-memory route_err_to_out swap
#    targets. Builtins converted to the thread-local sh_error! (instead of the
#    writer-based sh_error_to!) lose the diagnostic here, because the swap
#    lives only in the `out`/`err` writer params `run_builtin` hands the
#    builtin, not in the thread-local sink. Verified bug (pre-fix): `x=$(cd
#    /nonexistent 2>&1); echo "$x"` printed `[]` (empty) instead of capturing
#    the error.
# ---------------------------------------------------------------------------
b=$("$BASH_BIN" -c 'x=$(cd /nonexistent_xyz 2>&1); echo "$x"')
h=$("$HUCK_BIN" -c 'x=$(cd /nonexistent_xyz 2>&1); echo "$x"')
checkdiff "builtin bare-redirect capture: cd /nonexistent_xyz" "$b" "$h"
checkshape "builtin bare-redirect capture: cd capture is non-empty" "$h" '.'

h=$("$HUCK_BIN" -c 'x=$(type -Z 2>&1); echo "$x"')
checkshape "builtin bare-redirect capture: type -Z capture is non-empty" "$h" '.'
checkshape "builtin bare-redirect capture: type -Z carries the message body" "$h" 'invalid option'

# ---------------------------------------------------------------------------
# 8. Capture matrix (v269 T4fix): the durable regression gate for the
#    executor.rs misconversion (commit f03e8b5) — 84 sites that held a LOCAL
#    redirect-aware writer (`err_writer(err_sink, sink)`) were switched to the
#    thread-local `sh_error!`, which does not see an inner per-command
#    redirect (`$(cmd 2>&1)`) at all.
#
#    IMPORTANT (verified against the bash oracle, not assumed): a trailing
#    `2>&1` on the SAME simple command does NOT make every one of its own
#    diagnostics land in the `$(...)` capture buffer, even in real bash —
#    expansion-time errors (arith `$((1/0))`), redirect-processing errors
#    (a bad fd, an ambiguous redirect), and the readonly-reassignment check
#    all fire before that command's own redirect list has taken effect, so
#    bash itself leaks them to the real fd 2, empty-handed on the capture
#    side. Only command-resolution failures (command-not-found) and builtin
#    errors (cd, tested in §7) are captured. So the gate compares huck
#    against bash on BOTH channels — the `$(...)` capture AND the real-fd
#    leak — for each fragment, rather than assuming "must always capture".
#    This is what actually catches the misconversion: a misrouted site
#    leaks on huck while bash captures (or vice versa), which shows up as a
#    channel/content mismatch between the two here.
# ---------------------------------------------------------------------------

# checkcapture: run `frag` as `x=$(frag 2>&1); printf '%s' "$x"` under both
# shells, ALSO capturing each side's real stderr (the "leak" channel). Passes
# when the two sides route the diagnostic to the SAME channel (both capture,
# or both leak) with matching content (tail after the leading "<name>: "
# field, normalized).
checkcapture() {
    local label="$1" frag="$2"
    local bleakfile hleakfile
    bleakfile=$(mktemp "${TMPDIR:-/tmp}/huck-errmsg-leak.XXXXXX")
    hleakfile=$(mktemp "${TMPDIR:-/tmp}/huck-errmsg-leak.XXXXXX")
    local b_cap h_cap
    b_cap=$("$BASH_BIN" -c "x=\$($frag 2>&1); printf '%s' \"\$x\"" 2>"$bleakfile")
    h_cap=$("$HUCK_BIN" -c "x=\$($frag 2>&1); printf '%s' \"\$x\"" 2>"$hleakfile")
    local b_leak h_leak
    b_leak=$(cat "$bleakfile")
    h_leak=$(normalize "$(cat "$hleakfile")")
    rm -f "$bleakfile" "$hleakfile"
    local b_cap_tail="${b_cap#*: }" h_cap_tail="${h_cap#*: }"
    local b_leak_tail="${b_leak#*: }" h_leak_tail="${h_leak#*: }"
    if [[ "$b_cap_tail" == "$h_cap_tail" && "$b_leak_tail" == "$h_leak_tail" && -n "$b_cap$b_leak" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        printf '    bash  capture=[%s] leak=[%s]\n' "$b_cap" "$b_leak"
        printf '    huck  capture=[%s] leak=[%s]\n' "$h_cap" "$h_leak"
        FAIL=$((FAIL+1))
    fi
}

checkcapture "capture matrix: command not found" "nosuchcmd_xyz"
checkcapture "capture matrix: cd nonexistent" "cd /nonexistent_capture_xyz"
checkcapture "capture matrix: arith division by zero" "echo \$((1/0))"
checkcapture "capture matrix: bad-fd redirect" "echo x >&9"
checkcapture "capture matrix: readonly reassignment" "readonly r=1; r=2"

# Ambiguous redirect: bash's own body includes the offending word (`$y:
# ambiguous redirect`) while huck's omits it (a pre-existing, unrelated
# message-wording gap — not a routing regression). Assert routing parity
# (both sides leak to the real fd, capture stays empty) and that huck's own
# leaked body carries "ambiguous redirect", without requiring a byte-exact
# match against bash's wording.
b_cap=$("$BASH_BIN" -c 'y="a b"; x=$(echo x > $y 2>&1); printf "%s" "$x"' 2>/dev/null)
h_cap=$("$HUCK_BIN" -c 'y="a b"; x=$(echo x > $y 2>&1); printf "%s" "$x"' 2>/dev/null)
h_leak=$("$HUCK_BIN" -c 'y="a b"; x=$(echo x > $y 2>&1); printf "%s" "$x"' 2>&1 >/dev/null)
checkshape "capture matrix: ambiguous redirect — capture stays empty (bash) too" "$b_cap" '^$'
checkshape "capture matrix: ambiguous redirect — capture stays empty (huck)" "$h_cap" '^$'
checkshape "capture matrix: ambiguous redirect — huck leaks to real fd 2" "$h_leak" 'ambiguous redirect'

# type -Z: bash's own usage-dump second line is out of scope (message-body
# wording is a non-goal elsewhere in this harness too) — assert non-empty
# capture and that the shared "invalid option" wording matches instead of a
# full-body diff.
b=$("$BASH_BIN" -c 'x=$(type -Z 2>&1); printf "%s" "$x"')
h=$("$HUCK_BIN" -c 'x=$(type -Z 2>&1); printf "%s" "$x"')
checkshape "capture matrix: type -Z capture is non-empty" "$h" '.'
checkshape "capture matrix: type -Z body matches bash wording" "$h" 'invalid option'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
