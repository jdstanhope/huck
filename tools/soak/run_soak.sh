#!/usr/bin/env bash
# huck longevity / soak harness — long-lived single-process mode.
#
# Launches ONE huck process running tools/soak/workload.huck in an infinite
# self-verifying loop, then samples cheap /proc resource counters over time to a
# CSV. Built to run detached for days: signal-clean shutdown, timestamped run
# directory, a tail-followable rollup log, and a final leak analysis.
#
# Usage:
#   tools/soak/run_soak.sh [options]
#     --duration D    run for D then stop+analyze (e.g. 30m, 12h, 3d, 3600;
#                     0 or omitted = run until Ctrl-C / SIGTERM)
#     --interval S    sample period in seconds (default 30)
#     --warmup S      seconds to let the process settle before the baseline
#                     sample (default 120)
#     --sleep S       per-batch throttle passed to the workload (default 0)
#     --huck PATH     huck binary (default target/release/huck)
#     --out DIR       base directory for run dirs (default tools/soak/runs)
#
# Detached for days, e.g.:
#   nohup tools/soak/run_soak.sh --duration 3d >/dev/null 2>&1 &
#   tail -f tools/soak/runs/<stamp>/rollup.log      # watch progress
#   kill -TERM <pid-in tools/soak/runs/<stamp>/soak.pid>   # stop early + analyze
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

DURATION=0
INTERVAL=30
WARMUP=120
SLEEP=0
HUCK="$ROOT/target/release/huck"
OUTBASE="$HERE/runs"

to_secs() { # accepts NNN, NNs, NNm, NNh, NNd
  local v=$1 n=${1%[smhd]} u=${1: -1}
  case "$v" in
    *[!0-9smhd]*) echo "bad duration: $v" >&2; exit 2 ;;
  esac
  case "$u" in
    s) echo "$n" ;; m) echo $((n*60)) ;; h) echo $((n*3600)) ;; d) echo $((n*86400)) ;;
    *) echo "$v" ;;   # bare number = seconds
  esac
}

while [ $# -gt 0 ]; do
  case "$1" in
    --duration) DURATION=$(to_secs "$2"); shift 2 ;;
    --interval) INTERVAL=$2; shift 2 ;;
    --warmup)   WARMUP=$2; shift 2 ;;
    --sleep)    SLEEP=$2; shift 2 ;;
    --huck)     HUCK=$2; shift 2 ;;
    --out)      OUTBASE=$2; shift 2 ;;
    -h|--help)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

[ -x "$HUCK" ] || { echo "huck binary not found/executable: $HUCK (build: cargo build --release -p huck)" >&2; exit 2; }

STAMP=$(date +%Y%m%d-%H%M%S)
RUN="$OUTBASE/$STAMP"
mkdir -p "$RUN"
CSV="$RUN/samples.csv"
ROLLUP="$RUN/rollup.log"
WLOG="$RUN/workload.log"

# Record provenance.
{
  echo "start:     $(date -Is)"
  echo "huck:      $HUCK"
  echo "version:   $("$HUCK" --version 2>/dev/null | head -1)"
  echo "commit:    $(cd "$ROOT" && git rev-parse --short HEAD 2>/dev/null) $(cd "$ROOT" && git rev-parse --abbrev-ref HEAD 2>/dev/null)"
  echo "duration:  ${DURATION}s (0=until signalled)"
  echo "interval:  ${INTERVAL}s   warmup: ${WARMUP}s   batch-sleep: ${SLEEP}s"
  echo "host:      $(uname -a)"
} > "$RUN/meta.txt"

echo "soak run: $RUN"
echo "$$" > "$RUN/soak.pid"   # this orchestrator's pid (send SIGTERM here to stop)

# Copy the workload into the run dir and execute the copy, so a days-long
# detached run is immune to the working tree changing under it (branch switch,
# edit) mid-flight.
cp "$HERE/workload.huck" "$RUN/workload.huck"

# Launch the workload as its own session/pgroup leader so we can tear the whole
# process tree (background sleeps, procsub children) down on exit.
SOAK_DIR="$RUN" SOAK_SLEEP="$SLEEP" SOAK_HEARTBEAT=25 \
  setsid "$HUCK" "$RUN/workload.huck" >"$WLOG" 2>&1 &
HPID=$!
echo "$HPID" > "$RUN/huck.pid"
echo "huck pid:  $HPID   (workload log: $WLOG)"

FINALIZED=0
finalize() {
  [ "$FINALIZED" = 1 ] && return
  FINALIZED=1
  # Tear down the workload process group (negative pid = the whole group).
  kill -TERM -"$HPID" 2>/dev/null
  sleep 1
  kill -KILL -"$HPID" 2>/dev/null
  echo "" | tee -a "$ROLLUP"
  echo "=== soak finished $(date -Is) ===" | tee -a "$ROLLUP"
  if [ -s "$CSV" ]; then
    "$HERE/analyze.sh" "$CSV" | tee -a "$ROLLUP"
  else
    echo "no samples collected (did the process die during warmup? see $WLOG)" | tee -a "$ROLLUP"
  fi
  # Surface a workload assertion failure if one happened.
  if grep -q '^SOAK-FAIL' "$WLOG" 2>/dev/null; then
    echo "" | tee -a "$ROLLUP"
    echo "!!! WORKLOAD ASSERTION FAILED (behavioral drift/corruption):" | tee -a "$ROLLUP"
    grep '^SOAK-FAIL' "$WLOG" | tail -5 | tee -a "$ROLLUP"
  fi
}
trap 'finalize; exit 0' INT TERM
trap 'finalize' EXIT

