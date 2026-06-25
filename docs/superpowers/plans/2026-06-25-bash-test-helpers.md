# v217 — bash test-suite helper binaries (recho/zecho/printenv) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the bash test-suite runner compile and provide the `recho`/`zecho`/`printenv` helper programs so the ~21 categories that invoke them stop failing on `command not found`, and re-triage the baseline.

**Architecture:** Harness-only change. `tests/bash-test-suite/runner.sh` gains a preflight step that compiles the three helpers from `$BASH_SOURCE_DIR/support/*.c` (or uses a `HUCK_BASH_TEST_HELPERS` override dir) and prepends them to `PATH` for the category runs; it degrades gracefully if no compiler is available. No crate/source code changes.

**Tech Stack:** Bash (the runner script), a C compiler (`cc`, already required to build huck), the existing `tests/scripts/*_diff_check.sh` smoke-harness convention.

## Global Constraints

- Commit trailer on every commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on branch `v217-bash-test-helpers` (create from `main`; do NOT push or merge without confirmation).
- **No crate/source code changes** — this iteration touches only `tests/bash-test-suite/runner.sh`, `tests/bash-test-suite/README.md`, `tests/scripts/bash_test_suite_runner_diff_check.sh`, and `docs/bash-test-suite-baseline.md`.
- **GPL posture:** never vendor `recho.c`/`zecho.c`/`printenv.c` or their binaries; compile at runtime from the operator-supplied `$BASH_SOURCE_DIR`. Never paste verbatim bash `.right`/helper output into committed docs — describe divergences in huck-authored prose.
- Exact compile invocation (verified working on `cc` 13.3.0): for each helper `h` in `recho zecho printenv`,
  `"$CC" -I"$BASH_SOURCE_DIR" -I"$BASH_SOURCE_DIR/include" -I"$BASH_SOURCE_DIR/builtins" -include string.h -o "$dir/$h" "$BASH_SOURCE_DIR/support/$h.c"`
  (`CC` defaults to `cc`; `-include string.h` is required because `printenv.c` calls `strlen` without including it and modern gcc treats implicit declarations as a hard error).
- Override env var name: `HUCK_BASH_TEST_HELPERS` (a dir containing pre-built `recho`/`zecho`/`printenv`).
- Graceful degradation: missing compiler, missing `support/*.c`, or a failed compile must produce a `warning:` on stderr and let the sweep continue — the runner must never abort over helper provisioning.
- Verified expected outcome: with helpers provisioned, category `dollars` is PASS (was FAIL on `command not found: recho`), and no category's diff contains `command not found: recho/zecho/printenv`.

---

### Task 1: Compile and provide the helper binaries in the runner preflight

**Files:**
- Modify: `tests/bash-test-suite/runner.sh` (insert a new preflight section after the huck-binary check at line 53, before the `# ---- Scratch dir ----` section at line 55)
- Test: `tests/scripts/bash_test_suite_runner_diff_check.sh` (extend the existing smoke harness)

**Interfaces:**
- Consumes: `$BASH_SOURCE_DIR` (already required by the runner), optional `$HUCK_BASH_TEST_HELPERS`, optional `$CC`.
- Produces: a `PATH` (exported in the runner process) that includes a directory containing `recho`/`zecho`/`printenv` when provisioning succeeds. No new functions exposed to other scripts.

- [ ] **Step 1: Write the failing test — extend the smoke harness to require `dollars` PASS**

The smoke harness `tests/scripts/bash_test_suite_runner_diff_check.sh` currently runs the `arith` category and checks the output is well-formed. Add a second assertion that the `dollars` category is now `PASS` (it only reaches this code when `$BASH_SOURCE_DIR` is set, so CI without bash sources still SKIPs). Insert, just before the final `arith` success `echo`/`exit 0` (the block that prints `PASS [bash_test_suite_runner_diff_check] ...`), the following:

