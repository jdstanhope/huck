#!/usr/bin/env bash
# Triage a bucket from a completed run: show normalized bash-vs-huck diffs for
# every script in the given bucket (default RUN_HUCK_ERROR).
#   sg docker -c 'bash tools/runsweep/triage.sh [BUCKET] [MAX]'
# Reads tools/run_results.tsv. Re-builds the image first if triage_inner.sh is
# newer (run build.sh). Runs the diffs inside the same isolated container.
set -eu
cd "$(git rev-parse --show-toplevel)"
BUCKET=${1:-RUN_HUCK_ERROR}
MAX=${2:-60}
RES=tools/run_results.tsv
[ -s "$RES" ] || { echo "no $RES — run the sweep first" >&2; exit 1; }

mapfile -t paths < <(awk -F'\t' -v b="$BUCKET" '$2==b{print $1}' "$RES" | head -n "$MAX")
[ "${#paths[@]}" -gt 0 ] || { echo "no scripts in bucket $BUCKET"; exit 0; }
echo "triaging ${#paths[@]} script(s) in bucket $BUCKET …" >&2

docker run --rm \
    --network none --read-only \
    --tmpfs /work:rw,nosuid,nodev,size=256m \
    --tmpfs /tmp:rw,nosuid,nodev,size=256m \
    --memory=1g --memory-swap=1g --pids-limit=512 --cpus=1 \
    --security-opt no-new-privileges \
    --entrypoint /usr/local/bin/triage_inner.sh \
    huck-runsweep "${paths[@]}"
