# v214: Bash test-suite integration

## Goal

Run upstream bash's test suite (`bash-5.2.21/tests/`) against huck and
capture the divergences in a committed baseline document. Each
divergence becomes triage fodder for future bug-fix iterations. v214
ships the runner, the operator README, the initial baseline doc, and
a smoke test that pins the runner mechanics — no production-code
changes.

The goal is BUG DISCOVERY against a curated, well-respected, externally
maintained test corpus written by the people who maintain the reference
implementation.

## Background

Huck's existing bash-compat verification is the 131
`tests/scripts/*_diff_check.sh` harnesses — small, hand-curated
fragments that pin specific features (often added during iteration
review). They are byte-identical to bash for the cases they cover.

What they DON'T cover: the 80 `.tests` files in bash's own
`tests/` subdirectory. Each is a long script exercising one feature
area (arith, alias, array, comsub, jobs, etc.) followed by a
`.right` expected-output file. The bash maintainers update these
on every release.

Bash 5.2.21's `tests/` ships:
- 80 `.tests` files (the actual test scripts).
- 84 `.right` files (expected output, one per test plus per-category
  variants).
- ~470 `.sub` auxiliary scripts sourced/exec'd by the `.tests` files.
- ~60 `run-<category>` runner shell scripts of the form:
  `${THIS_SH} ./X.tests > ${BASH_TSTOUT} 2>&1; diff ${BASH_TSTOUT} X.right`
- A `run-all` aggregate that loops through the individual runners.

Substituting `THIS_SH=<huck-release-binary>` is the integration. Every
category that diffs cleanly is a feature huck has fully implemented;
every diff is a divergence to triage.

## Licensing posture

Bash is GPLv3+. The test files are part of bash and inherit that
license.

- **We do NOT vendor any bash sources into huck's repo.** The harness
  reads them from `$BASH_SOURCE_DIR/tests/` at runtime — the operator
  is responsible for fetching the bash tarball and pointing the
  environment variable at it.
- The harness itself (`tests/bash-test-suite/runner.sh`) is
  huck-authored, MIT-licensed code.
- The committed `docs/bash-test-suite-baseline.md` contains zero
  verbatim bash content — only category names (facts), pass/fail
  counts (facts), and huck-authored prose notes about what failed.
- Per-category diff logs land in `/tmp/huck-bash-tests-<UTC-timestamp>/`.
  Those CAN contain GPL'd bash output bytes — they stay on the
  operator's machine, never enter the repo.
- The new harness directory ships a README that documents this posture
  so future contributors don't accidentally vendor.

Running the harness does not implicate the GPL on huck itself, the
same way running gcc on C code or bash on a script doesn't taint the
input. The GPL only triggers when GPL-covered code is redistributed
or linked into a derivative work.

## Scope

**In scope:**

