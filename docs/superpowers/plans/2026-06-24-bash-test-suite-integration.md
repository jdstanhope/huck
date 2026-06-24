# v214 Bash test-suite integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run upstream bash 5.2.21's `tests/` against huck and commit a triaged baseline doc capturing every divergence. v214 ships the harness + README + skip list + smoke test + the first-sweep baseline doc. No production code changes.

**Architecture:** New `tests/bash-test-suite/` directory holds `runner.sh` (the entry point), `known-skips.txt` (intentional skips), and `README.md` (operator docs). The runner reads `$BASH_SOURCE_DIR/tests/`, runs every `run-<category>` under a timeout, classifies each as PASS/FAIL/TIMEOUT/ERROR/SKIP, and emits a Markdown summary. `docs/bash-test-suite-baseline.md` captures the first sweep (hand-triaged Note column). A tiny smoke harness `tests/scripts/bash_test_suite_runner_check.sh` pins runner mechanics by running a single known-passing category (`arith`); it skips gracefully when `$BASH_SOURCE_DIR` is unset so it doesn't break the existing 131-harness sweep.

**Tech Stack:** Pure bash. No Rust changes. No new crate deps. The runner uses `mktemp`, `timeout`, `diff` — all standard POSIX/coreutils.

**Branch:** `v214-bash-test-suite-integration`.

**Spec:** `docs/superpowers/specs/2026-06-24-bash-test-suite-integration-design.md`.

## Global Constraints

- Commit trailer (every commit): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` exact, last line of every commit.
- No source comments referencing `v214` / task numbers / iteration version. The harness file headers MAY mention `# v214:` (harness comments name the feature/iteration — that's their purpose, established by every existing `*_diff_check.sh` header).
- **Licensing**: bash test files are GPLv3+ and MUST NEVER be copied into huck's repo. The runner reads them from `$BASH_SOURCE_DIR/tests/` at runtime. Per-category diff logs land in `/tmp/huck-bash-tests-<timestamp>/` — those CAN contain GPL'd bash content and must NEVER be committed. `docs/bash-test-suite-baseline.md` MUST contain only huck-authored content (category names, status counts, prose notes).
- Bash version targeted: **5.2.21**. The baseline doc cites this version explicitly.
- Workspace test command unchanged: `cargo test --workspace`.
- The new smoke harness `tests/scripts/bash_test_suite_runner_check.sh` MUST exit 0 (PASS or harmlessly skip) when `$BASH_SOURCE_DIR` is unset, so the existing `for h in tests/scripts/*_diff_check.sh` sweep loop stays green on machines without bash sources.