# --- sampling helpers (Linux /proc) --------------------------------------
count_fds()     { ls "/proc/$1/fd"   2>/dev/null | wc -l; }
count_threads() { ls "/proc/$1/task" 2>/dev/null | wc -l; }
rss_kb()        { awk '/^VmRSS:/{print $2}' "/proc/$1/status" 2>/dev/null; }
count_deleted() { # fds pointing at unlinked files — a strong leak signal
  local d=0 l t
  for l in "/proc/$1/fd/"*; do
    t=$(readlink "$l" 2>/dev/null) || continue
    case "$t" in *"(deleted)") d=$((d+1)) ;; esac
  done
  echo "$d"
}
count_tmpfiles() { # fds whose target is a regular file under a temp dir
  local c=0 l t
  for l in "/proc/$1/fd/"*; do
    t=$(readlink "$l" 2>/dev/null) || continue
    case "$t" in /tmp/*|"$RUN"/*) c=$((c+1)) ;; esac
  done
  echo "$c"
}
# children (ppid==HPID) and of those, how many are zombies. One scan of /proc.
count_children() { # sets globals CH and ZOMB
  local pid=$1 c=0 z=0 s rest state ppid
  for s in /proc/[0-9]*/stat; do
    # A transient child can vanish between the glob and the read; suppress the
    # redirection error (must wrap the whole redirection, not just `read`).
    { read -r line < "$s"; } 2>/dev/null || continue
    rest=${line#*) }              # strip "pid (comm) " — comm may contain spaces/)
    state=${rest%% *}             # field: state
    ppid=${rest#* }; ppid=${ppid%% *}   # field: ppid
    if [ "$ppid" = "$pid" ]; then
      c=$((c+1))
      [ "$state" = "Z" ] && z=$((z+1))
    fi
  done
  CH=$c; ZOMB=$z
}

echo "elapsed_s,wall,iter,fds,threads,children,zombies,tmpfiles,deleted,rss_kb" > "$CSV"

START=$(date +%s)
LAST_ITER=-1
LAST_ITER_CHANGE=$START
STALL_LIMIT=$(( INTERVAL*6 > 300 ? INTERVAL*6 : 300 ))
nsample=0

while :; do
  now=$(date +%s)
  elapsed=$((now - START))

  # Duration reached?
  if [ "$DURATION" -gt 0 ] && [ "$elapsed" -ge "$DURATION" ]; then
    echo "duration reached (${elapsed}s)" | tee -a "$ROLLUP"
    break
  fi
  # Process still alive?
  if ! kill -0 "$HPID" 2>/dev/null; then
    echo "!!! huck exited unexpectedly at ${elapsed}s — see $WLOG" | tee -a "$ROLLUP"
    break
  fi

  # Warmup: don't record a baseline until the process has settled.
  if [ "$elapsed" -lt "$WARMUP" ]; then
    sleep "$INTERVAL"; continue
  fi

  iter=$(cat "$RUN/progress" 2>/dev/null); iter=${iter:-0}
  fds=$(count_fds "$HPID")
  threads=$(count_threads "$HPID")
  count_children "$HPID"
  tmp=$(count_tmpfiles "$HPID")
  del=$(count_deleted "$HPID")
  rss=$(rss_kb "$HPID"); rss=${rss:-0}

  echo "$elapsed,$(date +%H:%M:%S),$iter,$fds,$threads,$CH,$ZOMB,$tmp,$del,$rss" >> "$CSV"
  nsample=$((nsample+1))

  # Hang detection: iteration counter must advance.
  if [ "$iter" != "$LAST_ITER" ]; then
    LAST_ITER=$iter; LAST_ITER_CHANGE=$now
  elif [ $((now - LAST_ITER_CHANGE)) -ge "$STALL_LIMIT" ]; then
    echo "!!! HANG SUSPECTED: iter stuck at $iter for $((now-LAST_ITER_CHANGE))s (elapsed ${elapsed}s)" | tee -a "$ROLLUP"
    LAST_ITER_CHANGE=$now   # re-arm so we log periodically, not every sample
  fi

  # Periodic rollup line (every 10 samples) for `tail -f`.
  if [ $((nsample % 10)) -eq 1 ]; then
    printf '[%s] elapsed=%ss iter=%s fds=%s thr=%s child=%s zomb=%s tmp=%s del=%s rss=%sKB\n' \
      "$(date +%H:%M:%S)" "$elapsed" "$iter" "$fds" "$threads" "$CH" "$ZOMB" "$tmp" "$del" "$rss" \
      | tee -a "$ROLLUP"
  fi

  sleep "$INTERVAL"
done

finalize
exit 0