- New directory `tests/bash-test-suite/` containing:
  - `runner.sh` — the main runner.
  - `known-skips.txt` — initial skip list (categories that depend on
    huck features that don't exist yet, e.g. `run-loadable`).
  - `README.md` — operator instructions + licensing posture.
- New file `docs/bash-test-suite-baseline.md` — the triaged sweep
  results (the v214 implementer fills it in by running the survey
  on their machine).
- New shell harness `tests/scripts/bash_test_suite_runner_check.sh`
  — a tiny smoke test that runs the runner against ONE known-passing
  category (`arith` is the likely candidate) and asserts the runner
  itself reports PASS. This pins the runner mechanics without
  depending on a clean full sweep.
- One-sentence pointer in `docs/architecture.md` under the testing
  section.

**Out of scope:**

- Fixing any bash-test failure. Every failure goes into the baseline
  doc as a Note. Bug-fix iterations come later (v215+).
- CI integration. The runner is opt-in via env var. Most categories
  fail today; running it in CI would block every commit until the
  baseline is at PASS.
- Vendoring bash sources.
- Updating `docs/bash-divergences.md`. The bash-divergences catalog
  tracks specific named divergences; the bash-test-suite baseline is
  a parallel artifact (each category-level failure may correspond
  to one or more named divergences, or to new bugs).
- Promoting any subset of bash-test categories to regression-guard
  status. A future iteration will pick a clean subset and add a
  `bash_subset_diff_check.sh` once the underlying bugs are fixed.

## Architecture

### Directory layout

```
tests/bash-test-suite/
├── runner.sh          # entry point; 150-200 LOC bash
├── known-skips.txt    # initial skip list
└── README.md          # operator docs

docs/bash-test-suite-baseline.md   # committed triage doc

tests/scripts/bash_test_suite_runner_check.sh   # smoke test
```

The new `tests/bash-test-suite/` directory is intentionally
SEPARATE from `tests/scripts/` so the existing 131-harness sweep
loop (`for h in tests/scripts/*_diff_check.sh`) doesn't pick it up.
The runner is opt-in.

### Runner behavior

```
1. Preflight:
   a. Refuse if $BASH_SOURCE_DIR is unset.
   b. Refuse if $BASH_SOURCE_DIR/tests/run-arith doesn't exist.
   c. Refuse if $BASH_SOURCE_DIR/tests/README doesn't exist.
   Print actionable error pointing at the README in either case.

2. Build huck:
   cargo build --release --bin huck (quiet)
   HUCK=$(cd $(dirname runner.sh)/../.. && pwd)/target/release/huck

3. Scratch dir:
   SCRATCH=$(mktemp -d -t huck-bash-tests-XXXXXX)
   echo "Per-category logs: $SCRATCH"
   Copy $BASH_SOURCE_DIR/tests/* into $SCRATCH/tests/.

4. Discover categories:
   ls $SCRATCH/tests/run-* | grep -v 'run-all$'
   Skip categories listed in known-skips.txt.

5. Per category:
   cd $SCRATCH/tests
   BASH_TSTOUT=$SCRATCH/category.out
   THIS_SH=$HUCK
   Run: timeout $HUCK_TEST_TIMEOUT (default 30) sh ./run-<category>
   Classify:
     - exit 0, diff empty   → PASS
     - exit non-zero, diff non-empty → FAIL (record size + first 3 diff lines)
     - exit 124 (timeout)   → TIMEOUT
     - exit non-zero, no diff produced → ERROR (capture stderr)
   Write per-category stdout/stderr/diff to $SCRATCH/<category>.{out,err,diff}.

6. Emit Markdown summary to stdout:
   - Header: bash version, huck commit, sweep date.
   - Summary counts: PASS / FAIL / TIMEOUT / ERROR / SKIP totals.
   - Per-category table: name, status, first 3 diff lines (FAIL only).
   - Footer: path to $SCRATCH for full per-category logs.

7. Exit 0 if the runner itself ran to completion. The runner does NOT
   exit non-zero on bash-test failures — every status is a data point.
   Exit non-zero only on preflight failure or if huck's release build
   failed.
```

### Smoke test mechanics

`tests/scripts/bash_test_suite_runner_check.sh` does ONLY:

```bash
1. If $BASH_SOURCE_DIR is unset: print "skip: BASH_SOURCE_DIR not set" + exit 0.
   (This makes the smoke test green in the existing 131-harness sweep
   even when bash sources aren't available — non-blocking.)
2. Else: run the bash-test-suite runner ONLY for the `arith` category
   (pass a CATEGORY_FILTER env var the runner reads to limit work).
3. Assert the runner's per-category status for arith is PASS.
4. Exit 0 on PASS, 1 otherwise.
```

This smoke test runs in the existing `tests/scripts/*` sweep loop and
PASSes (or SKIPs harmlessly) on every machine. It pins runner mechanics
without depending on the full bash source tree.

The runner reads `$HUCK_BASH_TEST_CATEGORY` env var: when set, only
that category runs; when unset, the full sweep runs. This avoids
adding flag parsing to a small bash script. The smoke harness sets
`HUCK_BASH_TEST_CATEGORY=arith` before invoking the runner.

### Output format

The runner emits Markdown to stdout. The v214 implementer captures it
and saves to `docs/bash-test-suite-baseline.md` after hand-triaging
the Note column. The baseline doc:

```markdown
# bash 5.2.21 test-suite baseline (v214 initial sweep)

bash source: 5.2.21 (GNU, GPLv3+; not vendored, run from $BASH_SOURCE_DIR).
huck commit: <SHA at sweep time, fill in from `git rev-parse HEAD`>.
Sweep date: YYYY-MM-DD.

## Summary
- Categories run: NN (of 60, after skips)
- PASS: NN
- FAIL: NN
- TIMEOUT: NN
- ERROR: NN
- SKIP: NN (from known-skips.txt)

## Per-category status

| Category | Status | Note |
|---|---|---|
| alias | PASS | |
| arith | PASS | |
| arith-for | FAIL | Pre-existing huck divergence in alias chain handling. |
| array | FAIL | New — needs investigation in v215+. |
| assoc | FAIL | Known L-44 (assoc iteration order). |
| coproc | TIMEOUT | Known M-126 (single-coproc guard hangs the second). |
| (… one row per category …) | | |

## Skipped categories (from known-skips.txt)

| Category | Reason |
|---|---|
| run-loadable | huck has no loadable-builtin support; bash-specific. |
| run-cprint | depends on bash %(T) date format; deferred. |

## How to regenerate

Run `tests/bash-test-suite/runner.sh` with `$BASH_SOURCE_DIR` set;
hand-triage the Note column based on the per-category logs in
`/tmp/huck-bash-tests-<timestamp>/`; commit the updated doc.
```

The Notes column is the operator's classification. Categories of
notes:
- **Pre-existing huck divergence:** maps to a named entry in
  `docs/bash-divergences.md`. Cite the M-/L-id.
- **Known issue, no divergence ID yet:** cite the iteration number
  (`v157 review`, etc.) or describe in one line.
- **New bug:** flagged for investigation.
- **bash-specific feature:** the test exercises something huck
  doesn't claim to implement (e.g. `bashbug`-style features); should
  probably be added to `known-skips.txt` in a follow-on.
- **Test infrastructure issue:** test depends on a specific Linux
  feature, locale, or temp file behavior; out of huck's scope.

## Risks

1. **GPL contamination in committed artifacts.** Mitigation: the spec
   + README document the posture; reviewers explicitly check that
   `docs/bash-test-suite-baseline.md` contains no verbatim bash test
   content. Only huck-authored prose + facts (category names, counts).

2. **Bash version drift.** v214 targets bash 5.2.21 (the host's
   version). Future bash releases may add/rename categories; the
   baseline doc cites the bash version, so a v215+ refresh against
   bash 5.3 starts fresh. Not a v214 concern.

3. **Per-category timeout misclassifies real bugs as TIMEOUT.** A
   FAIL that hangs forever is still a bug, just a different shape.
   The Note column should distinguish "TIMEOUT — hang under known
   M-XX" from "TIMEOUT — unknown cause".

4. **Runner mechanics bugs.** The smoke harness pins the
   single-category path. Full-sweep failures could include runner
   bugs (e.g. mishandling `BASH_TSTOUT` env), which would look like
   "every category ERRORs". Mitigation: the implementer runs the
   sweep end-to-end during v214 and visually validates the output
   before committing the baseline.

5. **Some tests modify `tests/` files in place.** A few bash tests
   `cp`/`mv` files within the tests dir. We copy `tests/*` into
   `$SCRATCH/tests/` for isolation — modifications don't leak back
   to `$BASH_SOURCE_DIR`.

## Acceptance

- `tests/bash-test-suite/runner.sh` runs to completion in under 10
  minutes on bash 5.2.21 with huck at its current release build.
- `tests/scripts/bash_test_suite_runner_check.sh` exits 0 (PASS or
  harmlessly skips when `$BASH_SOURCE_DIR` is unset).
- `docs/bash-test-suite-baseline.md` committed with concrete counts
  + a triaged Note column for every non-PASS category. No verbatim
  bash content.
- `tests/bash-test-suite/README.md` documents fetching bash, setting
  `BASH_SOURCE_DIR`, the licensing posture, and the runner usage.
- `tests/bash-test-suite/known-skips.txt` lists initial skips with
  one-line reasons.
- `cargo test --workspace --quiet` green (no Rust changes; pure
  regression-survival check).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- All 131 existing `tests/scripts/*_diff_check.sh` harnesses still
  pass (the new smoke harness adds one to the loop, total 132).
- `docs/architecture.md` gains a one-sentence pointer to the new
  harness directory under the testing section.

## Testing strategy

This iteration has no Rust changes. Verification is:
1. Runner mechanics via the smoke harness.
2. Sweep execution on the implementer's machine.
3. Manual triage of the Note column.
4. Visual validation that the committed baseline doc contains no
   verbatim bash test content (spot-check by reviewers).

No bash-diff harness changes beyond the smoke.

## Documentation updates

- `docs/architecture.md`: one sentence under the "Testing" section
  pointing at `tests/bash-test-suite/` and noting it's opt-in.
- `docs/bash-test-suite-baseline.md`: NEW; the triage doc.
- `tests/bash-test-suite/README.md`: NEW; operator instructions.
- `docs/bash-divergences.md`: NO change in v214. Future iterations
  may edit it when they close a specific failure into a named
  divergence.
