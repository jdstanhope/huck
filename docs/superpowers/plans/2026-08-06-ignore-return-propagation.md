# v356 — one exempt scope, propagated — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the "this failure is ignored" exemption survive a fork ([#1](https://github.com/jdstanhope/huck/issues/1)'s remaining third) and collapse the several mechanisms that answer that one question into one — with a **net-negative diff**.

**Architecture:** `clear_for_subshell` stops discarding the caller's exemption. `run_andor_group`'s two near-identical arms collapse into one `run_list_element` that owns the exempt scope end-to-end — around the command, the interrupt checkpoint, control-flow propagation and the epilogue — which lets `finish_command` drop its `is_last` parameter and decide from suppression state instead.

**Tech Stack:** Rust 2024, crate `huck-engine`. Tests are `#[cfg(test)] mod tests` blocks, `tests/*.rs` integration binaries, and `tests/scripts/*_diff_check.sh` bash-differential harnesses.

**Spec:** `docs/superpowers/specs/2026-08-06-ignore-return-propagation-design.md` — §3 carries the measured subshell-inheritance table; read it before touching `clear_for_subshell`.

## Global Constraints

- **The diff must be NET NEGATIVE.** `git diff --stat main -- crates/` at the end must show more deletions than insertions. This is an acceptance criterion: an implementation that passes every test and adds lines has failed. The spike measured **−18**.
- **No expected-value edits** in `err_trap_compound` (30), `err_trap_function` (24), `set_e_andor` (34), `negated_errexit`, `trap_action_exit` (28), `arith_expansion_discard`. All six passed under the spike; if one needs its expectations changed, the change went further than the spec.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit; CI enforces `--check`.
- **This box has 1 core and 1.9 GB.** Never `cargo test --workspace` — it OOM-kills the session. Build with `cargo build -p huck --bin huck`.
- **Engine lib tests run with 4 threads:** `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4`.
- **The full sweep exceeds the 10-minute Bash-tool cap.** Run it backgrounded to a file, then block on `until grep -q "EXIT=" <file>; do sleep 15; done`.
- **Job-control harnesses are load-flaky (#476).** Check a sweep failure against `main` by run-count before calling it a regression.
- **Branch:** `v356-exempt-scope`, cut from `main` at or after `d8373bbb`. Never push to `main`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/huck-engine/src/traps.rs` | subshell trap reset | Delete the two suppression-clearing lines (−2) |
| `crates/huck-engine/src/executor.rs` | and-or dispatch + epilogue | Add `run_list_element`; collapse `run_andor_group`'s two arms into it; drop `is_last` from `finish_command` (net −16) |
| `tests/scripts/errexit_err_suppression_diff_check.sh` | the contract | Add the fork-family rows |

---

### Task 1: The fork family — stop discarding the exemption

The whole user-visible bug, in two deleted lines. Done first and alone so the behaviour change is separable from the refactor that follows.

**Files:**
- Modify: `tests/scripts/errexit_err_suppression_diff_check.sh`
- Modify: `crates/huck-engine/src/traps.rs` (`clear_for_subshell`)

**Interfaces:**
- Consumes: nothing (first task)
- Produces: nothing for later tasks — Task 2 is a pure refactor on top

- [ ] **Step 1: Add the failing rows**

Append to `tests/scripts/errexit_err_suppression_diff_check.sh`, immediately before the final `echo ""` summary block:

```bash
# --- the exemption must survive a FORK (#1's subshell third) ---------------
# A subshell inherits the entire option set (measured: `$-` is byte-identical
# parent to child, including -e/-u/-x/-m). The caller's "this failure does not
# count" travels the same way; `clear_for_subshell` used to discard it.
check "fork: subshell via ||"   'set -e; ( false; echo x ) || echo or'
check "fork: subshell via &&"   'set -e; ( false; echo x ) && echo and; echo after'
check "fork: in a function"     'set -e; f() { ( false; echo x ); }; f || echo or'
check "fork: if condition"      'set -e; if ( false; echo x ); then :; fi; echo after'
check "fork: nested subshells"  'set -e; ( ( false; echo x ) ) || echo or'
check "fork: ERR trap via ||"   'trap "echo E" ERR; ( false; echo x ) || echo or'
check "fork: ERR trap via &&"   'trap "echo E" ERR; ( false; echo x ) && echo and'
# NOT exempt — the subshell is the last command of its list, so it still counts:
check "fork: plain subshell"    'set -e; ( false ); echo after'
check "fork: plain, body runs"  'set -e; ( false; echo x ); echo after'
check "fork: ERR trap plain"    'trap "echo E" ERR; ( false )'
```

- [ ] **Step 2: Run it to verify the new rows fail**

Run:
```bash
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | grep -E "^FAIL|^Total" )
```
Expected: the seven `fork:` exempt rows FAIL; the three "NOT exempt" rows PASS; the pre-existing 32 still pass. If a "NOT exempt" row fails now, stop — the baseline is not what the spec describes.

- [ ] **Step 3: Delete the two lines**

In `crates/huck-engine/src/traps.rs`, in `clear_for_subshell`, delete:

```rust
    shell.errexit_suppressed_depth = 0;
    shell.err_trap_suppressed_depth = 0;
