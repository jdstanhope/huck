# Bash test-suite integration

This directory contains a harness that runs upstream bash's own test
suite against `huck` and produces a triage report at
`docs/bash-test-suite-baseline.md`.

## Licensing

**Bash is GPLv3+.** Its test suite (`tests/` in the bash source tree)
inherits that license. We do **not** vendor any bash source files into
this repository — the harness reads them from `$BASH_SOURCE_DIR/tests/`
at runtime.

The harness itself (`runner.sh`), the skip list (`known-skips.txt`),
and this README are huck's own MIT-licensed code. The committed
baseline document (`docs/bash-test-suite-baseline.md`) contains only
huck-authored content (category names, status counts, prose notes about
what failed) — never verbatim bash content.

Per-category diff logs land in `/tmp/huck-bash-tests-<UTC-timestamp>/`
when the runner executes. Those CAN contain GPL'd bash output and
**must not be committed** to this repo.

## One-time setup

```bash
curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
```

The harness targets **bash 5.2.21** specifically — see the spec for
why version pinning matters. Other versions may work but the baseline
doc is keyed to 5.2.21.

## Running the survey

```bash
bash tests/bash-test-suite/runner.sh
```

Output: a Markdown summary on stdout. Full per-category logs (the
actual diffs against bash's `.right` files) go to
`/tmp/huck-bash-tests-<timestamp>/`.

To run a single category:

```bash
HUCK_BASH_TEST_CATEGORY=arith bash tests/bash-test-suite/runner.sh
```

To change the per-category timeout (default 30s):

```bash
HUCK_TEST_TIMEOUT=60 bash tests/bash-test-suite/runner.sh
```

## Interpreting the output

| Status | Meaning |
|---|---|
| PASS | huck's output is byte-identical to bash's `.right` for the category. |
| FAIL | huck completed but its output diffs from `.right`. |
| TIMEOUT | The category exceeded the per-test timeout. Often a real hang in huck. |
| ERROR | The runner couldn't classify the result (e.g. huck crashed). |
| SKIP | Listed in `known-skips.txt`; not run. |

Full diffs and stderr land in `/tmp/huck-bash-tests-<timestamp>/<category>.{diff,err,out}`.

## Updating the committed baseline

After running the survey, hand-triage each non-PASS category and write
huck-authored prose for the Note column in
`docs/bash-test-suite-baseline.md`. Common Note categories:

- "Pre-existing huck divergence: M-XX" (cite a `docs/bash-divergences.md` entry)
- "Known issue, no divergence ID: ..." (one-line summary)
- "New bug — needs investigation"
- "bash-specific feature huck doesn't implement"

Never copy verbatim bash test bytes into the Note column.

## Why this is separate from `tests/scripts/`

The existing `tests/scripts/*_diff_check.sh` sweep loop runs every
harness as a regression guard. The bash-test-suite runner is **opt-in**
(requires `$BASH_SOURCE_DIR` to be set) and most categories FAIL today;
running it in CI would block every commit. The smoke harness
`tests/scripts/bash_test_suite_runner_diff_check.sh` is a tiny shim that
exercises the runner mechanics against a single known-passing category
when `$BASH_SOURCE_DIR` is set, and skips harmlessly otherwise — so the
sweep loop stays green on machines without bash sources.
