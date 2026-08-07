#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v270: `set` accepts the full bash 5.2
# set of shell options (builtin-completeness). Covers:
#   - every option accepted via `-o NAME` and via its single-char flag
#   - invalid-option-name error wording (`set: <name>: invalid option name`)
#   - `set -o` full-listing byte-match + `set +o` re-inputtable round-trip
#   - braceexpand gate (`set +B` / `+o braceexpand` disables `{a,b}`)
#   - allexport gate (`set -a` auto-exports subsequent assignments)
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Byte-identical stdout+stderr+exit comparison of a script body.
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-seto.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

# Like checkf but normalizes the leading program-name/path prologue on error
# lines: compares from the first `set:` occurrence onward (the prologue differs
# only by the emitting program's argv[0], which is not the behavior under test).
checkerr() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-seto.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    b=$(printf '%s\n' "$b" | sed 's/^.*\(set: \)/\1/')
    h=$(printf '%s\n' "$h" | sed 's/^.*\(set: \)/\1/')
    compare "$label" "$b" "$h"
}

# ── every long-form option accepted (rc 0, silent) ──
for o in allexport braceexpand emacs errtrace functrace hashall histexpand \
         history ignoreeof interactive-comments keyword monitor nolog notify \
         onecmd privileged vi; do
    checkf "-o $o accepted" "set -o $o; echo ok"
    checkf "+o $o accepted" "set +o $o; echo ok"
done

# ── every single-char flag accepted (rc 0, silent) ──
for f in -a -b -h -k -m -t -B -E -H -P -T -p; do
    checkf "$f accepted"  "set $f; echo ok"
    checkf "+${f#-} accepted" "set +${f#-}; echo ok"
done

# ── invalid option name wording + rc ──
checkerr "-o bad name"  "set -o nonexistent; echo after"
checkerr "+o bad name"  "set +o nonexistent; echo after"

# ── full `set -o` listing byte-match, and `set +o` re-inputtable form ──
checkf "set -o full listing" "set -o"
checkf "set +o full listing" "set +o"

# ── braceexpand gate ──
# huck performs `{a,b}` brace expansion at LEX time, so a `set +B` toggle only
# affects lines lexed AFTER it runs — i.e. subsequent REPL/piped-stdin lines,
# not the same batch (a `-c` string or a whole script file are lexed as one
# batch). This matches bash for the line-by-line stdin path; the same-batch
# case (deferred, would need expansion-time brace expansion) is not asserted.
checkf "braceexpand default on" "echo {a,b}"
# Line-by-line via piped stdin: the gate is byte-identical to bash here.
checkstdin() {
    local label="$1" body="$2" b h
    b=$(printf '%s' "$body" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s' "$body" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
checkstdin "set +B disables braces (stdin)" $'set +B\necho {a,b}\n'
checkstdin "+o braceexpand disables (stdin)" $'set +o braceexpand\necho {a,b}\n'
checkstdin "set -B re-enables (stdin)" $'set +B\nset -B\necho {a,b}\n'

# ── allexport gate ──
checkf "set -a auto-exports" $'set -a\nX=1\nexport -p | grep -w X'
checkf "no allexport no export" $'Y=2\nexport -p | grep -w Y'
checkf "set +a stops exporting" $'set -a\nX=1\nset +a\nZ=3\nexport -p | grep -Ew "X|Z"'

harness_summary