```

and extend the function's doc comment with the reason, so nobody re-adds them:

```rust
/// Resets all trap state in a freshly-forked subshell child. POSIX: trapped
/// signals reset to their original values in subshells; we also clear EXIT so
/// the parent's EXIT trap fires only when the parent exits.
///
/// v356 (#1): this does NOT touch the suppression counters. A subshell
/// inherits the shell's entire option set — `$-` is byte-identical parent to
/// child, including `-e`/`-u`/`-x`/`-m` — and the caller's "this command's
/// failure does not count" travels the same way, which is why
/// `set -e; ( false; echo x ) || echo or` prints `x` in bash. Clearing them
/// here made every exempt context correct in-process and wrong across a fork.
/// The counters were swept in originally because the field was named
/// `err_suppressed_depth` and sat beside the trap fields.
```

- [ ] **Step 4: Run the harness to verify it passes**

Run:
```bash
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | tail -3 )
```
Expected: **42/42, Fail: 0**.

- [ ] **Step 5: Check the six regression harnesses**

```bash
for h in err_trap_compound err_trap_function set_e_andor negated_errexit trap_action_exit arith_expansion_discard; do
  printf '%-26s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 2>&1 | grep "^test result" )
```

Expected: all green, no expected-value edits.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "fix(#1): the exempt scope survives a fork

clear_for_subshell zeroed both suppression counters, so a forked child threw
away the exemption its caller had established. Every exempt context was
correct in-process (since v355) and wrong across a fork:

    set -e; ( false; echo x ) || echo or     bash: x    huck: or

A subshell inherits the shell's ENTIRE option set — \$- is byte-identical
parent to child, including -e/-u/-x/-m — so the caller's \"this failure does
not count\" belongs with it. What a subshell resets is caught trap
dispositions, and suppression is not a trap; it was swept in because the field
was named err_suppressed_depth and sat beside the trap fields.

10 new rows in errexit_err_suppression_diff_check.sh (42 total): the seven
exempt fork shapes, plus three that must NOT change because the subshell is
the last command of its list.

Closes #1

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: One owner for the per-element sequence

Pure refactor, no behaviour change, and where the net-negative diff is earned.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`run_andor_group`, `finish_command`)

**Interfaces:**
- Consumes: `Shell::{suppress_both, unsuppress_both}` and `finish_command` (v355)
- Produces: `fn run_list_element(cmd: &Command, exempt: bool, shell: &mut Shell) -> Result<ExecOutcome, ExecOutcome>`; `finish_command` loses its `is_last` parameter

- [ ] **Step 1: Add the leak test**

Add to `crates/huck-engine/src/executor/errexit_andor_tests.rs`:

```rust
/// v356: `run_list_element` owns the exempt scope end-to-end, so no early
/// return can leak it. A leak would make `set -e` silently stop working for
/// the rest of the shell's life, which no differential harness would localise.
#[test]
fn exempt_scope_never_leaks_out_of_a_list() {
    for src in [
        "false || true",
        "false && true",
        "true && false || true",
        "{ false; } || true",
        "f() { return 3; }; f || true",
        "( false ) || true",
        "false | true || true",
    ] {
        let mut s = Shell::new();
        let _ = crate::shell::process_line(src, &mut s, false);
        assert!(
            !s.errexit_suppressed(),
            "errexit suppression leaked after `{src}`"
        );
        assert!(
            !s.err_trap_suppressed(),
            "ERR-trap suppression leaked after `{src}`"
        );
    }
}
```

- [ ] **Step 2: Run it — it must pass BEFORE the refactor**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 exempt_scope_never_leaks`
Expected: PASS. This is a characterisation test: it pins today's behaviour so the refactor cannot quietly break it.

- [ ] **Step 3: Replace `run_andor_group` with the collapsed pair**

Replace the whole of `run_andor_group` (from `fn run_andor_group(` through its closing `}` after `    status`) with:

