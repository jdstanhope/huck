# v280 Diff-Check CI Sweep + Normalizer Fixes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run the `tests/scripts/*_diff_check.sh` bash-diff harness sweep in CI, and fix the 4 stale program-name normalizers so the sweep is green.

**Architecture:** A committed runner script (`run_diff_checks.sh`) executes every diff-check harness against its own default binary and exits non-zero on any red. CI builds release huck (debug already built by the test step) and invokes it. Four harnesses have stale normalizers that don't strip huck's bash-faithful full-path error prefix — fixed with a path-robust `sed`. The one real deferred failure (#109) is quarantined by a surgical in-harness XFAIL.

**Tech Stack:** Bash, GitHub Actions (`ubuntu-24.04`), Rust/cargo (build only, no source change).

## Global Constraints

- **No Rust source or shell behavior changes** — this is test-harness + CI only. `huck`'s error output is already bash-correct; the harnesses are wrong.
- **Every commit** ends with the trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Do not push to main, do not merge.** Work on branch `v280-diff-check-ci-sweep`; the PR (`Closes #110`) is for the user to merge.
- **The sweep runner never overrides `HUCK_BIN`** — each harness must run against its intended default binary (161 → `target/debug/huck`, 8 → `target/release/huck`). This is the whole point: forcing one binary produced the funcnest false-failure.
- **Both binaries must exist before running the sweep**: `cargo build --locked --bin huck` (debug) and `cargo build --release --locked --bin huck` (release).
- Canonical path-robust prefix match: `([^:]*/)?(bash|huck): (line [0-9]+: )?` — anchored at `^`; `[^:]*` stops at the first `:`; local binary paths contain no `:`.

---

### Task 1: Fix the 4 stale program-name normalizers

**Files:**
- Modify: `tests/scripts/shift_range_diff_check.sh:15`
- Modify: `tests/scripts/indirect_unset_positional_diff_check.sh:17`
- Modify: `tests/scripts/trap_kill_stop_diff_check.sh:14`
- Modify: `tests/scripts/loop_levels_diff_check.sh:30`

**Interfaces:**
- Consumes: nothing.
- Produces: 4 harnesses that exit 0 against their default binary. Later tasks (runner, CI) rely on these being green.

**Background:** Each harness compares bash (invoked bare → prefix `bash: `) against huck (invoked by absolute path → prefix `/…/target/debug/huck: `), normalizing the prefix away first. The huck arm currently strips only a bare `^huck: `, so the absolute-path prefix survives and every error-message case mismatches. huck is bash-correct (`/usr/bin/bash -c 'shift abc'` also prints its full path). Fix: make the program-name arm tolerate an optional leading path.

- [ ] **Step 1: Confirm the 4 harnesses currently fail**

Both binaries must be built first (see Global Constraints). Then:

```bash
for h in shift_range indirect_unset_positional trap_kill_stop loop_levels; do
  bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 && echo "$h PASS" || echo "$h FAIL"
done
```
Expected: all four print `FAIL`.

- [ ] **Step 2: Fix `shift_range_diff_check.sh` line 15**

Replace:
```bash
norm() { sed -E 's/^(bash|huck): (line [0-9]+: )?//'; }
```
with:
```bash
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
```

- [ ] **Step 3: Fix `indirect_unset_positional_diff_check.sh` line 17**

Replace:
```bash
norm() { sed -E 's/^(bash|huck): (line [0-9]+: )?//'; }
```
with:
```bash
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
```

- [ ] **Step 4: Fix `trap_kill_stop_diff_check.sh` line 14**

Replace:
```bash
norm() { sed -E 's/^(bash|huck): (line [0-9]+: )?//; s/\bSIG([A-Z])/\1/g'; }
```
with:
```bash
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##; s/\bSIG([A-Z])/\1/g'; }
```

- [ ] **Step 5: Fix `loop_levels_diff_check.sh` line 30**

Replace (inside `run_normalized`):
```bash
        | sed 's/^bash: line [0-9]*: /SHELL: /g; s/^huck: /SHELL: /g'
```
with:
```bash
        | sed -E 's#^([^:]*/)?bash: (line [0-9]+: )?#SHELL: #g; s#^([^:]*/)?huck: (line [0-9]+: )?#SHELL: #g'
```
(This also strips huck's `line N:` — which huck now emits for stdin scripts — so it matches bash's already-stripped `SHELL:` form.)

- [ ] **Step 6: Verify all four now pass**

```bash
for h in shift_range indirect_unset_positional trap_kill_stop loop_levels; do
  bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 && echo "$h PASS" || echo "$h FAIL"
done
```
Expected: all four print `PASS`.

- [ ] **Step 7: Commit**

```bash
git add tests/scripts/shift_range_diff_check.sh tests/scripts/indirect_unset_positional_diff_check.sh tests/scripts/trap_kill_stop_diff_check.sh tests/scripts/loop_levels_diff_check.sh
git commit -m "test: path-robust prog-name normalizer in 4 diff-check harnesses (#110)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Surgical XFAIL for the #109 case in cmdsub_comment

**Files:**
- Modify: `tests/scripts/cmdsub_comment_diff_check.sh` (add `xfail()` after the `check()` definition at line 16; change the `check "comment after open"` call at lines 20–21 to `xfail`).

**Interfaces:**
- Consumes: the existing `PASS`/`FAIL` counters and `HUCK_BIN`.
- Produces: a harness that exits 0 while keeping its other 7 cases hard-asserted, and that self-flags if #109 is silently fixed.

**Background:** The `comment after open` case (a `#` comment inside `$()` whose `)` must not close the substitution) is a real deferred parser bug — bash prints `[yo]` rc 0, huck errors rc 2. Tracked in #109. Quarantine it surgically so the file goes green without dropping the other 7 cases.

- [ ] **Step 1: Confirm the harness currently fails on exactly this case**

```bash
bash tests/scripts/cmdsub_comment_diff_check.sh; echo "exit=$?"
```
Expected: a `FAIL: comment after open` line, `Total: 8, Pass: 7, Fail: 1`, `exit=1`.

- [ ] **Step 2: Add the `xfail()` function**

Immediately after the `check()` function's closing `}` (line 16), insert:
```bash
# Expected-fail: huck currently diverges from bash on this input (tracked in
# #109 — comment inside $() ). Passes the harness while it stays broken, and
# self-flags the day it is silently fixed so we restore check() and close #109.
xfail() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" != "$h" ]]; then printf 'XFAIL: %s (#109)\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s unexpectedly passes — close #109 and restore check()\n' "$label"; FAIL=$((FAIL+1)); fi
}
```

- [ ] **Step 3: Convert the `comment after open` case to `xfail`**

Change the call at lines 20–21 from `check` to `xfail` (only the command name changes; the label and fragment are unchanged):
```bash
xfail "comment after open"    'echo "[$(# c with ) paren
echo yo)]"'
```

- [ ] **Step 4: Verify the harness is now green and prints XFAIL**

```bash
bash tests/scripts/cmdsub_comment_diff_check.sh; echo "exit=$?"
```
Expected: an `XFAIL: comment after open (#109)` line, no `FAIL:` line, `Total: 8, Pass: 8, Fail: 0`, `exit=0`.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/cmdsub_comment_diff_check.sh
git commit -m "test: surgical XFAIL for comment-in-\$() case, quarantine #109 (#110)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Create the committed sweep runner

**Files:**
- Create: `tests/scripts/run_diff_checks.sh` (executable).

**Interfaces:**
- Consumes: `target/debug/huck` and `target/release/huck` (built by caller); the green harnesses from Tasks 1–2.
- Produces: an executable the CI job and developers invoke. Exit 0 iff every harness (except the excluded bash-test-suite runner) is green.

- [ ] **Step 1: Write the runner**

Create `tests/scripts/run_diff_checks.sh` with exactly:
```bash
#!/usr/bin/env bash
# Run every bash-diff harness against its DEFAULT binary. Green = all pass.
# Caller MUST build both binaries first:
#   cargo build --locked --bin huck            # target/debug/huck
#   cargo build --release --locked --bin huck  # target/release/huck
# Does NOT override HUCK_BIN (each harness picks its intended binary) and does
# NOT build or set ulimit — the caller owns that.
set -u
cd "$(dirname "$0")/../.." || exit 1   # repo root
for b in target/debug/huck target/release/huck; do
  [[ -x "$b" ]] || { echo "missing binary: $b — build it first" >&2; exit 1; }
done
pass=0; fail=0; failed=()
for h in tests/scripts/*_diff_check.sh; do
  name=$(basename "$h")
  case "$name" in
    run_diff_checks.sh|bash_test_suite_runner_diff_check.sh) continue ;;
  esac
  if timeout 120 bash "$h" >/dev/null 2>&1; then
    pass=$((pass+1)); echo "PASS $name"
  else
    fail=$((fail+1)); failed+=("$name"); echo "FAIL $name"
  fi
done
echo
echo "Diff-check sweep: $pass passed, $fail failed"
(( fail == 0 )) || { echo "Failed: ${failed[*]}" >&2; exit 1; }
```

- [ ] **Step 2: Make it executable**

```bash
chmod +x tests/scripts/run_diff_checks.sh
```

- [ ] **Step 3: Run the full sweep — expect all green**

Ensure both binaries are built, then:
```bash
cargo build --locked --bin huck
cargo build --release --locked --bin huck
tests/scripts/run_diff_checks.sh; echo "exit=$?"
```
Expected: a `PASS <name>` line per harness, final `Diff-check sweep: 180 passed, 0 failed`, `exit=0`. (180 = 181 `*_diff_check.sh` files minus the excluded `bash_test_suite_runner_diff_check.sh`.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/run_diff_checks.sh
git commit -m "test: add run_diff_checks.sh bash-diff sweep runner (#110)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Wire the sweep into CI and the iteration checklist

**Files:**
- Modify: `.github/workflows/ci.yml` (add two steps after the `Test (workspace)` step, line ~54).
- Modify: `CLAUDE.md` (add the sweep to the iteration verification step).

**Interfaces:**
- Consumes: `tests/scripts/run_diff_checks.sh` from Task 3.
- Produces: a CI gate; a documented per-iteration check.

- [ ] **Step 1: Inspect the current CI job tail**

```bash
sed -n '44,60p' .github/workflows/ci.yml
```
Note the `Test (workspace)` step is the last step in the `build & test` job and leaves `target/debug/huck` built.

- [ ] **Step 2: Append the release build + sweep steps**

After the `Test (workspace)` step (the `run: cargo test --workspace --locked --no-fail-fast --verbose` line, ~line 54), add, at the same indentation as the other `- name:` steps:
```yaml
      - name: Build release huck (release-default harnesses)
        run: cargo build --release --locked --bin huck

      - name: Bash-diff harness sweep
        run: tests/scripts/run_diff_checks.sh
```

- [ ] **Step 3: Validate the YAML parses**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('YAML OK')"
```
Expected: `YAML OK`.

- [ ] **Step 4: Add the sweep to the CLAUDE.md iteration loop**

In `CLAUDE.md`, in the numbered iteration loop under "When the user says start vNN", in step 5 (Final review) or step 6 (Open a pull request), add a bullet requiring the sweep. Locate the "Final review" line:
```bash
grep -n 'Final review\|Before opening the PR' CLAUDE.md
```
Under step 5, add this bullet (matching the surrounding list style):
```markdown
   - Run the bash-diff sweep before the PR: build both binaries
     (`cargo build --locked --bin huck` + `cargo build --release --locked
     --bin huck`) then `tests/scripts/run_diff_checks.sh`; it must be green.
     (CI runs it too, but catch regressions locally first.)
```

- [ ] **Step 5: Verify the CLAUDE.md edit landed**

```bash
grep -n 'run_diff_checks.sh' CLAUDE.md
```
Expected: one match inside the iteration loop.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/ci.yml CLAUDE.md
git commit -m "ci: run the bash-diff harness sweep on every build; document in iteration loop (#110)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] Run the whole sweep once more from a clean state:
```bash
cargo build --locked --bin huck && cargo build --release --locked --bin huck
tests/scripts/run_diff_checks.sh; echo "exit=$?"
```
Expected: `Diff-check sweep: 180 passed, 0 failed`, `exit=0`.

- [ ] Confirm no Rust source changed (test-harness + CI + docs only):
```bash
git diff --stat main -- 'crates/**/*.rs'
```
Expected: empty output.

## Notes for the whole-branch review

- No `cargo test` / `cargo fmt` impact (no `.rs` changes) — do not require them.
- The funcnest harness is deliberately untouched: it is not a bug (debug-stack artifact), documented in the spec.
- #109 stays open and quarantined; the XFAIL self-flags if it is silently fixed.
