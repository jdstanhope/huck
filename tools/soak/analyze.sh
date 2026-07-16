#!/usr/bin/env bash
# Analyze a soak samples.csv for resource leaks. Standalone: run it on a
# still-growing CSV mid-run, or on a finished one.
#
# Usage: analyze.sh <samples.csv>
#
# Leak model: instantaneous resource counts are NOISY — a sample taken mid-batch
# catches transient capture/procsub pipes and an in-flight background child. A
# real leak is the RESTING FLOOR drifting up over time, so we split the run into
# an early half and a late half and compare their MINIMUMS (the resting level).
# A rising floor on an integer counter = leak; transient spikes are ignored. RSS
# gets a per-hour slope (early median vs late median) with a noise band.
set -u

CSV=${1:?usage: analyze.sh <samples.csv>}
[ -r "$CSV" ] || { echo "analyze: cannot read $CSV" >&2; exit 2; }

# Floor-rise tolerance for the bounded integer counters (late-min - early-min).
TOL_FDS=2
TOL_THREADS=1
TOL_CHILDREN=1
TOL_ZOMBIES=0
TOL_DELETED=0
TOL_TMPFILES=1
# RSS: a healthy steady state does not drift at all (a leak-free run is often
# byte-identical across samples). Flag a sustained early->late median RISE that
# clears BOTH an absolute floor (noise / one-time cache fill) AND a relative
# fraction of the early median — regardless of the absolute per-hour rate, since
# even a slow leak (well under any KB/hr threshold) is a real unbounded leak.
TOL_RSS_RISE_KB=3072      # ignore rises smaller than this (allocator noise / one-time)
TOL_RSS_RISE_FRAC=0.10    # ...and require the rise to exceed 10% of the early median

awk -F, -v tol_fds="$TOL_FDS" -v tol_threads="$TOL_THREADS" \
        -v tol_children="$TOL_CHILDREN" -v tol_zombies="$TOL_ZOMBIES" \
        -v tol_deleted="$TOL_DELETED" -v tol_tmp="$TOL_TMPFILES" \
        -v rss_rise="$TOL_RSS_RISE_KB" -v rss_frac="$TOL_RSS_RISE_FRAC" '
function median(arr, m,   a,b,key) {
  for (a=2;a<=m;a++){ key=arr[a]; b=a-1; while(b>=1 && arr[b]>key){arr[b+1]=arr[b];b--}; arr[b+1]=key }
  return (m%2) ? arr[int(m/2)+1] : (arr[m/2]+arr[m/2+1])/2
}
NR==1 { for (i=1;i<=NF;i++) col[$i]=i; next }
{ for (c=1;c<=NF;c++) row[NR-1,c]=$c; n++ }
END {
  if (n==0) { print "analyze: no data rows"; exit 2 }
  t0=row[1,col["elapsed_s"]]; tN=row[n,col["elapsed_s"]]
  i0=row[1,col["iter"]];      iN=row[n,col["iter"]]
  hours=(tN-t0)/3600.0
  half=(n>=2)?int(n/2):1     # rows 1..half early, half+1..n late

  printf "Samples: %d   Span: %.3f h   Iterations: %d -> %d (delta %d)\n", n, hours, i0, iN, iN-i0
  if (iN==i0 && n>1) print "  WARNING: iteration counter did not advance -- possible hang or dead workload."
  printf "\n%-10s %9s %9s %8s %8s %9s %11s\n", \
     "counter","early-min","late-min","floorD","overmax","late-med","verdict"

  ncnt=split("fds threads children zombies tmpfiles deleted rss_kb", order, " ")
  verdict="PASS"; reason=""
  for (oi=1; oi<=ncnt; oi++) {
    k=order[oi]; if (!(k in col)) continue
    ci=col[k]
    emin=1e18; lmin=1e18; omax=-1e18; ec=0; lc=0
    delete ev; delete lv
    for (r=1;r<=n;r++) {
      v=row[r,ci]+0
      if (v>omax) omax=v
      if (r<=half) { if (v<emin) emin=v; ev[++ec]=v }
      else         { if (v<lmin) lmin=v; lv[++lc]=v }
    }
    if (lc==0) { lmin=emin; lv[1]=emin; lc=1 }
    emed=median(ev, ec); lmed=median(lv, lc)
    floord=lmin-emin

    if (k=="rss_kb") {
      rise=lmed-emed
      perhr=(hours>0)?rise/hours:0
      cell=sprintf("%+.0fKB(%+.0f/hr)", rise, perhr)
      if (rise>rss_rise && rise>rss_frac*emed) {
        verdict="LEAK-SUSPECTED"; reason=reason sprintf(" rss+%.0fKB", rise); cell=cell " LEAK"
      }
      printf "%-10s %9d %9d %8d %8d %9.0f %18s\n", k, emin, lmin, floord, omax, lmed, cell
      continue
    }
    tol=-1
    if (k=="fds") tol=tol_fds; else if (k=="threads") tol=tol_threads
    else if (k=="children") tol=tol_children; else if (k=="zombies") tol=tol_zombies
    else if (k=="tmpfiles") tol=tol_tmp; else if (k=="deleted") tol=tol_deleted
    cell="ok"
    if (tol>=0 && floord>tol) { verdict="LEAK-SUSPECTED"; reason=reason sprintf(" %s+%d",k,floord); cell="LEAK" }
    printf "%-10s %9d %9d %8d %8d %9.0f %11s\n", k, emin, lmin, floord, omax, lmed, cell
  }
  printf "\nVerdict: %s%s\n", verdict, (reason==""?"":"  (floor rise:" reason " )")
  print  "(early-min/late-min = resting floor per half; a rising floor is the leak signal;"
  print  " overmax includes normal transient in-batch spikes and is expected to exceed the floor.)"
}
' "$CSV"