```rust
/// Runs ONE element of an and-or list: the exempt scope, the command, the
/// interrupt checkpoint, control-flow propagation, and the post-command
/// epilogue. `Err(outcome)` means the caller must return it immediately;
/// `Ok(status)` means carry on with the list.
///
/// `exempt` is bash's ignore-return: an element that is NOT the syntactically
/// last of its list is "part of a list being tested", so neither it nor
/// anything it runs counts. The scope therefore spans the body AND the
/// epilogue — which is what lets the epilogue decide from suppression state
/// rather than a separate `is_last` flag.
///
/// Owning both ends here is also what makes the scope leak-proof: there is one
/// exit path. Previously the raise and the lower straddled five early returns,
/// and leaking the depth would have made `set -e` silently stop working for
/// the rest of the list.
fn run_list_element(
    cmd: &Command,
    exempt: bool,
    shell: &mut Shell,
) -> Result<ExecOutcome, ExecOutcome> {
    // #444 (bash's `was_error_trap`): snapshot BEFORE the command runs, so a
    // command that INSTALLS the ERR trap is not itself caught by it.
    let err_armed = crate::traps::err_trap_armed(shell);
    if exempt {
        shell.suppress_both();
    }
    let status = run_command(cmd, shell);
    let out = 'elem: {
        if let Some(o) = check_interrupt(shell) {
            break 'elem Err(o);
        }
        if matches!(
            status,
            ExecOutcome::Exit(_)
                | ExecOutcome::LoopBreak(_, _)
                | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_)
                | ExecOutcome::Interrupted(_)
        ) {
            break 'elem Err(status);
        }
        if let ExecOutcome::Continue(c) = status
            && let Some(o) = finish_command(cmd, c, err_armed, shell)
        {
            break 'elem Err(o);
        }
        Ok(status)
    };
    if exempt {
        shell.unsuppress_both();
    }
    out
}

fn run_andor_group(
    first: &Command,
    rest: &[(Connector, &Command)],
    shell: &mut Shell,
) -> ExecOutcome {
    // An element is exempt iff it is not the syntactically last of the list.
    let mut status = match run_list_element(first, !rest.is_empty(), shell) {
        Ok(s) => s,
        Err(o) => return o,
    };
    for i in 0..rest.len() {
        let (connector, command) = &rest[i];
        let should_run = match connector {
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
            // Semi/Amp are group boundaries; they never appear inside a group.
            Connector::Semi | Connector::Amp => true,
        };
        if should_run {
            status = match run_list_element(command, i + 1 != rest.len(), shell) {
                Ok(s) => s,
                Err(o) => return o,
            };
        }
    }
    status
}
```

- [ ] **Step 4: Drop `is_last` from `finish_command`**

Change the signature:

```rust
fn finish_command(
    cmd: &Command,
    c: i32,
    err_armed: bool,
    shell: &mut Shell,
) -> Option<ExecOutcome> {
```

and the fire gate, deleting the `is_last &&` term:

```rust
    if c != 0 && !shell.err_trap_suppressed() && !is_negated_pipeline(cmd) {
```

Update the doc comment above `finish_command`: the `is_last` paragraph is replaced by "an exempt element runs inside a suppression scope raised by `run_list_element`, so the epilogue reads that state instead of a flag". Leave the `err_armed` paragraph alone.

- [ ] **Step 5: Build and run the leak test plus the contract**

```bash
cargo fmt --all
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | tail -3 )
for h in err_trap_compound err_trap_function set_e_andor negated_errexit trap_action_exit arith_expansion_discard; do
  printf '%-26s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
```

Expected: engine lib green, contract **42/42**, all six harnesses green, **no expected-value edits**.

- [ ] **Step 6: Check the diff is negative**

