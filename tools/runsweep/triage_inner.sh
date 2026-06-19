#!/usr/bin/env bash
# Inside the container: for each script path given as an argument, show the
# normalized bash-vs-huck combined-output diff. Used to triage the
# RUN_HUCK_ERROR / RUN_HUCK_DIFF buckets into real divergences.
#   docker run --rm --network none --read-only --tmpfs /work --tmpfs /tmp \
#     -u sandbox --entrypoint /usr/local/bin/triage_inner.sh huck-runsweep <path>...
set -u
HUCK=/usr/local/bin/huck
TIMEOUT=8
MAXOUT=8000

norm() {  # $1=text $2=script-path
    local esc
    esc=$(printf '%s' "$2" | sed 's/[][\\.^$*/]/\\&/g')
    printf '%s' "$1" | sed -E "s#^(bash|huck|${HUCK}):#SH:#; s#${esc}#SCRIPT#g"
}

for s in "$@"; do
    [ -f "$s" ] || { echo "########## $s   (MISSING)"; echo; continue; }
    w=$(mktemp -d); b=$(cd "$w" && timeout "$TIMEOUT" bash "$s"  </dev/null 2>&1); br=$?; rm -rf "$w"; b=${b:0:MAXOUT}
    w=$(mktemp -d); h=$(cd "$w" && timeout "$TIMEOUT" "$HUCK" "$s" </dev/null 2>&1); hr=$?; rm -rf "$w"; h=${h:0:MAXOUT}
    echo "########## $s   (bash rc=$br, huck rc=$hr)"
    diff <(norm "$b" "$s") <(norm "$h" "$s") | head -30
    echo
done