**Verified facts (pre-plan):**
- Bash 5.2.21 ships ~60 `run-<category>` scripts of the form `${THIS_SH} ./X.tests > ${BASH_TSTOUT} 2>&1; diff ${BASH_TSTOUT} X.right`.
- `run-all` is the aggregate; we skip it and iterate individuals.
- Tarball URL: `https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz`.
- Some run-* scripts emit a leading "warning: ..." to stderr (verified in `run-builtins`); the diff against `.right` accounts for it because bash's own `.right` files include that warning. Our harness captures stderr to the BASH_TSTOUT file (same as bash's own runner does via `2>&1`).
- Existing harnesses live in `tests/scripts/`. The new harness directory is `tests/bash-test-suite/` (intentionally separate so the `*_diff_check.sh` glob loop doesn't pick up runner.sh).
- Existing harness style for the smoke test: see `tests/scripts/array_transforms_diff_check.sh` — `cd "$(dirname "$0")/../.."`, `cargo build --quiet --workspace --bin huck`, helper `check label frag`, exit 0/1.

---

## File structure

**Create:**
- `tests/bash-test-suite/runner.sh` (executable, ~180 LOC bash)
- `tests/bash-test-suite/known-skips.txt` (~15 LOC, plain-text skip list)
- `tests/bash-test-suite/README.md` (~80 LOC operator docs)
- `tests/scripts/bash_test_suite_runner_check.sh` (executable, ~40 LOC smoke harness)
- `docs/bash-test-suite-baseline.md` (~100 LOC; filled in by Task 4's actual sweep)

**Modify:**
- `docs/architecture.md` — one-sentence pointer under the testing section.

No Rust changes. No `Cargo.toml` changes. No `crates/` changes.

---

## Task 1: Scaffold `tests/bash-test-suite/` (README + skip list + runner stub)

**Files:**
- Create: `tests/bash-test-suite/runner.sh` (stub: prints "not yet implemented", exits 0).
- Create: `tests/bash-test-suite/known-skips.txt` (initial entries with one-line reasons).
- Create: `tests/bash-test-suite/README.md` (operator docs).

**Interfaces:**
- Produces:
  - Directory `tests/bash-test-suite/` exists; both files referenced by Tasks 2+ are present.
  - `runner.sh` is a no-op stub (exits 0). Task 2 fills in the real body.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v214-bash-test-suite-integration
```

- [ ] **Step 2: Create the directory + skip list**

```bash
mkdir -p tests/bash-test-suite
```

Create `tests/bash-test-suite/known-skips.txt`:

```text
# Categories from bash's tests/ directory that we intentionally skip.
# Format: one category name per line, optional comment after #.
# A "category" here is the suffix of run-<category> in bash's tests/ dir.
# Update via the v214+ baseline triage as more skips are identified.

loadable          # huck has no loadable-builtin support; bash-specific
intl              # depends on locale/i18n; out of huck's compat scope
strict-posix      # depends on POSIX-strict mode huck doesn't implement
```

The initial skip list is conservative — only categories the operator
KNOWS huck doesn't claim to support. Task 4 may add more after running
the sweep and seeing which categories TIMEOUT or ERROR for reasons
unrelated to a real bug.

- [ ] **Step 3: Create the README**

Create `tests/bash-test-suite/README.md`:

````markdown
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
`tests/scripts/bash_test_suite_runner_check.sh` is a tiny shim that
exercises the runner mechanics against a single known-passing category
when `$BASH_SOURCE_DIR` is set, and skips harmlessly otherwise — so the
sweep loop stays green on machines without bash sources.
````

- [ ] **Step 4: Create the stub runner**

Create `tests/bash-test-suite/runner.sh`:

```bash
#!/usr/bin/env bash
# v214: bash test-suite runner — runs bash's own tests/run-* against huck.
# This is a stub; the real body lands in the next commit.
set -u
echo "bash-test-suite runner stub — implementation pending"
exit 0
```

Then make it executable:

```bash
chmod +x tests/bash-test-suite/runner.sh
```

- [ ] **Step 5: Verify the directory + files exist**

```bash
ls -la tests/bash-test-suite/
bash tests/bash-test-suite/runner.sh
```

Expected: 3 files (`runner.sh` executable, `known-skips.txt`,
`README.md`); runner prints the stub message and exits 0.

- [ ] **Step 6: Confirm existing harnesses still pass**

```bash
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done
```

Expected: zero failures (131 harnesses still pass; we haven't added
the smoke harness yet — that lands in Task 3).

- [ ] **Step 7: Commit**

```bash
git add tests/bash-test-suite/
git commit -m "$(cat <<'EOF'
v214 task 1: scaffold tests/bash-test-suite/ — README + skip list + runner stub

New directory holds the bash test-suite integration. README documents
the GPL-licensing posture (bash source NOT vendored; harness reads from
$BASH_SOURCE_DIR at runtime), operator workflow (fetch bash, set env
var, run survey), interpretation of status codes, and why this is
separate from tests/scripts/. known-skips.txt has 3 conservative
initial entries (loadable / intl / strict-posix). runner.sh is a stub
that prints a marker and exits 0; Task 2 fills in the real body.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Implement `runner.sh` (preflight + per-category + classification)

**Files:**
- Modify: `tests/bash-test-suite/runner.sh` (~180 LOC bash, replaces the stub).

**Interfaces:**
- Consumes: `tests/bash-test-suite/known-skips.txt` (from Task 1).
- Produces:
  - When invoked with `$BASH_SOURCE_DIR` set: iterates every `run-<category>` in `$BASH_SOURCE_DIR/tests/` (except `run-all` and entries in `known-skips.txt`), runs each with `THIS_SH=$HUCK` under a `$HUCK_TEST_TIMEOUT`-second timeout (default 30), classifies as PASS/FAIL/TIMEOUT/ERROR, writes per-category logs to a scratch dir, emits a Markdown summary to stdout.
  - When invoked with `$HUCK_BASH_TEST_CATEGORY=<name>` set: runs only that one category (used by the smoke harness in Task 3).
  - Exit 0 on successful runner completion (regardless of per-category PASS/FAIL counts); exit non-zero only on preflight failure or huck-build failure.

- [ ] **Step 1: Replace the stub with the real runner**

Replace the entire contents of `tests/bash-test-suite/runner.sh` with:

```bash
#!/usr/bin/env bash
# v214: bash test-suite runner — runs bash's own tests/run-* against huck.
# Reads bash source from $BASH_SOURCE_DIR (operator-supplied; NOT vendored).
# Emits a Markdown summary to stdout; per-category logs land in a /tmp scratch.
#
# Env vars:
#   BASH_SOURCE_DIR             (required) path to extracted bash-5.2.21/ source
#   HUCK_BASH_TEST_CATEGORY     (optional) single category name to run; default: all
#   HUCK_TEST_TIMEOUT           (optional) per-category timeout in seconds; default: 30
set -u

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SKIPS_FILE="$ROOT/tests/bash-test-suite/known-skips.txt"
TIMEOUT=${HUCK_TEST_TIMEOUT:-30}

# ---- Preflight -----------------------------------------------------

if [ -z "${BASH_SOURCE_DIR:-}" ]; then
    cat >&2 <<EOF
error: \$BASH_SOURCE_DIR is not set.

This harness reads upstream bash's test files from \$BASH_SOURCE_DIR/tests/
at runtime; nothing is vendored into this repo (GPL licensing posture).
See $ROOT/tests/bash-test-suite/README.md for setup instructions.
EOF
    exit 1
fi

if [ ! -f "$BASH_SOURCE_DIR/tests/run-arith" ]; then
    echo "error: \$BASH_SOURCE_DIR/tests/run-arith not found." >&2
    echo "  BASH_SOURCE_DIR=$BASH_SOURCE_DIR doesn't look like a bash source tree." >&2
    echo "  See $ROOT/tests/bash-test-suite/README.md." >&2
    exit 1
fi

if [ ! -f "$BASH_SOURCE_DIR/tests/README" ]; then
    echo "error: \$BASH_SOURCE_DIR/tests/README not found." >&2
    echo "  BASH_SOURCE_DIR=$BASH_SOURCE_DIR is missing the tests/ README." >&2
    exit 1
fi

# ---- Build huck ----------------------------------------------------

if ! (cd "$ROOT" && cargo build --release --quiet --bin huck 2>/dev/null); then
    echo "error: 'cargo build --release --bin huck' failed; cannot run survey." >&2
    exit 1
fi
HUCK="$ROOT/target/release/huck"

if [ ! -x "$HUCK" ]; then
    echo "error: huck release binary not at $HUCK after build." >&2
    exit 1
fi

# ---- Scratch dir ---------------------------------------------------

STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SCRATCH=$(mktemp -d -t "huck-bash-tests-${STAMP}.XXXXXX")

# Copy bash tests/ into the scratch so tests that modify-in-place don't
# touch the operator's $BASH_SOURCE_DIR. Many .tests reference .sub
# files by relative path, so we need the full directory shape.
cp -a "$BASH_SOURCE_DIR/tests" "$SCRATCH/tests"

# ---- Load skip list -----------------------------------------------

declare -A SKIPS=()
if [ -f "$SKIPS_FILE" ]; then
    while IFS= read -r line; do
        # strip comments and whitespace
        cat=$(printf '%s\n' "$line" | sed -e 's/#.*//' -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
        [ -z "$cat" ] && continue
        SKIPS[$cat]=1
    done < "$SKIPS_FILE"
fi

# ---- Discover categories ------------------------------------------

CATEGORIES=()
if [ -n "${HUCK_BASH_TEST_CATEGORY:-}" ]; then
    # Single-category mode (used by the smoke harness).
    if [ -f "$SCRATCH/tests/run-${HUCK_BASH_TEST_CATEGORY}" ]; then
        CATEGORIES=("$HUCK_BASH_TEST_CATEGORY")
    else
        echo "error: HUCK_BASH_TEST_CATEGORY=$HUCK_BASH_TEST_CATEGORY not found at $SCRATCH/tests/run-$HUCK_BASH_TEST_CATEGORY." >&2
        exit 1
    fi
else
    # Full sweep: every run-* except run-all and any in the skip list.
    while IFS= read -r runner; do
        base=$(basename "$runner")
        cat=${base#run-}
        [ "$cat" = "all" ] && continue
        [ -n "${SKIPS[$cat]:-}" ] && continue
        CATEGORIES+=("$cat")
    done < <(ls -1 "$SCRATCH/tests"/run-* 2>/dev/null | sort)
fi

# ---- Per-category execution + classification ---------------------

declare -A STATUS=()
declare -A DIFF_HEAD=()
PASS=0; FAIL=0; TIMEOUTS=0; ERRORS=0

for cat in "${CATEGORIES[@]}"; do
    runner="$SCRATCH/tests/run-$cat"
    if [ ! -x "$runner" ] && [ ! -r "$runner" ]; then
        STATUS[$cat]=ERROR
        ERRORS=$((ERRORS+1))
        echo "missing runner: $runner" > "$SCRATCH/$cat.err"
        continue
    fi

    # bash's runners cd into ./tests (they're invoked from inside that dir).
    # We replicate by running from $SCRATCH/tests.
    out_file="$SCRATCH/$cat.out"
    err_file="$SCRATCH/$cat.err"
    diff_file="$SCRATCH/$cat.diff"

    # bash's runners expect $THIS_SH and $BASH_TSTOUT.
    # Run with `timeout` to bound hangs.
    (
        cd "$SCRATCH/tests"
        THIS_SH="$HUCK" BASH_TSTOUT="$out_file" timeout "$TIMEOUT" sh "./run-$cat" \
            > "$diff_file" 2> "$err_file"
    )
    rc=$?

    if [ "$rc" -eq 124 ]; then
        STATUS[$cat]=TIMEOUT
        TIMEOUTS=$((TIMEOUTS+1))
    elif [ "$rc" -eq 0 ] && [ ! -s "$diff_file" ]; then
        STATUS[$cat]=PASS
        PASS=$((PASS+1))
    elif [ -s "$diff_file" ]; then
        STATUS[$cat]=FAIL
        FAIL=$((FAIL+1))
        DIFF_HEAD[$cat]=$(head -3 "$diff_file" | tr '\n' '|' | sed 's/|$//')
    else
        STATUS[$cat]=ERROR
        ERRORS=$((ERRORS+1))
    fi
done

# ---- Emit Markdown summary ----------------------------------------

TOTAL=${#CATEGORIES[@]}
SKIP_COUNT=${#SKIPS[@]}

cat <<EOF
# bash test-suite sweep — $(date -u +%Y-%m-%d)

bash source: $(basename "$BASH_SOURCE_DIR")
huck commit: $(cd "$ROOT" && git rev-parse --short HEAD 2>/dev/null || echo unknown)
Scratch dir (full diffs): $SCRATCH

## Summary

- Categories run: $TOTAL
- PASS: $PASS
- FAIL: $FAIL
- TIMEOUT: $TIMEOUTS
- ERROR: $ERRORS
- SKIP (from known-skips.txt): $SKIP_COUNT

## Per-category status

| Category | Status |
|---|---|
EOF

for cat in "${CATEGORIES[@]}"; do
    printf '| %s | %s |\n' "$cat" "${STATUS[$cat]}"
done

if [ "$SKIP_COUNT" -gt 0 ]; then
    echo
    echo "## Skipped (from known-skips.txt)"
    echo
    echo "| Category |"
    echo "|---|"
    for cat in "${!SKIPS[@]}"; do
        printf '| %s |\n' "$cat"
    done | sort
fi

exit 0
```

Notes on the implementation:

- We deliberately do NOT emit the `DIFF_HEAD` first-three-lines into the
  Markdown table — those bytes are bash test output (potentially
  GPL-derivative). They live ONLY in the per-category `$SCRATCH/$cat.diff`
  file on the operator's machine, never in stdout.
- `timeout` exit code 124 is the canonical "process was killed by
  timeout" signal from GNU coreutils. macOS `timeout` from Homebrew
  uses the same convention.
- The runner runs each category serially. Parallelism would be nice
  but risks subtle interactions (some tests use `/tmp`, fd numbers,
  signal handlers); keep it simple.
- `cp -a $BASH_SOURCE_DIR/tests $SCRATCH/tests` is the isolation step —
  any in-place file modifications by a test only affect the scratch.

- [ ] **Step 2: Smoke-run the runner with `$BASH_SOURCE_DIR` UNSET**

```bash
unset BASH_SOURCE_DIR
bash tests/bash-test-suite/runner.sh
echo "exit=$?"
```

Expected: actionable error message to stderr; exit 1.

- [ ] **Step 3: Fetch bash source for the implementer's smoke run**

```bash
if [ ! -d /tmp/bash-5.2.21 ]; then
    curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp
fi
```

If the implementer is offline, they can skip this step and run Task 4
later when network is available. Tasks 2-3 work without the bash source
because Task 2 verifies error paths and Task 3's smoke skips gracefully.

- [ ] **Step 4: Smoke-run the runner against a single category**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
HUCK_BASH_TEST_CATEGORY=arith bash tests/bash-test-suite/runner.sh
```

Expected: Markdown summary printed to stdout, single row for `arith`,
status PASS or FAIL (PASS is most likely given huck's strong arith
coverage). Scratch dir path printed in the header. Exit 0.

If the output looks reasonable, the runner mechanics work.

- [ ] **Step 5: Commit**

```bash
git add tests/bash-test-suite/runner.sh
git commit -m "$(cat <<'EOF'
v214 task 2: implement runner.sh — preflight + per-category + classification

Replaces Task 1's stub. The runner:
- Preflights $BASH_SOURCE_DIR (set, contains tests/run-arith + README).
- Builds huck release binary; aborts if build fails.
- Copies $BASH_SOURCE_DIR/tests into a /tmp scratch (isolation).
- Loads tests/bash-test-suite/known-skips.txt; honors the skip list.
- Iterates every run-* (or just $HUCK_BASH_TEST_CATEGORY if set),
  classifies each as PASS / FAIL / TIMEOUT / ERROR under a 30s
  default per-category timeout ($HUCK_TEST_TIMEOUT override).
- Emits a Markdown summary with category-name + status only — no
  diff content (would be GPL-derivative bash test output).
  Per-category diffs land in $SCRATCH/<cat>.{diff,out,err} for the
  operator's local triage; never committed.
- Exits 0 on completion regardless of pass/fail counts.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Smoke harness `bash_test_suite_runner_check.sh`

**Files:**
- Create: `tests/scripts/bash_test_suite_runner_check.sh` (~40 LOC bash, executable).

**Interfaces:**
- Consumes: `tests/bash-test-suite/runner.sh` (from Task 2; supports `HUCK_BASH_TEST_CATEGORY`).
- Produces:
  - A harness that exits 0 when `$BASH_SOURCE_DIR` is unset (harmless skip; lets the existing 131-harness sweep stay green on machines without bash sources).
  - When `$BASH_SOURCE_DIR` is set: runs the runner against `arith`, asserts the per-category status is PASS, exits 0 on PASS, 1 otherwise.

- [ ] **Step 1: Create the smoke harness**

Create `tests/scripts/bash_test_suite_runner_check.sh`:

```bash
#!/usr/bin/env bash
# v214: smoke harness for the bash test-suite runner.
# Asserts the runner reports PASS for the `arith` category (huck's
# strongest area, expected to be byte-identical to bash). When
# $BASH_SOURCE_DIR is unset, exits 0 with a SKIP message — keeps the
# existing tests/scripts/*_diff_check.sh sweep loop green on machines
# without bash sources.
set -u

cd "$(dirname "$0")/../.." || exit 1

if [ -z "${BASH_SOURCE_DIR:-}" ]; then
    echo "SKIP: \$BASH_SOURCE_DIR unset; bash-test-suite runner not exercised."
    echo "      See tests/bash-test-suite/README.md to enable."
    exit 0
fi

# Run the runner against a single category.
OUT=$(HUCK_BASH_TEST_CATEGORY=arith bash tests/bash-test-suite/runner.sh 2>&1)
rc=$?

if [ "$rc" -ne 0 ]; then
    echo "FAIL: runner exited $rc"
    echo "$OUT"
    exit 1
fi

# Look for the arith row in the Markdown table. Pattern: `| arith | <status> |`.
status=$(printf '%s\n' "$OUT" | awk -F'|' '/^\| arith / { gsub(/ /, "", $3); print $3 }')

if [ -z "$status" ]; then
    echo "FAIL: could not find arith row in runner output"
    echo "$OUT"
    exit 1
fi

if [ "$status" != "PASS" ]; then
    echo "FAIL: arith status = $status (expected PASS)"
    echo
    echo "Runner output:"
    echo "$OUT"
    exit 1
fi

echo "PASS [bash_test_suite_runner_check] arith=PASS"
exit 0
```

Make executable:

```bash
chmod +x tests/scripts/bash_test_suite_runner_check.sh
```

- [ ] **Step 2: Smoke the harness with `$BASH_SOURCE_DIR` UNSET**

```bash
unset BASH_SOURCE_DIR
bash tests/scripts/bash_test_suite_runner_check.sh
echo "exit=$?"
```

Expected: prints the SKIP message; exits 0. This is the critical
behavior — the existing sweep loop must stay green.

- [ ] **Step 3: Smoke the harness with `$BASH_SOURCE_DIR` SET (if bash source available)**

```bash
if [ -d /tmp/bash-5.2.21 ]; then
    BASH_SOURCE_DIR=/tmp/bash-5.2.21 bash tests/scripts/bash_test_suite_runner_check.sh
    echo "exit=$?"
fi
```

Expected (when bash source is present): prints `PASS
[bash_test_suite_runner_check] arith=PASS`; exits 0.

If the harness reports `FAIL: arith status = FAIL` instead of PASS,
huck's arith has regressed against bash 5.2.21's tests. That's a real
bug to investigate (could happen if some recent iteration broke
arith), but for Task 3 the harness mechanics are still correct — the
failure is informational. If this happens, file the finding in a
comment on the v214 controller's review and defer the fix to a
separate iteration.

- [ ] **Step 4: Confirm the full existing-harness sweep still passes**

```bash
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done
```

Expected: zero failures. The new smoke harness must SKIP cleanly (exit
0) when `$BASH_SOURCE_DIR` is unset, which is the default in CI / local
runs.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/bash_test_suite_runner_check.sh
git commit -m "$(cat <<'EOF'
v214 task 3: smoke harness for the bash test-suite runner

tests/scripts/bash_test_suite_runner_check.sh runs the runner against
the `arith` category and asserts PASS. When $BASH_SOURCE_DIR is unset
(the default), the harness prints SKIP and exits 0 — keeps the
existing tests/scripts/*_diff_check.sh sweep loop green on machines
without bash sources. When set, it pins runner mechanics by verifying
the Markdown output shape and the arith row's status.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Run the full sweep + write the baseline doc

**Files:**
- Create: `docs/bash-test-suite-baseline.md` (~100 LOC; the triage doc).

**Interfaces:**
- Consumes: `tests/bash-test-suite/runner.sh` (from Task 2; full-sweep mode).
- Produces: the committed baseline doc.

This is the iteration's central deliverable. The implementer fetches
bash source, runs the full sweep, hand-triages each non-PASS category,
and commits the result. Unlike Tasks 1-3 which were mechanical, this
task requires JUDGMENT for the Note column.

- [ ] **Step 1: Ensure bash source is available**

```bash
if [ ! -d /tmp/bash-5.2.21 ]; then
    curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp
fi
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
```

- [ ] **Step 2: Run the full sweep**

```bash
bash tests/bash-test-suite/runner.sh | tee /tmp/v214-sweep.md
echo "---"
echo "Scratch dir (full per-category diffs):"
grep '^Scratch dir' /tmp/v214-sweep.md | head -1
```

Expected: runs to completion in 5-10 minutes; ~60 categories
classified. The Markdown summary is captured to `/tmp/v214-sweep.md`
for use in Step 3. The scratch dir path is printed in the header.

If the runner crashes or hangs (a runner-mechanics bug, not a
bash-test failure), fix the runner and re-run. v214's deliverable is a
completed sweep; a crashed runner means re-running Task 2.

- [ ] **Step 3: Triage each non-PASS category**

For each FAIL/TIMEOUT/ERROR category, examine
`$SCRATCH/<category>.diff` and `$SCRATCH/<category>.err`. Classify into
one of these Note categories (write huck-authored prose; never copy
verbatim bash content):

- **Pre-existing huck divergence: M-XX / L-XX** — when the failure
  maps to a named entry in `docs/bash-divergences.md`. Cite the
  divergence ID.
- **Known issue, no divergence ID** — when huck has a documented
  limitation in a prior iteration's memory but no divergence-doc
  entry. One-line description.
- **New bug — needs investigation** — when the failure looks novel
  (not in the divergence doc, not in iteration memory).
- **Bash-specific feature** — when the test exercises something
  huck doesn't claim to support (e.g. `printf '%(...)T'` time
  format, loadable builtins). If this is observed, the implementer
  should also add the category to `tests/bash-test-suite/known-skips.txt`
  in a follow-up commit (or this task can include both — see Step 5).
- **Test infrastructure** — when the failure depends on Linux-specific
  features, locale, or test-environment quirks (e.g. assumes
  `/dev/fd/N` is available). Out of huck's scope.

Use the operator's judgment. Don't spend more than ~5 minutes per
category; a fast first-pass triage is more valuable than perfect
classification.

- [ ] **Step 4: Write `docs/bash-test-suite-baseline.md`**

Create `docs/bash-test-suite-baseline.md` using the summary captured in
`/tmp/v214-sweep.md` PLUS the triaged Note column. Template:

```markdown
# bash 5.2.21 test-suite baseline

bash source: 5.2.21 (GNU, GPLv3+; not vendored, run from `$BASH_SOURCE_DIR`).
huck commit: <SHA at sweep time; fill in from `git rev-parse --short HEAD`>.
Sweep date: <YYYY-MM-DD>.

## Summary

- Categories run: <NN>
- PASS: <NN>
- FAIL: <NN>
- TIMEOUT: <NN>
- ERROR: <NN>
- SKIP (from known-skips.txt): <NN>

## Per-category status

| Category | Status | Note |
|---|---|---|
| alias | <status> | <prose; empty for PASS> |
| arith | PASS | |
| ... one row per category, in alphabetical order ... |

## Skipped categories

| Category | Reason |
|---|---|
| loadable | huck has no loadable-builtin support; bash-specific. |
| intl | depends on locale/i18n; out of huck's compat scope. |
| strict-posix | depends on POSIX-strict mode huck doesn't implement. |

## How to regenerate

1. `curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp`
2. `export BASH_SOURCE_DIR=/tmp/bash-5.2.21`
3. `bash tests/bash-test-suite/runner.sh > /tmp/sweep.md`
4. Hand-triage non-PASS categories using the per-category diffs printed
   in the runner's header path.
5. Update this document with the new status column and prose Notes.
6. Commit.

## Licensing reminder

This document contains only huck-authored content (category names,
status counts, prose notes). NEVER copy verbatim bash test output or
test-script contents into the Note column — those bytes are GPL'd.
The full per-category diffs live in `/tmp/huck-bash-tests-<timestamp>/`
and stay local.
```

Replace `<NN>` and `<status>` and `<prose>` with actual values from
the sweep + triage. Categories appear in alphabetical order.

- [ ] **Step 5: If any new skips were identified, update `known-skips.txt`**

If the triage in Step 3 surfaced categories that should be skipped
(e.g. they uniformly require a feature huck doesn't implement and
their FAIL is uninformative noise), add them to
`tests/bash-test-suite/known-skips.txt` with a one-line reason.

Re-run the sweep to confirm those categories now report SKIP and the
baseline counts shift accordingly. Update the baseline doc to match.

- [ ] **Step 6: Verify the baseline doc has no verbatim bash content**

Manual spot-check: open `docs/bash-test-suite-baseline.md` and confirm
every Note cell is huck-authored prose. No copy-pasted snippets from
bash `.tests` files or `.right` output.

A simple heuristic: if a Note column contains a `$`-sigil, backticks
around bash variables, or quoted strings that LOOK like shell output,
re-write to describe the failure abstractly instead.

- [ ] **Step 7: Commit**

```bash
git add docs/bash-test-suite-baseline.md tests/bash-test-suite/known-skips.txt
git commit -m "$(cat <<'EOF'
v214 task 4: initial bash 5.2.21 test-suite sweep + baseline doc

Ran the survey against bash 5.2.21 source. NN categories: NN PASS, NN
FAIL, NN TIMEOUT, NN ERROR, NN SKIP. Every non-PASS category has a
huck-authored Note classifying the failure (pre-existing divergence
ID, known limitation, new bug, bash-specific feature, or test-infra
quirk).

Document is committed without verbatim bash test content — Notes are
huck-authored prose; full per-category diffs stay in /tmp on the
operator's machine.

[If known-skips.txt was updated, note: "Also added M new entries to
known-skips.txt for categories N, O, P — these test features huck
doesn't claim to implement."]

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

The commit message should report the actual counts from the sweep.

---

## Task 5: Architecture pointer + final sweep + stop

**Files:**
- Modify: `docs/architecture.md` — one-sentence pointer under the testing section.

- [ ] **Step 1: Find the testing section in architecture.md**

```bash
grep -n -i 'testing\|harness\|tests/' docs/architecture.md | head -10
```

Identify the section that discusses huck's testing strategy
(`tests/scripts/`, the diff-check harnesses, etc.). Add a one-sentence
pointer near it:

```markdown
- **Bash test-suite integration** (v214) lives in
  `tests/bash-test-suite/` — opt-in runner that consumes upstream
  bash's own `tests/` directory via `$BASH_SOURCE_DIR`. Triaged
  baseline at `docs/bash-test-suite-baseline.md`.
```

Match the surrounding style (bullet list, headed list, or paragraph
prose). Keep it short — this is a pointer, not a duplication of the
README.

- [ ] **Step 2: Final full sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

# Existing harness sweep (must stay green; new smoke harness skips cleanly):
unset BASH_SOURCE_DIR
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"

# bash test-suite smoke (if bash source available):
if [ -d /tmp/bash-5.2.21 ]; then
    BASH_SOURCE_DIR=/tmp/bash-5.2.21 bash tests/scripts/bash_test_suite_runner_check.sh
fi
```

Expected: all green; release build clean; all 132 harnesses pass (131
prior + new smoke); smoke prints `hello` + `exit=0`; bash-test smoke
(if run) reports `PASS [bash_test_suite_runner_check] arith=PASS`.

If the new smoke harness FAILs against `BASH_SOURCE_DIR=/tmp/bash-5.2.21`
because the `arith` category isn't PASS, that's informational — not a
v214 blocker (v214 ships the harness + triage; the arith FAIL would
itself be a row in the baseline doc with a Note).

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v214 task 5: architecture.md pointer + final sweep

One-sentence pointer in docs/architecture.md noting the opt-in bash
test-suite integration under tests/bash-test-suite/, the
$BASH_SOURCE_DIR contract, and the committed baseline doc.

Full sweep: cargo test --workspace green; clippy -D warnings clean;
release builds; all 132 *_diff_check.sh harnesses pass (131 prior +
new bash_test_suite_runner_check.sh smoke).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Stop — do NOT merge**

The final whole-branch code review is the controller's call. Stop after
this commit.

---

## Self-review

**Spec coverage:**
- `tests/bash-test-suite/runner.sh`: Task 1 stub, Task 2 implementation.
- `tests/bash-test-suite/known-skips.txt`: Task 1 initial, Task 4 may extend.
- `tests/bash-test-suite/README.md`: Task 1.
- `docs/bash-test-suite-baseline.md`: Task 4.
- `tests/scripts/bash_test_suite_runner_check.sh`: Task 3.
- `docs/architecture.md` pointer: Task 5.
- Licensing posture (no vendoring, no verbatim bash content in committed
  artifacts): documented in Task 1 README, enforced in Task 4 Step 6
  spot-check.
- Smoke harness skips when `$BASH_SOURCE_DIR` unset (keeps existing
  sweep green): Task 3 Step 1's `if [ -z … ]` short-circuit, verified
  in Task 3 Step 2 + Task 5 Step 2.
- Runner mechanics (preflight, scratch dir, classification, Markdown
  output): Task 2.

**Placeholder scan:**
- `<NN>` / `<status>` / `<prose>` in Task 4 Step 4's template ARE
  placeholders — but they're labels for the implementer to fill from
  the actual sweep output. The template explicitly says "Replace `<NN>`
  and `<status>` and `<prose>` with actual values from the sweep +
  triage." Acceptable.
- "If known-skips.txt was updated" in Task 4 Step 7's commit message is
  a conditional, not vagueness — the bracketed text is template the
  implementer either includes or removes based on actual Task 4 Step 5
  behavior.
- No "TBD" / "implement later" / "fill in details" anywhere.

**Type consistency:**
- `$BASH_SOURCE_DIR` (env var name): consistent across Tasks 1, 2, 3, 4, 5.
- `$HUCK_BASH_TEST_CATEGORY` (env var name): consistent in Tasks 2, 3.
- `$HUCK_TEST_TIMEOUT` (env var name): consistent in Task 2 + README.
- Scratch dir name pattern `/tmp/huck-bash-tests-<timestamp>`: consistent
  in spec, Task 1 README, Task 2 runner.
- Skip list location `tests/bash-test-suite/known-skips.txt`: consistent.
- Baseline doc location `docs/bash-test-suite-baseline.md`: consistent.
- Smoke harness location `tests/scripts/bash_test_suite_runner_check.sh`:
  consistent.

**5 tasks. ~250 LOC bash + ~100 LOC docs. Zero Rust changes.**
