#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v327 (#272): DEBUG/RETURN traps are
# inherited into a shell function or sourced script only under `set -T` /
# `set -o functrace` (or the equivalent `shopt -s extdebug`, which bash's
# shopt_set_debug_mode() internally couples to the same function_trace_mode
# flag) -- at the top level they always fire, functrace or not.
#
# The DEBUG cases use a COUNTER + boolean-comparison technique (rather than
# literal `echo D` repetition-counting) deliberately: real bash fires an
# additional DEBUG "before the first command executes in a shell function"
# under functrace (documented in bash(1)'s trap description) that huck does
# not implement (a separate, pre-existing firing-point gap, out of scope for
# this gate -- see the v327 task report). Counting literal "D" occurrences
# would make these checks depend on that unrelated gap. Instead each function
# body snapshots a shared counter before and after its own statements; the
# printed boolean (`fired=yes`/`fired=no`) reflects only whether DEBUG fired
# for the STATEMENTS inside the subroutine, independent of the exact count.
# The DEBUG action itself is a plain inline assignment (NOT a function call)
# to sidestep a separate, pre-existing bug: a function called FROM WITHIN a
# trap action's own execution unconditionally fires RETURN at its own return
# (call_function's fire_return_trap call doesn't check whether a trap is
# already in flight) -- also out of scope for this gate (see report).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "return no -T"         'f(){ echo body; }; trap "echo RET" RETURN; f'
check "return -T"            'f(){ echo body; }; trap "echo RET" RETURN; set -T; f'
check "return -T then off"   'f(){ echo b; }; trap "echo R" RETURN; set -T; f; set +T; f'
check "top-level unaffected" 'trap "echo D" DEBUG; echo one; echo two'

check "debug in fn no -T" \
  'n=0; trap "n=$((n+1))" DEBUG; f(){ b=$n; echo body; a=$n; }; f; echo "fired=$([[ $a -gt $b ]] && echo yes || echo no)"'
check "debug in fn -T" \
  'n=0; trap "n=$((n+1))" DEBUG; set -T; f(){ b=$n; echo body; a=$n; }; f; echo "fired=$([[ $a -gt $b ]] && echo yes || echo no)"'

# --- extdebug alone (no -T) also inherits DEBUG into a function -- bash's
# shopt_set_debug_mode() sets function_trace_mode = debugging_mode as a side
# effect of `shopt -s extdebug`.
check "extdebug alone inherits DEBUG" \
  'n=0; trap "n=$((n+1))" DEBUG; shopt -s extdebug; f(){ b=$n; echo body; a=$n; }; f; echo "fired=$([[ $a -gt $b ]] && echo yes || echo no)"'
check "extdebug alone inherits RETURN" 'f(){ echo body; }; trap "echo RET" RETURN; shopt -s extdebug; f'

# --- toggle -T on/off between calls: RETURN literal (single event, safe);
# DEBUG via the counter/boolean technique (immune to the unrelated
# function-entry-extra-fire gap noted above).
check "toggle T between (RETURN)" \
  'f(){ echo b; }; trap "echo R" RETURN; set -T; f; set +T; f; set -T; f'
check "toggle T between (DEBUG)" \
  'n=0; trap "n=$((n+1))" DEBUG; f(){ b=$n; echo body; a=$n; }; set -T; f; d1=$([[ $a -gt $b ]] && echo yes || echo no); set +T; f; d2=$([[ $a -gt $b ]] && echo yes || echo no); set -T; f; d3=$([[ $a -gt $b ]] && echo yes || echo no); echo "d1=$d1 d2=$d2 d3=$d3"'

# --- sourced-file case: DEBUG fires inside a sourced script only under -T ---
SRC_TMPDIR=$(mktemp -d)
cleanup_src_tmpdir() { rm -rf "$SRC_TMPDIR"; }
trap cleanup_src_tmpdir EXIT
printf 'b=$n\necho insrc\na=$n\n' > "$SRC_TMPDIR/src.sh"

check_file() {
    local label="$1" frag="$2" f b h
    f="$SRC_TMPDIR/frag_$$_${PASS}_${FAIL}_$RANDOM.sh"
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc --noprofile "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s (file)\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (file)\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

check_file "debug in sourced file no -T" \
    "n=0; trap 'n=\$((n+1))' DEBUG; . $SRC_TMPDIR/src.sh; echo \"fired=\$([[ \$a -gt \$b ]] && echo yes || echo no)\""
check_file "debug in sourced file -T" \
    "n=0; trap 'n=\$((n+1))' DEBUG; set -T; . $SRC_TMPDIR/src.sh; echo \"fired=\$([[ \$a -gt \$b ]] && echo yes || echo no)\""

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