```bash
# v217: dollars exercises recho heavily; it PASSes only when the runner
# provisions the recho/zecho/printenv helpers. This asserts the helper
# provisioning end-to-end.
DOUT=$(HUCK_BASH_TEST_CATEGORY=dollars bash tests/bash-test-suite/runner.sh 2>&1)
drc=$?
if [ "$drc" -ne 0 ]; then
    echo "FAIL: runner exited $drc for dollars"
    echo "$DOUT"
    exit 1
fi
dstatus=$(printf '%s\n' "$DOUT" | awk -F'|' '/^\| dollars / { gsub(/ /, "", $3); print $3 }')
if [ "$dstatus" != "PASS" ]; then
    echo "FAIL: dollars=$dstatus (expected PASS) — recho/zecho/printenv helpers not provisioned?"
    echo "$DOUT"
    exit 1
fi
echo "PASS [bash_test_suite_runner_diff_check] helpers provisioned (dollars=PASS)"
```

- [ ] **Step 2: Run the smoke harness to verify it fails**

Run: `export BASH_SOURCE_DIR=/tmp/bash-5.2.21 && bash tests/scripts/bash_test_suite_runner_diff_check.sh`
Expected: FAIL — `dollars=FAIL (expected PASS)` (because the runner does not yet provide `recho`, so `dollars.tests` hits `command not found: recho`).

(If `/tmp/bash-5.2.21` is not present, extract it first: `curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp`.)

- [ ] **Step 3: Implement helper provisioning in the runner**

In `tests/bash-test-suite/runner.sh`, insert this section after line 53 (the `if [ ! -x "$HUCK" ]; then … fi` block) and before `# ---- Scratch dir ----`:

```sh
# ---- Provision test helpers (recho / zecho / printenv) -------------
# bash's .tests invoke these standalone helper programs (NOT builtins);
# bash builds them from support/*.c. We compile them at runtime from the
# operator-supplied $BASH_SOURCE_DIR (nothing vendored; GPL posture) into
# an ephemeral dir and prepend it to PATH for the category runs. An
# operator may instead point HUCK_BASH_TEST_HELPERS at a pre-built dir.
# If no compiler is available, we warn and continue (categories that need
# the helpers stay FAIL, exactly as before) — provisioning never aborts
# the sweep.
HELPER_DIR=""
HELPERS="recho zecho printenv"

if [ -n "${HUCK_BASH_TEST_HELPERS:-}" ]; then
    # Override: use a pre-built dir if it has all three executables.
    # (subshell so the `exit 1` aborts only the check, not the runner)
    if ( for h in $HELPERS; do [ -x "$HUCK_BASH_TEST_HELPERS/$h" ] || exit 1; done ); then
        HELPER_DIR="$HUCK_BASH_TEST_HELPERS"
    else
        echo "warning: HUCK_BASH_TEST_HELPERS=$HUCK_BASH_TEST_HELPERS is missing one of: $HELPERS; compiling from source instead." >&2
    fi
fi

if [ -z "$HELPER_DIR" ]; then
    CC="${CC:-cc}"
    if ! command -v "$CC" >/dev/null 2>&1; then
        echo "warning: no C compiler ('$CC') found; test helpers (recho/zecho/printenv) unavailable. Categories needing them will FAIL. Set HUCK_BASH_TEST_HELPERS to a pre-built dir or install a compiler." >&2
    elif [ ! -f "$BASH_SOURCE_DIR/support/recho.c" ]; then
        echo "warning: $BASH_SOURCE_DIR/support/recho.c not found; cannot build test helpers. Categories needing them will FAIL." >&2
    else
        built_dir=$(mktemp -d -t "huck-bash-helpers.XXXXXX")
        inc="-I$BASH_SOURCE_DIR -I$BASH_SOURCE_DIR/include -I$BASH_SOURCE_DIR/builtins"
        for h in $HELPERS; do
            # -include string.h: printenv.c uses strlen without including it,
            # which modern gcc treats as a hard error.
            if ! "$CC" $inc -include string.h -o "$built_dir/$h" "$BASH_SOURCE_DIR/support/$h.c" 2>"$built_dir/$h.log"; then
                echo "warning: failed to compile test helper '$h' (see $built_dir/$h.log); categories needing it will FAIL." >&2
            fi
        done
        # Use the dir if anything compiled (a partial build still helps).
        if [ -n "$(ls -A "$built_dir" 2>/dev/null | grep -vE '\.log$')" ]; then
            HELPER_DIR="$built_dir"
        fi
    fi
fi

if [ -n "$HELPER_DIR" ]; then
    PATH="$HELPER_DIR:$PATH"
    export PATH
fi
```

- [ ] **Step 4: Run the smoke harness to verify it passes**

