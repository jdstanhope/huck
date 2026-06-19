# Runtime sweep — run the script corpus through bash vs huck, safely

Runs the corpus (`../scripts.tsv`, col 3 = abs paths) through **bash** and
**huck** inside an isolated rootful-Docker container and buckets the
differences, to find RUNTIME divergences the parse-only sweep (`../parse_sweep.sh`)
can't. The container has **no network and no host mount** — corpus scripts
cannot see or touch the host.

## One-time setup

Rootful Docker (Ubuntu's `docker.io`), user in the `docker` group. If the group
isn't active in your shell, prefix commands with `sg docker -c '…'`.

## Use

```sh
cargo build --release                                   # huck binary the image bakes in
awk -F'\t' '{print $3}' ../scripts.tsv | while read -r p; do [ -f "$p" ] && [ -r "$p" ] && echo "$p"; done > paths.txt
sg docker -c 'bash tools/runsweep/build.sh'             # build image (corpus + huck baked in)
sg docker -c 'bash tools/runsweep/run.sh'               # full sweep -> tools/run_results.tsv
sg docker -c 'bash tools/runsweep/triage.sh RUN_HUCK_ERROR'   # dump bash-vs-huck diffs
```

`run.sh N` limits to the first N scripts (smoke test).

## How the signal is extracted

Per script: `bash` ×2 (a **determinism gate** — differing runs are `SKIP_NONDET`,
auto-culling timestamps/PIDs/random/tty-needers) → if bash succeeds
deterministically, `huck` ×1 → compare (the leading `bash:`/`huck:` and the
script path are normalized). Buckets: `RUN_AGREE`, `RUN_HUCK_DIFF` (output/rc
differs), `RUN_HUCK_ERROR` (huck non-zero where bash succeeded — highest signal),
`SKIP_BASH_FAIL`, `SKIP_NONDET`, `MISSING`.

`paths.txt`, `corpus.tgz`, `huck`, and `run_results.tsv` are box-specific build
artifacts and are gitignored.
