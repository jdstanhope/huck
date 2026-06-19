#!/usr/bin/env bash
# Run the huck runtime sweep in the sandbox container and copy out the results.
#   sg docker -c 'bash tools/runsweep/run.sh [LIMIT]'
# LIMIT>0 → only the first LIMIT corpus scripts (smoke test); 0/absent → all.
#
# Isolation: no network, read-only rootfs, a small writable tmpfs at /work and
# /tmp, non-root user, memory/pid/cpu caps. NO host filesystem is mounted, so a
# corpus script cannot see or touch the host. Results are streamed to stdout and
# the per-script TSV is copied out via `docker cp`.
set -eu
cd "$(git rev-parse --show-toplevel)"
LIM=${1:-0}
OUT=tools/run_results.tsv   # gitignored, like parse_results.tsv

# The harness writes TSV rows to stdout (captured here) and the bucket summary
# to stderr (shown live). No host mount: corpus scripts cannot touch the host.
docker run --rm \
    --network none \
    --read-only \
    --tmpfs /work:rw,nosuid,nodev,size=512m \
    --tmpfs /tmp:rw,nosuid,nodev,size=512m \
    --memory=1g --memory-swap=1g --pids-limit=512 --cpus=1 \
    --security-opt no-new-privileges \
    huck-runsweep "$LIM" > "$OUT"

echo "results -> $OUT ($(wc -l < "$OUT") rows)"
