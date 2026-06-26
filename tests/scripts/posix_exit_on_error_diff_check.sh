#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v226: POSIX non-interactive exit-on-error
# (Cluster A). Compares STDOUT + exit code only (NOT stderr — huck's error-message
# prologue differs from bash's `script: line N:`, a separate deferred divergence).
# Each fragment ends in `echo AFTER`; a shell exit suppresses AFTER and yields
# bash's status, while a continue prints AFTER.
#
# Scope notes (intentionally NOT asserted here):
#  - DEFAULT-mode checks are run only for triggers whose default behavior already
#    matches bash. `assign-no-cmd` (L-43 readonly-assign-fatal), `arith-error`
#    (L-55 / v215 intentional), and top-level `return` are pre-existing default-mode
#    divergences OUTSIDE v226's posix-only scope — their default rows are omitted.
#  - `readonly x=1; x=2 true` (assignment error before a REGULAR command) is the
#    deferred "abort-rest-of-line" case: bash aborts the rest of the *line* (so a
#    one-liner shows no AFTER) but continues at the next input line — it is not a
#    shell exit. huck continues. Excluded (see bash-divergences).
#  - `trap -z`, `. -z`, `set -z`/`set -h` single-char: huck lacks bad-option
#    detection there, so it does not exit where bash does. Documented divergences.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Compare (stdout, exit-code) of `bash <flag> -c FRAG` vs `huck <flag> -c FRAG`.
# stderr discarded. $1 label, $2 fragment, $3 extra flag ("" or "--posix").
cmp_run() {
    local label="$1" frag="$2" flag="$3" bo ho br hr
    bo=$(bash $flag -c "$frag" 2>/dev/null); br=$?
    ho=$("$HUCK_BIN" $flag -c "$frag" 2>/dev/null); hr=$?
    if [[ "$bo" == "$ho" && "$br" == "$hr" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash=(%q,%s) huck=(%q,%s)\n' "$label" "$bo" "$br" "$ho" "$hr"; FAIL=$((FAIL+1)); fi
}
check_posix()   { cmp_run "posix:$1"   "$2" "--posix"; }
check_default() { cmp_run "default:$1" "$2" ""; }

# --- the nine triggers: POSIX mode exits with bash's status ---
check_posix "assign-no-cmd"         'readonly x=1; x=2; echo AFTER'
check_posix "assign-before-special" 'readonly x=1; x=2 export y; echo AFTER'
check_posix "readonly-for-var"      'readonly i=1; for i in a b; do :; done; echo AFTER'
check_posix "source-not-found"      '. /no/such/huck_xyz; echo AFTER'
check_posix "fn-name-clash"         'eval(){ :; }; echo AFTER'
check_posix "arith-error"           'echo $(( 1 + )); echo AFTER'
check_posix "special-bad-option"    'set -o nosuchopt; echo AFTER'
check_posix "export-bad-assign"     'export AA[4]=1; echo AFTER'
check_posix "return-outside-fn"     'return 2; echo AFTER'

# --- default mode: only triggers whose default behavior matches bash (gating guard) ---
check_default "source-not-found"      '. /no/such/huck_xyz; echo AFTER'
check_default "fn-name-clash"         'eval(){ :; }; echo AFTER'
check_default "readonly-for-var"      'readonly i=1; for i in a b; do :; done; echo AFTER'
check_default "assign-before-special" 'readonly x=1; x=2 export y; echo AFTER'
check_default "special-bad-option"    'set -o nosuchopt; echo AFTER'
check_default "export-bad-assign"     'export AA[4]=1; echo AFTER'

# --- case #1 boundaries that MUST still continue in POSIX mode ---
check_posix "shift-oor-continues"       'shift 99; echo AFTER'
check_posix "shift-badopt-continues"    'shift -z; echo AFTER'
check_posix "eval-false-continues"      'eval false; echo AFTER'
check_posix "legit-return2-continues"   'f(){ return 2; }; f; echo AFTER'
check_posix "break-continues"           'break; echo AFTER'
check_posix "trap-badsig-continues"     'trap x NOSUCHSIG; echo AFTER'
check_posix "export-badname-continues"  'export "AA[4]"; echo AFTER'
check_posix "set-unimpl-o-continues"    'set -o emacs; echo AFTER'
check_posix "set-unimpl-flag-continues" 'set -h; echo AFTER'
check_posix "command-strips"            'command set -o bad; echo AFTER'
check_posix "builtin-strips"            'builtin set -o bad; echo AFTER'
check_posix "command-strips-assign"     'command export AA[4]=1; echo AFTER'

# --- mechanism edges ---
check_posix "exit-trap-fires"   'trap "echo TRAP" EXIT; . /no/such/huck_xyz; echo AFTER'
check_posix "subshell-isolated" '( . /no/such/huck_xyz; echo INNER ); echo AFTER'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
