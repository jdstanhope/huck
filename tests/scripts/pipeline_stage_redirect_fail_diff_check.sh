#!/usr/bin/env bash
# v296 (#145): a pipeline STAGE's own redirect-setup failure must fail only that
# stage (report the error, exit 1, wired into the pipe topology) and let the rest
# of the pipeline run — matching bash. Compares stdout+stderr+rc+PIPESTATUS.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Run a fragment, appending an rc+PIPESTATUS probe; normalise the shell's own
# error prefix (`bash: line N:` / `<huckpath>: line N:` / `<huckpath>:`) to a
# uniform `SH:` so only the libc message text + rc + PIPESTATUS are compared.
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  h=$(timeout 10 "$HUCK" -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# Structural check: compare rc + stdout + PIPESTATUS only (stderr discarded).
# For cases where huck's ERROR TEXT diverges from bash for an orthogonal reason
# (e.g. the ambiguous-redirect message omits the target word) but the per-stage
# continue-semantics — rc, stdout, PIPESTATUS — must match. See issue #152
# tracking the ambiguous-redirect message gap.
check_ps() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>/dev/null)
  h=$(timeout 10 "$HUCK" -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>/dev/null)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# NOTE ON UPSTREAMS: a stage feeding a FAILED stage must be either silent
# (`true`, deterministically exits 0) or an infinite writer (`yes`,
# deterministically SIGPIPEs 141). A FINITE writer (`echo A`) races the failed
# stage's dummy exit — bash reliably delivers the small write (it prints the
# redirect error IN the failed child, keeping the read end open) while huck's
# instant dummy occasionally SIGPIPEs it. That timing divergence is tracked in
# issue #151; do NOT reintroduce `echo A |` upstreams here until #151 is fixed.
# --- external stage fails at each position ---
check 'ext-middle'   'true | cat </no/such/file | cat'
check 'ext-first'    'cat </no/such/file | wc -c'
check 'ext-last'     'true | cat </no/such/file'
# --- builtin stage fails at each position ---
check 'blt-middle'   'true | read x </no/such/file | cat'
check 'blt-first'    'read x </no/such/file | wc -c'
check 'blt-last'     'true | read x </no/such/file'
# --- compound stage fails (regression guard: already correct) ---
check 'cmp-middle'   'true | { cat; } </no/such/file | cat'
# --- two stages fail (both stages print an error; bash's two failed children
#     race on stderr ORDER, so compare structurally — rc+stdout+PIPESTATUS are
#     deterministic (1 1 0) in both) ---
check_ps 'two-fail'  'cat </no/a | cat </no/b | wc -c'
# --- upstream floods a dead reader -> SIGPIPE 141 (yes never exits, so its
#     141 is deterministic; a `head -N` middle stage would race 0-vs-141) ---
check 'sigpipe-up'   'yes | cat </no/such/file | wc -l'
# --- failed stage redirects stdin AWAY from the pipe; upstream still SIGPIPEs ---
check 'stdin-away'   'yes | read x </no/such/file | cat'
# --- bad-fd source-order (message fixed in v293; only rc/continue diverged) ---
check 'badfd-simple' 'cat <&7 | cat'
check 'badfd-heredoc' "/bin/cat <&3 3<<<'HS' | cat"
# --- in-process ambiguous-redirect (expansion) failure: continue, rc+PIPESTATUS
#     match bash; stderr text diverges orthogonally (see issue #152, the
#     ambiguous-redirect message gap), so compare structurally. ---
check_ps 'amb-stdin-mid'  'true | read x <$(echo a b) | cat'
check_ps 'amb-stdin-last' 'true | read x <$(echo a b)'
check_ps 'amb-stdout-mid' 'true | read x >$(echo a b) | cat'
check_ps 'amb-append-mid' 'true | read x >>$(echo a b) | cat'
check_ps 'amb-stderr-mid' 'true | read x 2>$(echo a b) | cat'
check_ps 'amb-serr-app'   'true | read x 2>>$(echo a b) | cat'
check_ps 'amb-ext-guard'  'true | cat >$(echo a b) | wc -c'   # external (already worked): regression guard

if [ $FAIL -ne 0 ]; then echo "pipeline_stage_redirect_fail_diff_check FAILED" >&2; exit 1; fi
echo "pipeline_stage_redirect_fail_diff_check OK"