Run: `export BASH_SOURCE_DIR=/tmp/bash-5.2.21 && bash tests/scripts/bash_test_suite_runner_diff_check.sh`
Expected: PASS — both `arith` well-formedness and `helpers provisioned (dollars=PASS)`.

- [ ] **Step 5: Verify graceful degradation and the override path**

Run (degrade — no compiler):
`export BASH_SOURCE_DIR=/tmp/bash-5.2.21 && CC=/nonexistent-cc HUCK_BASH_TEST_CATEGORY=dollars bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "warning: no C compiler|^\| dollars"`
Expected: a `warning: no C compiler …` line on stderr AND a `| dollars | FAIL |` row — i.e. the runner completes (does not abort) and dollars falls back to FAIL.

Run (override):
```bash
mkdir -p /tmp/hbh && for h in recho zecho printenv; do \
  cc -I/tmp/bash-5.2.21 -I/tmp/bash-5.2.21/include -I/tmp/bash-5.2.21/builtins -include string.h \
     -o /tmp/hbh/$h /tmp/bash-5.2.21/support/$h.c; done
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
HUCK_BASH_TEST_HELPERS=/tmp/hbh HUCK_BASH_TEST_CATEGORY=dollars bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "^\| dollars"
```
Expected: `| dollars | PASS |` (the override dir was used; no compile-from-source needed).

- [ ] **Step 6: Commit**

```bash
git add tests/bash-test-suite/runner.sh tests/scripts/bash_test_suite_runner_diff_check.sh
git commit -m "v217 task 1: runner provisions recho/zecho/printenv test helpers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Document helper provisioning in the README

**Files:**
- Modify: `tests/bash-test-suite/README.md`

**Interfaces:**
- Consumes: nothing. Produces: documentation only.

- [ ] **Step 1: Read the README to match its style**

Run: `sed -n '1,80p' tests/bash-test-suite/README.md` and note the existing section structure (Licensing, One-time setup, Running the survey, env-var table).

- [ ] **Step 2: Add a "Test helper binaries" subsection**

Add a section (place it after "One-time setup", before/near "Running the survey") with this content (adjust headings to match the doc's existing level):

```markdown
## Test helper binaries (recho / zecho / printenv)

Many bash `.tests` invoke three helper programs — `recho`, `zecho`,
`printenv` — that are NOT bash builtins but standalone C programs in bash's
own test tooling (`support/recho.c`, `support/zecho.c`, `support/printenv.c`).
The runner compiles them at runtime from `$BASH_SOURCE_DIR/support/*.c` into
an ephemeral temp dir and adds that dir to `PATH` for the category runs.
Nothing is vendored — they are built from the operator-supplied bash source,
the same posture as the `.tests`/`.right` files.

Requirements: a C compiler (`cc` by default; override with `$CC`) — the same
toolchain already needed to build huck.

Override: set `HUCK_BASH_TEST_HELPERS` to a directory containing pre-built
`recho`/`zecho`/`printenv` executables and the runner will use those instead
of compiling.

If no compiler is available and no override is set, the runner prints a
warning and continues; the ~21 categories that invoke these helpers will FAIL
on `command not found` (as they did before this support existed), but the rest
of the sweep is unaffected.
```

Also add a row to the env-var table (if one exists) for `HUCK_BASH_TEST_HELPERS`.

- [ ] **Step 3: Verify the README renders sensibly**

Run: `sed -n '1,120p' tests/bash-test-suite/README.md`
Expected: the new section reads cleanly and the env-var table (if present) lists `HUCK_BASH_TEST_HELPERS`.

- [ ] **Step 4: Commit**

```bash
git add tests/bash-test-suite/README.md
git commit -m "v217 task 2: document test-helper provisioning in README

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Re-triage the baseline doc against the full sweep

**Files:**
- Modify: `docs/bash-test-suite-baseline.md`

**Interfaces:**
- Consumes: the runner with helper provisioning (Task 1). Produces: documentation only.