Run: `git diff --stat main -- crates/`
Expected: deletions exceed insertions. If not, the refactor did not earn its place — the most likely cause is reintroducing the duplicated early-return block instead of using the labelled block. Fix before committing rather than after.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(#198): one owner for the and-or element sequence

run_andor_group carried two near-copies of snapshot -> run -> interrupt check
-> control-flow propagation -> epilogue. They become one run_list_element that
also owns the exempt scope end-to-end.

Two things fall out. finish_command loses its is_last parameter: with the
scope spanning the epilogue, suppression state already carries the fact, so
four conditions on the fire gate become three. And the scope becomes
leak-proof — one exit path via a labelled block, where the raise and lower
previously straddled five early returns and a mistake would have disabled
set -e silently for the rest of the list.

No behavior change: contract 42/42, six ERR/errexit harnesses unchanged, and a
new unit test pins that no list shape leaves suppression raised.

Refs #198

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Verification, docs, blog, PR

- [ ] **Step 1: Full test + sweep**

```bash
cargo fmt --all && cargo fmt --all --check
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
for t in set_options_integration pipefail_integration subshell_integration \
         cmdsub_subshell_integration pipeline_subshell_integration \
         if_integration while_integration for_integration functions_integration \
         trap_integration; do
  printf '%-40s ' $t
  ( ulimit -v 1500000; timeout 500 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 | grep "test result" )
done
( ulimit -v 1500000; timeout 590 cargo build --release --locked --bin huck )
( ulimit -v 1500000; tests/scripts/run_diff_checks.sh > /tmp/v356-sweep.txt 2>&1; echo "EXIT=$?" >> /tmp/v356-sweep.txt )
# then: until grep -q "EXIT=" /tmp/v356-sweep.txt; do sleep 15; done
```

Expected: all green. A `coproc` / `job_notify` / `job_spec_percent` failure is #476 — verify against `main` by run-count, then re-run.

- [ ] **Step 2: bash-suite PASS-set diff**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
( ulimit -v 2500000; bash tests/bash-test-suite/runner.sh > /tmp/v356-suite.md 2>&1 )
git checkout main
( ulimit -v 2500000; bash tests/bash-test-suite/runner.sh > /tmp/main-suite.md 2>&1 )
git checkout v356-exempt-scope
head -4 /tmp/main-suite.md | grep -i commit    # MUST show main's sha, not the branch's
diff <(grep '| PASS' /tmp/main-suite.md | sort) <(grep '| PASS' /tmp/v356-suite.md | sort)
```

⚠️ Use `git checkout main`, **never `git stash`** — on a clean tree the stash is a no-op and the "baseline" is the branch, which produced a confidently wrong "identical" result in v355. The commit-stamp check above is the guard. `errexit`, `set-e` and the subshell categories are the ones that could move; movement in either direction must be explained in the PR. The runner rebuilds the release binary, so rebuild before capturing any before/after output.

- [ ] **Step 3: Update the architecture doc**

In `docs/architecture.md`, the errexit/ERR paragraph gains: the exempt scope is raised by `run_list_element` for any non-last element of an and-or list and spans the command, its body and the epilogue; it is inherited by a forked child, because a subshell inherits the shell's whole option set and the exemption travels with it; `clear_for_subshell` deliberately does not touch it.

- [ ] **Step 4: Blog entry**

`site/content/blog/<slug>.mdx`, frontmatter `title` (≤120), `date: 2026-08-06`, `summary` (≤300), `tags`, `version: "v356"`, `draft: false`. Lead with the user-visible symptom — `set -e; ( … ) || handler` killing the script inside the subshell — with real before/after from a pre-v356 binary built from `main`. The story worth telling is that the fix is a *deletion*: the shell already knew the answer and was throwing it away at the fork, and the mechanism was discarded because a field was named after the wrong neighbour. Validate:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use node >/dev/null \
  && ( ulimit -v 12000000; node_modules/.bin/velite --strict )
```

- [ ] **Step 5: Open the PR and hand over**

```bash
git push -u origin v356-exempt-scope
gh pr create --base main --title 'v356: one exempt scope, propagated across the fork (#1)' --body '...'
```

The body must carry: the five-of-five fork evidence, the subshell-inheritance measurement that justifies the deletion, `git diff --stat main -- crates/` showing the net-negative result, the contract count (42), the six unchanged harnesses, the sweep, and the bash-suite PASS-set outcome with the commit stamp confirmed. Poll `gh pr checks <N>` until both workflows finish and pass. Do **not** self-merge — a `vNN` iteration is the user's to merge.

---

## Self-Review

**Spec coverage.** §Problem/the bug → Task 1. §Problem/the redundancy → Task 2. Design §1 (`run_list_element`, labelled block) → Task 2 Step 3. §2 (`is_last` disappears) → Task 2 Step 4. §3 (`clear_for_subshell`, with the measured justification in the doc comment) → Task 1 Step 3. §4 (what is NOT deleted) → enforced by the six regression harnesses in both tasks' verify steps. Verification items 1-6 → the net-negative check in Task 2 Step 6 and Task 3 Steps 1-2 and 5.

**Placeholders.** The only `...` is the `gh pr create --body` in Task 3 Step 5, whose required content is enumerated immediately below it. The blog slug is the author's choice; its frontmatter fields and validation command are exact.

**Type consistency.** `run_list_element(cmd: &Command, exempt: bool, shell: &mut Shell) -> Result<ExecOutcome, ExecOutcome>` is defined in Task 2 Step 3 and called with those exact arguments twice in the same step. `finish_command(cmd, c, err_armed, shell)` matches the signature set in Step 4. `suppress_both` / `unsuppress_both` / `errexit_suppressed` / `err_trap_suppressed` are v355's and used by those names.

**One risk worth stating.** Task 2's leak test is written to pass BEFORE the refactor as well as after. That is deliberate — it is a characterisation test, not a regression test — but it means it cannot catch a leak that already exists. It doesn't: v355's `andor_scope_balances_across_a_list` already covers the current shape, and this widens the input set.
