#!/usr/bin/env bash
# Differential audit: BACKGROUND pipeline-stage redirects, bash 5.2.21 vs huck.
# A bare `pipeline &` detaches; we observe via result files, polling each until
# its size is stable (job finished) or a per-case timeout elapses. Usage:
#   HUCK=./target/debug/huck bash tools/bg_pipeline_redirect_audit.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
HUCK="${HUCK:-$HERE/../target/debug/huck}"
source "$HERE/bg_pipeline_redirect_audit_cases.sh"

# Poll the given files until their combined byte-size is unchanged for 3
# consecutive 50ms reads (job settled) or ~5s elapses. A minimum 200ms floor
# gives a slow-starting detached job time to write before we read "empty".
poll_settle() {
  local files="$1" elapsed=0 stable=0 prev="" sig f sz
  while [ "$elapsed" -lt 5000 ]; do
    sig=""
    for f in $files; do
      sz=$(wc -c < "$f" 2>/dev/null || echo -1); sig="$sig,$sz"
    done
    if [ "$sig" = "$prev" ]; then stable=$((stable+1)); else stable=0; fi
    prev="$sig"
    if [ "$stable" -ge 3 ] && [ "$elapsed" -ge 200 ]; then return; fi
    sleep 0.05; elapsed=$((elapsed+50))
  done
}

# Run one bare-bg fragment through $1 in a fresh temp dir; echo each result
# file's contents labeled, after polling for completion. Normalize the shell's
# own path prefix so only message text is compared.
run_one_bg() {
  local shell="$1" files="$2" frag="$3" d out f
  d="$(mktemp -d)"
  # </dev/null: without an explicit stdin, the spawned shell (and, for
  # no-redirect stage-0 fragments like "stage0 nodir", its pipeline's first
  # external stage) inherits OUR stdin — the `emit_bg_cases` process
  # substitution feeding the case-list `while read` loop below. A backgrounded
  # stage-0 reader racing that pipe intermittently steals case lines, so the
  # loop silently runs fewer than 9 cases (observed 7, then 0, nondeterministically).
  # Severing stdin here makes every case deterministic and independent of the
  # runner's own plumbing.
  ( cd "$d" && timeout 10 "$shell" -c "$frag" </dev/null ) >/dev/null 2>&1
  ( cd "$d" && poll_settle "$files" )
  out=""
  for f in $files; do
    out="$out==$f==
$(cd "$d" && cat "$f" 2>/dev/null)
"
  done
  rm -rf "$d"
  printf '%s' "$out" | sed -e "s#^$shell:#sh:#" -e "s#^${HUCK}:#sh:#" -e 's#^bash:#sh:#'
}

n=0; agree=0; diverge=0
while IFS=$'\t' read -r label files frag; do
  [ -z "${label:-}" ] && continue
  n=$((n+1))
  b="$(run_one_bg bash "$files" "$frag")"
  h="$(run_one_bg "$HUCK" "$files" "$frag")"
  if [ "$b" = "$h" ]; then agree=$((agree+1)); else diverge=$((diverge+1)); printf 'DIVERGE: %s\n' "$label"; fi
done < <(emit_bg_cases)

echo "=================================================================="
echo "AUDIT: $n cases, $agree agree, $diverge DIVERGE"
echo "=================================================================="
[ "$diverge" -eq 0 ]
