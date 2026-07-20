#!/usr/bin/env bash
# v319 (#222): restricted shell (rbash). Runs each fragment through
# `bash -r -c` and `huck -r -c` (plus a script-file and an argv[0]==rbash
# variant) and asserts byte-identical stdout, stderr AND exit status.
#
# Covers: the guarded ops (cd/exec/`/` in command names/`/` in source paths),
# ALL FIVE testable file-target output redirects, the forms that must stay
# PERMITTED (input `<`, fd-dup `>&2` / `2>&1`, bare command names), every
# variable write path against the readonly-marked SHELL/PATH/HISTFILE/ENV/
# BASH_ENV set, the one-way property (`set +r`, `set -o restricted`,
# `shopt -s/-u restricted_shell`), `shopt restricted_shell`'s PROVENANCE
# reporting, and propagation into functions, subshells and command subs.
#
# NOT covered, each a divergence OUTSIDE #222 filed separately:
#  * `>& file` — huck rejects it as `bad fd` independently of restricted mode
#    (#223). The other five output-redirect operators ARE covered.
#  * `set -r` in `-c` mode applying the readonly marks where bash does not
#    (#229) — so the `set -r` + variable-write case is routed through a SCRIPT
#    FILE (check_script), where the two shells agree. The `set -r` provenance
#    cases below are unaffected and run both ways.
#  * bundled flags (`-rc`) (#230) — always use separated `-r -c`.
#  * `source` with NO operand — huck omits bash's `filename argument required`
#    line and names the builtin `.` in the usage line (#232, pre-existing).
#  * a failed PREFIX assignment (`PATH=/tmp cmd`) wrongly skips the command in
#    huck; bash runs it (#203, pre-existing, not restricted-specific).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=$PWD/target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

# Several fragments are PERMITTED writers (`echo hi > f` is denied, but the
# script-file cases create files). Run entirely inside a disposable dir so a
# sweep from the repo root leaves no droppings, whatever the caller's cwd.
SCRATCH=$(mktemp -d) || exit 1
trap 'rm -rf "$SCRATCH"' EXIT
cd "$SCRATCH" || exit 1
# argv[0]-based entry points: bash and huck each reached under the name `rbash`.
mkdir -p "$SCRATCH/bin-h" "$SCRATCH/bin-b"
ln -s "$HUCK" "$SCRATCH/bin-h/rbash"
ln -s "$(command -v bash)" "$SCRATCH/bin-b/rbash"

FAIL=0
# Normalise each shell's own program-name prefix (`bash: line N:` /
# `<huckpath>: line N:` / `rbash: line N:`) to `SH:` so only the diagnostic
# text is compared. Script-file diagnostics are prefixed with the script's
# name, which is identical for both shells and so is left alone.
norm() { sed -E -e "s#^($SCRATCH/bin-[bh]/rbash|bash|rbash|$HUCK): line [0-9]+: #SH: #" \
                -e "s#^($SCRATCH/bin-[bh]/rbash|bash|rbash|$HUCK): #SH: #"; }

