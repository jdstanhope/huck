#!/usr/bin/env bash
# Differential audit: pipeline-stage redirects, bash 5.2.21 vs huck.
# Usage: HUCK=./target/debug/huck bash tools/pipeline_redirect_audit.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
HUCK="${HUCK:-$HERE/../target/debug/huck}"
source "$HERE/pipeline_redirect_audit_cases.sh"

# Run one fragment through $1 (a shell) in a fresh temp dir; echo combined
# stdout+stderr with the shell's own path prefix normalized to "sh:".
run_one() {
  local shell="$1" frag="$2" d out
  d="$(mktemp -d)"
  out="$( cd "$d" && timeout 10 "$shell" -c "$frag" 2>&1 )"
  local rc=$?
  rm -rf "$d"
  # Normalize the leading program path (bash prints "bash:", huck prints its
  # binary path) so only the message text is compared; mark a timeout.
  if [ $rc -eq 124 ]; then printf 'TIMEOUT\n'; return; fi
  printf '%s\n' "$out" | sed -e "s#^$shell:#sh:#" -e "s#^${HUCK}:#sh:#" -e 's#^bash:#sh:#'
}

n=0; agree=0; diverge=0
while IFS=$'\t' read -r label frag; do
  [ -z "${label:-}" ] && continue
  n=$((n+1))
  b="$(run_one bash "$frag")"
  h="$(run_one "$HUCK" "$frag")"
  if [ "$b" = "$h" ]; then
    agree=$((agree+1))
  else
    diverge=$((diverge+1))
    printf 'DIVERGE: %s\n' "$label"
  fi
done < <(emit_cases)

echo "=================================================================="
echo "AUDIT: $n cases, $agree agree, $diverge DIVERGE"
echo "=================================================================="
[ "$diverge" -eq 0 ]