- [ ] **Step 1: Run the full sweep with helpers provisioned**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh > /tmp/v217-sweep.md 2>/tmp/v217-sweep.err
```
This rebuilds huck, compiles the helpers, and runs all ~82 categories (several minutes; categories with deliberate sleeps may TIMEOUT as before). The Markdown summary (counts + per-category table) is in `/tmp/v217-sweep.md`; per-category diffs are in the printed `/tmp/huck-bash-tests-*/` scratch dir.

- [ ] **Step 2: Confirm the helper unblock landed**

Run:
```bash
D=$(grep -oE '/tmp/huck-bash-tests-[^ ]+' /tmp/v217-sweep.md | head -1)
grep -rl "command not found: recho\|command not found: zecho\|command not found: printenv" "$D"/*.diff 2>/dev/null | wc -l
grep -E '^\| (dollars|nquote5) ' /tmp/v217-sweep.md
```
Expected: `0` diffs still containing the helper command-not-found lines; `dollars` and `nquote5` rows show `PASS`.

- [ ] **Step 3: Update the baseline Summary counts**

In `docs/bash-test-suite-baseline.md`, update the `## Summary` bullet counts (Categories run / PASS / FAIL / TIMEOUT / ERROR / SKIP) to match `/tmp/v217-sweep.md`'s summary block, and update the `huck commit:` and `Sweep date:` header lines (use `git rev-parse --short HEAD` and the sweep date).

- [ ] **Step 4: Re-triage the per-category Notes for the unblocked categories**

For each previously helper-blocked category — `array2, braces, comsub, comsub-posix, dollars, exp-tests, glob-test, ifs, iquote, more-exp, new-exp, nquote, nquote1, nquote2, nquote3, nquote4, nquote5, posixexp, rhs-exp, tilde2, varenv` — update its row:
- If now PASS: set Status to PASS and clear/trim the Note.
- If still FAIL: replace the "`recho`/`zecho` not found" wording in the Note with a huck-authored, prose description of the *real* remaining divergence, derived by reading that category's `.diff` in the scratch dir. **Do not paste verbatim bash `.right` or helper output** — describe what differs (e.g. "high-byte character shown as a different escape", "field-splitting of an adjacent IFS run diverges"). Keep notes concise.

Read each relevant diff to inform the Note, e.g.:
```bash
sed -n '1,40p' "$D/nquote.diff"
```

- [ ] **Step 5: Sanity-check the doc**

Run: `grep -ciE "recho|zecho" docs/bash-test-suite-baseline.md`
Expected: `0` (no remaining references to the helper binaries as a blocker — they're provisioned now). If any remain, they should only be in a historical/explanatory context; otherwise remove them.

- [ ] **Step 6: Commit**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "v217 task 3: re-triage bash test-suite baseline after helper provisioning

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- Helper provisioning in preflight (spec §1) → Task 1, Step 3. ✓
- PATH wiring for category runs (spec §2) → Task 1, Step 3 (`export PATH`). ✓
- `HUCK_BASH_TEST_HELPERS` override (spec §1) → Task 1, Steps 3 & 5. ✓
- Graceful degradation (spec Goals/Risks) → Task 1, Steps 3 & 5. ✓
- Compile details incl. `-include string.h` and include paths (spec §1) → Global Constraints + Task 1, Step 3. ✓
- Licensing posture (spec §4) → Global Constraints + Task 2 wording. ✓
- README update (spec §6) → Task 2. ✓
- Baseline re-triage (spec §5) → Task 3. ✓
- No crate code changes (spec Non-goals) → Global Constraints. ✓
- Lifetime/cleanup via ephemeral mktemp dir (spec §3) → Task 1, Step 3 (`mktemp -d`). ✓

**Placeholder scan:** No "TBD"/"handle edge cases" — every step has concrete code/commands. Task 3 Step 4 requires per-category judgment (reading diffs) by nature, but gives the exact category list, the method (read the `.diff`), and the licensing constraint, not a vague "describe divergences."

**Type/consistency:** The env var (`HUCK_BASH_TEST_HELPERS`), helper list (`recho zecho printenv`), and compile flags are identical across the spec, Global Constraints, and Tasks 1–2. The smoke-harness assertion (Task 1) and the sweep confirmation (Task 3 Step 2) both key on `dollars=PASS` / no `command not found: recho`, consistent with the verified outcome.

**Note for the executor:** Tasks 2 and 3 are documentation; their "tests" are the `sed`/`grep` sanity checks shown, not cargo tests. Do not run `cargo test` for this iteration — no crate code changes. The gold-standard verification is Task 1's smoke harness (`dollars=PASS`) plus Task 3's full-sweep confirmation.
