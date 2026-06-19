#!/usr/bin/env bash
# Runs INSIDE the sandbox container. For each corpus script:
#   1. bash run 1 — if it fails (rc!=0), SKIP_BASH_FAIL (no clean baseline).
#   2. bash run 2 — if output/rc differs from run 1, SKIP_NONDET.
#   3. huck run — compare to the (deterministic, succeeding) bash baseline.
# Each run is in a fresh mktemp workdir under a hard timeout. Output is the
# combined stdout+stderr (capped). The leading shell-name and the script path
# are normalized so only real behavioral differences remain.
#
# Buckets: RUN_AGREE | RUN_HUCK_DIFF | RUN_HUCK_ERROR | SKIP_BASH_FAIL |
#          SKIP_NONDET | MISSING
#
# Usage (inside container): run_sweep_inner.sh [LIMIT]
#   LIMIT > 0 → only the first LIMIT scripts (smoke test); 0/absent → all.
set -u
HUCK=/usr/local/bin/huck
PATHS=/corpus/paths.txt
LIM=${1:-0}
TIMEOUT=8
MAXOUT=200000
# TSV rows go to STDOUT (captured on the host); the bucket summary to STDERR.
# (No persisted file: the container's /work is tmpfs and vanishes on exit.)
emit() { printf '%s\n' "$1"; }

# run a script in a fresh workdir under a timeout; capture combined output
# (truncated to MAXOUT chars) into OUT and the rc into RC. Inlined as a macro-
# style helper would lose RC across the `$(…)` subshell, so callers inline it
# via this `eval`-free pattern: out=$(...); rc=$?; out=${out:0:MAXOUT}.
normalize() {  # $1=text $2=script-path  -> stdout normalized
    local esc
    esc=$(printf '%s' "$2" | sed 's/[][\\.^$*/]/\\&/g')
    printf '%s' "$1" | sed -E "s#^(bash|huck|${HUCK}):#SH:#; s#${esc}#SCRIPT#g"
}

declare -A BUCKETS=()
n=0
while IFS= read -r s; do
    n=$((n + 1))
    [ "$LIM" -gt 0 ] && [ "$n" -gt "$LIM" ] && break
    bucket=""; br1="-"; hr="-"
    if [ ! -f "$s" ]; then
        bucket=MISSING
    else
        w=$(mktemp -d); b1=$(cd "$w" && timeout "$TIMEOUT" bash "$s" </dev/null 2>&1); br1=$?; rm -rf "$w"; b1=${b1:0:MAXOUT}
        if [ "$br1" -ne 0 ]; then
            bucket=SKIP_BASH_FAIL
        else
            w=$(mktemp -d); b2=$(cd "$w" && timeout "$TIMEOUT" bash "$s" </dev/null 2>&1); br2=$?; rm -rf "$w"; b2=${b2:0:MAXOUT}
            if [ "$b1" != "$b2" ] || [ "$br1" != "$br2" ]; then
                bucket=SKIP_NONDET
            else
                w=$(mktemp -d); h=$(cd "$w" && timeout "$TIMEOUT" "$HUCK" "$s" </dev/null 2>&1); hr=$?; rm -rf "$w"; h=${h:0:MAXOUT}
                if [ "$(normalize "$b1" "$s")" = "$(normalize "$h" "$s")" ] && [ "$br1" = "$hr" ]; then
                    bucket=RUN_AGREE
                elif [ "$hr" -ne 0 ] && [ "$br1" -eq 0 ]; then
                    bucket=RUN_HUCK_ERROR
                else
                    bucket=RUN_HUCK_DIFF
                fi
            fi
        fi
    fi
    emit "$(printf '%s\t%s\t%s\t%s' "$s" "$bucket" "$br1" "$hr")"
    BUCKETS[$bucket]=$(( ${BUCKETS[$bucket]:-0} + 1 ))
done < "$PATHS"

{
    echo "=== buckets (of $n scanned) ==="
    for k in "${!BUCKETS[@]}"; do printf '%6d %s\n' "${BUCKETS[$k]}" "$k"; done | sort -rn
} >&2
