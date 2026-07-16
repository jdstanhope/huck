# huck soak / longevity harness

Runs **one long-lived `huck` process** through an infinite, self-verifying
workload while sampling cheap `/proc` resource counters over time, to surface
**resource leaks** (file descriptors, threads, unreaped children/zombies, temp
files, RSS creep) and **behavioral drift** (an assertion that starts failing
after millions of iterations) during runs of hours to days.

Why one long-lived process rather than repeated `huck -c '…'`: every interesting
leak lives *inside the running interpreter* — the fd table, the job table, the
`live_external_children` pid registry, allocator creep, unjoined timer threads.
Re-spawning huck resets all of that each time, so only a single long-lived
process exposes them.

## Files

| file | role |
|------|------|
| `workload.huck` | the infinite loop; each batch exercises pipelines, command/process substitution, small **and >64KB** heredocs, background jobs + `wait`, subshells, fd>2 / `2>&1` / `2>file` redirects, spawn-failure diagnostics (127/126), functions/locals, arrays, and traps — and **asserts its own output** (a mismatch prints `SOAK-FAIL:` and exits 1). Assertions encode huck's *current* behavior, so any drift is caught. |
| `run_soak.sh` | orchestrator: launches the workload, samples counters to `samples.csv`, writes a `tail -f`-able `rollup.log`, and runs the final analysis. Signal-clean, tears down the whole process tree on exit. |
| `analyze.sh` | leak analysis of a `samples.csv` (standalone; run it mid-flight too). |
| `runs/<stamp>/` | per-run output (git-ignored). |

## Run it (detached, for days)

```sh
cargo build --release -p huck          # analyze/run use target/release/huck

# fire-and-forget for 3 days:
nohup tools/soak/run_soak.sh --duration 3d >/dev/null 2>&1 &

# watch it live:
tail -f tools/soak/runs/<stamp>/rollup.log

# stop early (also runs the final analysis):
kill -TERM "$(cat tools/soak/runs/<stamp>/soak.pid)"
```

Options: `--duration D` (`30m`/`12h`/`3d`/seconds; `0`/omitted = until signalled),
`--interval S` (sample period, default 30s), `--warmup S` (settle before the
baseline, default 120s), `--sleep S` (per-batch throttle, default 0), `--huck
PATH`, `--out DIR`.

At ~8 batches/sec (each ~15 process forks) a day is ~700k batches / ~10M forks
through the exec/reap/fd paths — the load stays light because most of the time is
spent waiting on children.

## Reading the result

Instantaneous counts are **noisy** — a sample taken mid-batch catches transient
capture/procsub pipes and an in-flight background child. A real leak is the
**resting floor drifting up**, so `analyze.sh` splits the run in half and
compares the **minimum** of each half (`early-min` vs `late-min`); a rising floor
on an integer counter is the leak signal. `overmax` includes the normal transient
spikes and is expected to exceed the floor. RSS is judged on early-vs-late median
slope with a noise band.

```
counter    early-min  late-min   floorD  overmax  late-med     verdict
fds                3         3        0        8         3          ok      <- floor flat, spikes to 8 are normal
zombies            0         0        0        1         0          ok      <- one in-flight bg child, reaped
...
Verdict: PASS
```

`Verdict: LEAK-SUSPECTED (floor rise: fds+3 …)` means a counter's resting floor
climbed past tolerance — investigate. A `SOAK-FAIL` in `workload.log`, or a
`HANG SUSPECTED` / `huck exited unexpectedly` line in `rollup.log`, means the
workload asserted wrong output, stalled, or crashed.

## Notes

- Two divergences were found just by building this: [#175](https://github.com/jdstanhope/huck/issues/175)
  (non-interactive job table never pruned — the workload works around it with a
  per-batch `disown -a`) and [#176](https://github.com/jdstanhope/huck/issues/176)
  (in-process group stderr mis-ordered/leaked under `2>&1` in a comsub — the
  workload uses an external-command variant instead).
- Linux-only (reads `/proc`). huck's compat target is `ubuntu-24.04`.