# stdout, stderr and rc are compared SEPARATELY: folding them with 2>&1 would
# compare an interleaving that depends on each shell's stdout buffering.
cmp3() {
  local label=$1 bo=$2 be=$3 br=$4 ho=$5 he=$6 hr=$7
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"
    echo "  bash(rc=$br): out=[$bo] err=[$be]"
    echo "  huck(rc=$hr): out=[$ho] err=[$he]"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# `-r -c FRAG` in both shells.
check() {
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(timeout 10 bash -r -c "$frag" 2>"$SCRATCH/be"); br=$?; be=$(norm <"$SCRATCH/be")
  ho=$(timeout 10 "$HUCK" -r -c "$frag" 2>"$SCRATCH/he"); hr=$?; he=$(norm <"$SCRATCH/he")
  cmp3 "$label" "$bo" "$be" "$br" "$ho" "$he" "$hr"
}

# The fragment as a SCRIPT FILE, run WITHOUT -r (so `set -r` inside it is the
# entry point). Both shells run the same path, so the `<script>: line N:`
# prefix already matches and norm leaves it alone.
check_script() {
  local label=$1; shift
  local f=$SCRATCH/s.sh bo be br ho he hr
  printf '%s\n' "$@" >"$f"
  bo=$(timeout 10 bash "$f" 2>"$SCRATCH/be"); br=$?; be=$(norm <"$SCRATCH/be")
  ho=$(timeout 10 "$HUCK" "$f" 2>"$SCRATCH/he"); hr=$?; he=$(norm <"$SCRATCH/he")
  cmp3 "$label" "$bo" "$be" "$br" "$ho" "$he" "$hr"
}

# Entry point via argv[0] == "rbash" — NO -r flag; the name alone restricts.
check_argv0() {
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(timeout 10 "$SCRATCH/bin-b/rbash" -c "$frag" 2>"$SCRATCH/be"); br=$?; be=$(norm <"$SCRATCH/be")
  ho=$(timeout 10 "$SCRATCH/bin-h/rbash" -c "$frag" 2>"$SCRATCH/he"); hr=$?; he=$(norm <"$SCRATCH/he")
  cmp3 "$label" "$bo" "$be" "$br" "$ho" "$he" "$hr"
}

# --- guarded operations -------------------------------------------------
check 'cd'                  'cd /etc'
check 'cd-then-continue'    'cd /etc; echo after=$?'
check 'exec'                'exec /bin/true'
check 'exec-with-options'   'exec -c /bin/true'
# `exec` WITHOUT a command word is permitted — only the replace-the-shell form
# is refused; its redirections then face the ordinary redirect policy.
check 'exec-bare-ok'        'exec; echo ok=$?'
check 'exec-input-fd-ok'    'exec 3</etc/hostname; echo ok=$?'
check 'exec-dup-ok'         'exec 2>&1; echo ok=$?'
check 'exec-optonly-ok'     'exec -c; echo ok=$?'
# `exec REDIR CMD` — the COMBINATION. bash evaluates the redirections BEFORE
# refusing the exec, so a denied redirect speaks first and `exec: restricted`
# is never reached; a PERMITTED redirect (input `<`, fd-dup) leaves the exec
# refusal to report. Asserting only the two halves separately (above) missed
# this ordering entirely.
check 'exec-redir-cmd-err'  'exec 2>/dev/null true'
check 'exec-redir-cmd-file' 'exec 3> f true'
check 'exec-redir-cmd-out'  'exec > f /bin/echo hi'
check 'exec-redir-cmd-2nd'  'exec 2>/dev/null 3> f true'
check 'exec-inredir-cmd'    'exec 3</etc/hostname true'
# `exec 2>&1 true` omitted — bash APPLIES the permitted redirect (permanently,
# as an `exec` redirect) before refusing, so its own diagnostic lands on the
# REDIRECTED stderr; huck refuses without applying and writes to the original.
# Message text and rc agree; only the destination differs. See #233.
check 'command-abs-path'    '/bin/echo hi'
check 'command-rel-path'    './foo'
check 'command-subdir'      'bin/foo'
check 'dot-with-slash'      '. /etc/profile'
check 'source-with-slash'   'source /etc/profile'

# --- redirects: every file target is denied ------------------------------
check 'redir-truncate'      'echo hi > f'
check 'redir-append'        'echo hi >> f'
check 'redir-clobber'       'echo hi >| f'
check 'redir-readwrite'     'echo hi <> f'
check 'redir-amp-gt'        'echo hi &> f'
check 'redir-numbered'      'echo hi 2> f'
check 'redir-exec-perm'     'exec 3> f'
# `{var}`-fd redirect: bash names the VARIABLE in the diagnostic, not the
# resolved file. Same for an ordinary command, so both forms are pinned.
check 'redir-varfd-exec'    'exec {v}> f'
check 'redir-varfd-cmd'     'echo hi {v}> f'
check 'redir-varfd-input'   'exec {v}</etc/hostname; echo ok=$?'
# `>& f` omitted — #223 (huck: `bad fd`, independent of restricted mode).

# --- redirects: PERMITTED forms — the regression guard -------------------
check 'input-redirect'      'read x < /etc/hostname; echo "$x"'
check 'dup-to-stderr'       'echo hi >&2'
check 'dup-stderr-to-out'   'echo hi 2>&1'
check 'dup-close'           'echo hi >&-'
check 'bare-command-name'   'echo hi'
check 'bare-external'       'true; echo rc=$?'
check 'pipeline-ok'         'echo hi | cat'
check 'heredoc-ok'          'cat <<< hi'

# --- variable write paths (readonly-marked set; each path has its OWN
# --- wording, which is exactly what a single hand-written message would
# --- have got wrong) -----------------------------------------------------
for v in SHELL PATH HISTFILE ENV BASH_ENV; do
  check "assign-$v"         "$v=/tmp"
done
check 'assign-then-cont'    'PATH=/tmp; echo after=$?'
check 'append-assign'       'PATH+=/tmp'
check 'export-assign'       'export PATH=/tmp'
check 'export-bare'         'export PATH'
check 'declare-assign'      'declare PATH=/tmp'
check 'declare-attr'        'declare -x PATH'
check 'unset-var'           'unset PATH'
check 'read-into-var'       'echo x | read PATH'
check 'read-redirect'       'read PATH < /etc/hostname'
# `PATH=/tmp true` (prefix assignment) omitted — #203.
# Controls: an UNPROTECTED variable is untouched by restricted mode.
check 'plain-var-ok'        'FOO=/tmp; echo "$FOO"'
check 'plain-var-export'    'export FOO=/tmp; echo "$FOO"'
check 'plain-var-unset'     'FOO=1; unset FOO; echo "[${FOO-unset}]"'

# --- the one-way property: restriction cannot be lifted ------------------
check 'set-plus-r'          'set +r'
check 'set-plus-r-then-cd'  'set +r; cd /etc'
check 'set-o-restricted'    'set -o restricted'
check 'set-plus-o-restr'    'set +o restricted'
check 'shopt-s'             'shopt -s restricted_shell; echo rc=$?'
check 'shopt-u'             'shopt -u restricted_shell; echo rc=$?'
check 'shopt-u-then-cd'     'shopt -u restricted_shell; cd /etc'

# --- `shopt restricted_shell` reports PROVENANCE, not current state ------
# `on` when restricted at STARTUP (-r or argv[0]), `off` after `set -r` even
# though the shell IS restricted — hence the paired `cd` assertion below.
check 'shopt-query-dash-r'  'shopt restricted_shell'
check 'shopt-query-plus-cd' 'shopt restricted_shell; cd /etc'
check 'set-r-provenance'    'set -r; shopt restricted_shell'
# `off` must NOT mean unrestricted: cd is still refused.
check 'set-r-still-restr'   'set -r; shopt restricted_shell; cd /etc'
check 'set-r-redirect'      'set -r; echo hi > f'
check 'set-r-source'        'set -r; . /etc/profile'
# The variable marks under `set -r` diverge in -c mode (#229) — script only.
check_script 'set-r-var-script'   'set -r' 'PATH=/tmp' 'echo after=$?'
check_script 'set-r-cd-script'    'set -r' 'cd /etc' 'echo after=$?'
check_script 'set-r-shopt-script' 'set -r' 'shopt restricted_shell' 'cd /etc'

# --- entry point via argv[0] == rbash ------------------------------------
check_argv0 'argv0-cd'      'cd /etc'
check_argv0 'argv0-redir'   'echo hi > f'
check_argv0 'argv0-var'     'PATH=/tmp'
check_argv0 'argv0-shopt'   'shopt restricted_shell'
check_argv0 'argv0-permit'  'echo hi; read x < /etc/hostname; echo "$x"'

# --- propagation into nested execution contexts --------------------------
check 'in-function'         'f() { cd /etc; }; f'
check 'in-nested-function'  'g() { cd /etc; }; f() { g; }; f'
check 'in-subshell'         '( cd /etc )'
check 'in-cmdsub'           'echo "[$(cd /etc)]"'
check 'in-pipeline-stage'   'echo x | { cd /etc; }'
check 'in-loop-body'        'for i in 1 2; do cd /etc; done'
check 'in-eval'             'eval "cd /etc"'
check 'redirect-in-func'    'f() { echo hi > f; }; f'
check 'var-in-subshell'     '( PATH=/tmp )'

if [ $FAIL -ne 0 ]; then echo "rbash_diff_check FAILED" >&2; exit 1; fi
echo "rbash_diff_check OK"
