# v355 — split ERR-trap suppression from errexit suppression — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `set -e` exiting inside a body whose caller is exempt ([#480](https://github.com/jdstanhope/huck/issues/480)), and fix the three ERR-trap rules that share its machinery ([#468](https://github.com/jdstanhope/huck/issues/468), [#469](https://github.com/jdstanhope/huck/issues/469), [#470](https://github.com/jdstanhope/huck/issues/470)).

**Architecture:** `Shell::err_suppressed_depth` gates two things bash treats separately, and is missing from the one exempt context that matters most. It becomes two counters — `errexit_suppressed_depth` and `err_trap_suppressed_depth` — raised through intent-named helpers. Three existing sites raise both; the negated-pipeline site raises both only when errexit is on; and a NEW scope in `run_andor_group` raises both around any command that is not last in its and-or list, which is where all four issues converge.

**Tech Stack:** Rust 2024, crate `huck-engine`. Tests are `#[cfg(test)] mod tests` blocks, `tests/*.rs` integration binaries, and `tests/scripts/*_diff_check.sh` bash-differential harnesses.

**Spec:** `docs/superpowers/specs/2026-08-05-suppression-split-design.md` — read the contract table first; it is the acceptance criteria.

## Global Constraints

- **The contract table is the spec.** Every behavioural claim in this plan was measured against bash 5.2.21 on 2026-08-05. If an implementation disagrees with a row, the implementation is wrong — do not adjust the row without re-measuring against real bash and saying so.
- **Existing ERR/errexit harnesses must stay green with NO expected-value edits**: `err_trap_compound` (30), `err_trap_function` (24), `trap_action_exit` (28), `set_e_andor`, `negated_errexit`. An edit there means the change went further than the contract.
- **Job-control harnesses are load-flaky (#476).** A sweep failure in `coproc`, `job_notify`, `job_spec_percent` or similar must be checked against `main` by run-count before being called a regression, then re-run. Do not absorb it into this iteration.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit; CI enforces `--check`.
- **This box has 1 core and 1.9 GB.** Never `cargo test --workspace` — it OOM-kills the session. Build with `cargo build -p huck --bin huck`.
- **Engine lib tests run with 4 threads:** `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4`.
- **The full sweep exceeds the 10-minute Bash-tool cap.** Run it with `run_in_background: true` writing to a file, then block on `until grep -q "EXIT=" <file>; do sleep 15; done`.
- **Branch:** `v355-suppression-split`, cut from `main` at or after `8aa4f2ad`. Never push to `main`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/huck-engine/src/shell_state.rs` | `Shell` state | Replace `err_suppressed_depth` with two counters + four helpers + two predicates |
| `crates/huck-engine/src/executor.rs` | suppression scopes + read sites | `maybe_errexit` and `finish_command` read their own predicate; three condition sites use `suppress_both`; the negate site becomes conditional; a NEW scope in `run_andor_group` |
| `crates/huck-engine/src/traps.rs` | subshell reset | `clear_for_subshell` zeroes both counters |
| `tests/scripts/errexit_err_suppression_diff_check.sh` | the contract | Create |

---

### Task 1: Two counters, no behaviour change

Split the field mechanically. Every site raises BOTH counters, so behaviour is identical to today — the point is to make the later tasks a one-line choice per site rather than a rename plus a semantic change at once.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (field ~888, init ~1202)
- Modify: `crates/huck-engine/src/executor.rs` (`maybe_errexit` ~95, `finish_command` ~434, sites ~1329, ~2107, ~2124, ~2554)
- Modify: `crates/huck-engine/src/traps.rs` (`clear_for_subshell` ~363, and its unit test ~1143)

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `Shell::errexit_suppressed_depth: u32`, `Shell::err_trap_suppressed_depth: u32`, and `Shell::{suppress_both, unsuppress_both, suppress_errexit_only, unsuppress_errexit_only, errexit_suppressed, err_trap_suppressed}`

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-engine/src/shell_state/tests.rs`:

```rust
#[test]
fn suppression_counters_are_independent() {
    let mut shell = Shell::new();
    assert!(!shell.errexit_suppressed());
    assert!(!shell.err_trap_suppressed());

    shell.suppress_both();
    assert!(shell.errexit_suppressed());
    assert!(shell.err_trap_suppressed());
    shell.unsuppress_both();
    assert!(!shell.errexit_suppressed());
    assert!(!shell.err_trap_suppressed());

    // #469: the negated-pipeline case raises only one of them.
    shell.suppress_errexit_only();
    assert!(shell.errexit_suppressed(), "errexit is suppressed");
    assert!(!shell.err_trap_suppressed(), "the ERR trap is NOT suppressed");
    shell.unsuppress_errexit_only();
    assert!(!shell.errexit_suppressed());
}

#[test]
fn suppression_counters_nest() {
    let mut shell = Shell::new();
    shell.suppress_both();
    shell.suppress_both();
    shell.unsuppress_both();
    assert!(
        shell.errexit_suppressed() && shell.err_trap_suppressed(),
        "still suppressed at depth 1"
    );
    shell.unsuppress_both();
    assert!(!shell.errexit_suppressed() && !shell.err_trap_suppressed());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 suppression_counters`
Expected: FAIL — `no method named 'suppress_both' found`.

- [ ] **Step 3: Replace the field with two, and add the helpers**

In `crates/huck-engine/src/shell_state.rs`, replace `pub err_suppressed_depth: u32,` with:

```rust
    /// Depth of nested contexts where a failing command must NOT exit the
    /// shell under `set -e`: a negated pipeline, an `if`/`while` condition, or
    /// a command in a non-last `&&`/`||` position — and everything they run.
    pub errexit_suppressed_depth: u32,

    /// Depth of nested contexts where a failing command must NOT fire the ERR
    /// trap. Raised with the errexit counter everywhere EXCEPT a negated
    /// pipeline while errexit is off, where bash still fires the body's trap
    /// (#469). Two counters exist for that one cell of the contract table.
    pub err_trap_suppressed_depth: u32,
```

Replace the initialiser `err_suppressed_depth: 0,` with both set to `0`, and add near `take_discard`:

```rust
    /// Enter a context that exempts failures from BOTH `set -e` and the ERR
    /// trap. Pair with `unsuppress_both`.
    pub fn suppress_both(&mut self) {
        self.errexit_suppressed_depth += 1;
        self.err_trap_suppressed_depth += 1;
    }

    pub fn unsuppress_both(&mut self) {
        self.errexit_suppressed_depth = self.errexit_suppressed_depth.saturating_sub(1);
        self.err_trap_suppressed_depth = self.err_trap_suppressed_depth.saturating_sub(1);
    }

    /// Enter a context that exempts failures from `set -e` but leaves the ERR
    /// trap live — a negated pipeline while errexit is off (#469).
    pub fn suppress_errexit_only(&mut self) {
        self.errexit_suppressed_depth += 1;
    }

    pub fn unsuppress_errexit_only(&mut self) {
        self.errexit_suppressed_depth = self.errexit_suppressed_depth.saturating_sub(1);
    }

    /// True when a failing command must not exit the shell under `set -e`.
    pub fn errexit_suppressed(&self) -> bool {
        self.errexit_suppressed_depth > 0
    }

    /// True when a failing command must not fire the ERR trap.
    pub fn err_trap_suppressed(&self) -> bool {
        self.err_trap_suppressed_depth > 0
    }
```

`saturating_sub` rather than `-= 1` so a mismatched pair cannot panic in release-mode arithmetic; a mismatch is a bug either way, and the tests in Step 1 pin the nesting.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 suppression_counters`
Expected: PASS (2 tests).

- [ ] **Step 5: Migrate every existing site to raise BOTH**

`cargo build -p huck --bin huck` and fix each error:

- `maybe_errexit` (`executor.rs` ~95): `shell.err_suppressed_depth == 0` → `!shell.errexit_suppressed()`
- `finish_command` (~434): `shell.err_suppressed_depth == 0` → `!shell.err_trap_suppressed()`
- the `while`/`until` condition (~1329), the `if` condition (~2107), the `elif` condition (~2124), and the negate arm (~2554): `shell.err_suppressed_depth += 1;` → `shell.suppress_both();` and `-= 1;` → `shell.unsuppress_both();`
- `traps::clear_for_subshell` (~363): `shell.err_suppressed_depth = 0;` → set both to `0`
- the unit test in `traps.rs` (~1143) that sets the old field to 5 and asserts it is cleared: set and assert BOTH counters (a mechanical access update, allowed)

- [ ] **Step 6: Verify nothing changed**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
for h in err_trap_compound err_trap_function set_e_andor negated_errexit trap_action_exit; do
  printf '%-22s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
```

Expected: engine lib green (2 more tests), all five harnesses green, **no expected value edited**.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(#480): split err_suppressed_depth into two counters

Mechanical split, no behavior change: every site raises BOTH counters, so the
later tasks are a one-line choice per site rather than a rename tangled with a
semantic change.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The truth-table harness (expected to fail)

Write the contract down as executable rows BEFORE changing behaviour, so the next two tasks are measured rather than argued.

**Files:**
- Create: `tests/scripts/errexit_err_suppression_diff_check.sh`

**Interfaces:**
- Consumes: nothing from Task 1
- Produces: the harness later tasks must turn green

- [ ] **Step 1: Write the harness**

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for how an EXEMPT command's body treats
# the ERR trap and `set -e` (#480, #468, #469, #470).
#
# bash propagates "ignore return" INTO the body of a command whose own failure
# is exempt. huck applied the exemption only at the outer command, so a body
# still tripped `set -e` — the shell exited where bash ran the handler.
#
# The one asymmetric cell: `!` does NOT stop a compound body firing ERR unless
# errexit is on. Confirmed as the inner command firing, not an artefact:
# `! { (exit 5); }` reports E:5, and `! { false; true; }` fires though the
# group SUCCEEDS.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- errexit inside an exempt body: the sharp end (#480) -------------------
check "errexit, func via ||"   'set -e; f() { false; echo x; }; f || echo or'
check "errexit, func via &&"   'set -e; f() { false; echo x; }; true && f; echo after'
check "errexit, brace via ||"  'set -e; { false; echo x; } || echo or'
check "errexit, brace via &&"  'set -e; { false; echo x; } && echo and; echo after'
check "errexit, for via ||"    'set -e; for i in 1; do false; echo x; done || echo or'
check "errexit, nested via ||" 'set -e; { { false; echo x; }; } || echo or'
check "errexit still exits"    'set -e; f() { false; echo x; }; f; echo after'
check "errexit plain"          'set -e; false; echo after'

# --- ERR in an exempt body (#468) ------------------------------------------
check "ERR, brace via ||"      'trap "echo E" ERR; { false; } || echo or'
check "ERR, brace via &&"      'trap "echo E" ERR; { false; } && echo and'
check "ERR, body prints"       'trap "echo E" ERR; { false; echo x; } || echo or'
check "ERR, for via ||"        'trap "echo E" ERR; for i in 1; do false; done || echo or'
check "ERR, if cond"           'trap "echo E" ERR; if { false; }; then :; fi; echo after'
check "ERR, while cond"        'trap "echo E" ERR; while { false; }; do :; done; echo after'

# --- the `!` asymmetry (#469) ----------------------------------------------
check "! fires without -e"     'trap "echo E" ERR; ! { false; }; echo after'
check "! silent with -e"       'set -e; trap "echo E" ERR; ! { false; }; echo after'
check "! carries the status"   'trap "echo E:\$?" ERR; ! { (exit 5); }'
check "! group succeeds"       'trap "echo E" ERR; ! { false; true; }'
check "! nested"               'trap "echo E" ERR; ! { { false; }; }'
check "! double negation"      'trap "echo E" ERR; ! ! { false; }'
check "! simple command"       'trap "echo E" ERR; ! false'
check "! subshell"             'trap "echo E" ERR; ! ( false )'

# --- the inherited path under set -E (#470) --------------------------------
check "-E func via ||"         'set -E; trap "echo E" ERR; f() { false; }; f || echo or'
check "-E func plain"          'set -E; trap "echo E" ERR; f() { false; }; f; echo after'
check "-E func negated"        'set -E; trap "echo E" ERR; f() { false; }; ! f'
check "-E brace via ||"        'set -E; trap "echo E" ERR; { false; } || echo or'

# --- rules that must NOT change --------------------------------------------
check "last command fires"     'trap "echo E" ERR; { false; }'
check "compound once (#445)"   'trap "echo E" ERR; { { false; }; }'
check "subshell fires"         'trap "echo E" ERR; ( false )'
check "function call fires"    'trap "echo E" ERR; f() { false; }; f'
check "errexit in if body"     'set -e; if true; then false; fi; echo after'
check "status after exempt"    'trap "echo E" ERR; { false; } || echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
```

- [ ] **Step 2: Run it to see the starting point**

Run: `cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | tail -5 )`
Expected: FAIL on the errexit-in-exempt-body rows, the ERR-in-exempt-body rows, the `!`-without-`-e` rows and the `-E` rows. Record the count — the next two tasks reduce it to zero. The "must NOT change" rows must ALREADY pass; if any of them fails now, stop: the baseline is not what the spec says it is.

- [ ] **Step 3: Commit the harness**

```bash
chmod +x tests/scripts/errexit_err_suppression_diff_check.sh
git add tests/scripts/errexit_err_suppression_diff_check.sh
git commit -m "test(#480): pin the ERR/errexit suppression contract

The contract table from the spec as executable rows, committed RED so the
fixes that follow are measured rather than argued. The 'must not change' rows
pass already and guard #445's compound rule and the outer-command exemptions.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Suppress inside a non-last and-or command — #480, #468, #470

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`run_andor_group`)

**Interfaces:**
- Consumes: `Shell::{suppress_both, unsuppress_both}` (Task 1)
- Produces: nothing for later tasks

- [ ] **Step 1: Raise the scope around a non-last `first`**

In `run_andor_group`, the `first` command is exempt iff `rest` is non-empty. Replace:

```rust
    let err_armed_first = crate::traps::err_trap_armed(shell);
    let mut status = run_command(first, shell);
```

with:

```rust
    let err_armed_first = crate::traps::err_trap_armed(shell);
    // #480/#468/#470: a command that is NOT the last of its and-or list is
    // exempt, and bash propagates that exemption INTO whatever the command
    // runs — a brace group's statements, a function's body, a loop's
    // iterations. Without this scope `set -e; f() { false; echo x; }; f || or`
    // exits the shell inside f, where bash prints x and then runs the handler.
    // For a simple command the scope changes nothing: its own fire is already
    // skipped by the `is_last` guard and it has no body.
    let first_exempt = !rest.is_empty();
    if first_exempt {
        shell.suppress_both();
    }
    let mut status = run_command(first, shell);
    if first_exempt {
        shell.unsuppress_both();
    }
```

**The un-suppress must happen before any early return below it.** Placing it immediately after `run_command` — rather than at the end of the function — is what guarantees that; the `check_interrupt` and control-flow returns that follow would otherwise leak the depth into the caller.

- [ ] **Step 2: Raise the scope around a non-last `rest` element**

In the `for i in 0..rest.len()` loop, replace:

```rust
            let err_armed = crate::traps::err_trap_armed(shell);
            status = run_command(command, shell);
```

with:

```rust
            let err_armed = crate::traps::err_trap_armed(shell);
            // Same rule as `first`: exempt iff this is not the last element.
            let exempt = i + 1 != rest.len();
            if exempt {
                shell.suppress_both();
            }
            status = run_command(command, shell);
            if exempt {
                shell.unsuppress_both();
            }
```

- [ ] **Step 3: Run the harness**

Run: `cargo fmt --all && cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | grep -E "^FAIL|^Total" )`
Expected: every errexit row, every `#468` ERR row and every `-E` row passes. The two `!`-without-`-e` rows (`! fires without -e`, `! carries the status`, `! group succeeds`, `! nested`, `! double negation`) still FAIL — Task 4 fixes those.

- [ ] **Step 4: Check the regression harnesses**

```bash
for h in err_trap_compound err_trap_function set_e_andor negated_errexit trap_action_exit; do
  printf '%-22s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 2>&1 | grep "^test result" )
```

Expected: all green, no expected-value edits.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix(#480): an exempt command's body inherits the exemption

bash propagates ignore-return INTO the body of a command whose own failure is
exempt. huck applied it only at the outer command, so \`set -e; f() { false;
echo x; }; f || echo or\` exited the shell inside f where bash prints x and
runs the handler — a script guarding a failing step with \`|| handler\` died
instead of handling it.

One scope in run_andor_group around any non-last command covers the errexit
case (#480), the ERR-trap case (#468) and the inherited \`set -E\` path (#470).
The un-suppress sits immediately after run_command so the early returns below
it cannot leak the depth.

Closes #480
Closes #468
Closes #470

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The negation asymmetry — #469

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the `pipeline.negate` arm)

**Interfaces:**
- Consumes: `Shell::{suppress_both, unsuppress_both, suppress_errexit_only, unsuppress_errexit_only}` (Task 1)
- Produces: nothing

- [ ] **Step 1: Make the negate scope conditional**

Replace the two `pipeline.negate` blocks. The opening one:

```rust
    if pipeline.negate {
        shell.suppress_both();
    }
```

becomes:

```rust
    // #469: `!` exempts the negated command itself from both — that part is
    // handled by `is_negated_pipeline` at the fire site. What it does NOT do
    // is stop a compound BODY firing the ERR trap: bash prints the trap's
    // output for `! { false; }` with `set +e` and stays silent with `set -e`.
    // Reproducing a bash quirk, not choosing a rule; the contract table in the
    // spec records the measurement. Read at execution time so `set -e` /
    // `set +e` mid-script behaves as bash does.
    let negate_suppresses_err_trap = shell.shell_options.errexit;
    if pipeline.negate {
        if negate_suppresses_err_trap {
            shell.suppress_both();
        } else {
            shell.suppress_errexit_only();
        }
    }
```

and the closing one:

```rust
    if pipeline.negate {
        shell.unsuppress_both();
```

becomes:

```rust
    if pipeline.negate {
        // Undo exactly what was raised — `set -e` may have CHANGED inside the
        // body, so re-reading `shell_options.errexit` here would unbalance the
        // counters.
        if negate_suppresses_err_trap {
            shell.unsuppress_both();
        } else {
            shell.unsuppress_errexit_only();
        }
```

- [ ] **Step 2: Run the harness**

Run: `cargo fmt --all && cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 400 bash tests/scripts/errexit_err_suppression_diff_check.sh 2>&1 | tail -3 )`
Expected: **Fail: 0** across all rows.

- [ ] **Step 3: Add a unit test for the balance rule**

The "undo what you raised" rule above is invisible to the harness — `set -e` changing inside a negated body is exotic. Pin it in `crates/huck-engine/src/executor/errexit_andor_tests.rs`:

```rust
/// v355 (#469): the negate arm chooses which counters to raise from errexit's
/// state at ENTRY, and must undo exactly those — a `set -e` inside the body
/// would otherwise unbalance the counters and leave the shell permanently
/// suppressed.
#[test]
fn negation_scope_balances_when_errexit_changes_inside() {
    let mut s = Shell::new();
    assert!(!s.errexit_suppressed() && !s.err_trap_suppressed());
    let _ = crate::shell::process_line("! { set -e; false; }", &mut s, false);
    assert!(
        !s.errexit_suppressed(),
        "errexit suppression leaked out of the negated body"
    );
    assert!(
        !s.err_trap_suppressed(),
        "ERR-trap suppression leaked out of the negated body"
    );
}
```

- [ ] **Step 4: Run it**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 negation_scope_balances`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "fix(#469): \`!\` does not silence a compound body's ERR trap

bash fires the body's ERR trap under \`!\` when errexit is off and stays silent
when it is on. huck suppressed both ways, so \`trap 'echo E' ERR; ! { false; }\`
printed nothing where bash prints E.

The negate arm now picks its counters from errexit's state at ENTRY and undoes
exactly those — re-reading the option on the way out would unbalance them if
the body ran \`set -e\`, which a unit test pins.

Closes #469

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Verification, docs, blog, PR

**Files:**
- Modify: `docs/architecture.md`
- Create: `site/content/blog/<slug>.mdx`

- [ ] **Step 1: Full test + sweep**

```bash
cargo fmt --all && cargo fmt --all --check
( ulimit -v 1500000; timeout 590 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
for t in set_options_integration pipefail_integration if_integration while_integration \
         for_integration case_integration functions_integration subshell_integration \
         trap_integration trap_pseudo_signals_integration; do
  printf '%-40s ' $t
  ( ulimit -v 1500000; timeout 500 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 | grep "test result" )
done
( ulimit -v 1500000; timeout 590 cargo build --release --locked --bin huck )
```

Then the sweep, backgrounded (it exceeds the tool's 10-minute cap):

```bash
( ulimit -v 1500000; tests/scripts/run_diff_checks.sh > /tmp/v355-sweep.txt 2>&1; echo "EXIT=$?" >> /tmp/v355-sweep.txt )
# then: until grep -q "EXIT=" /tmp/v355-sweep.txt; do sleep 15; done
```

Expected: all green. A failure in `coproc`, `job_notify` or `job_spec_percent` is #476 — verify against `main` by run-count, then re-run the sweep.

- [ ] **Step 2: bash-suite PASS-set diff**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
( ulimit -v 2500000; bash tests/bash-test-suite/runner.sh > /tmp/v355-suite.md 2>&1 )
git stash
( ulimit -v 2500000; bash tests/bash-test-suite/runner.sh > /tmp/main-suite.md 2>&1 )
git stash pop
diff <(grep '| PASS' /tmp/main-suite.md | sort) <(grep '| PASS' /tmp/v355-suite.md | sort)
```

Expected: `errexit` and `set-e` are the categories most likely to move. **Movement in EITHER direction must be explained in the PR** — a new PASS is good news that still needs naming, and a lost PASS blocks the merge. ⚠️ The runner rebuilds the RELEASE binary, so rebuild before capturing any before/after output.

- [ ] **Step 3: Update the architecture doc**

In `docs/architecture.md`, extend the errexit/ERR paragraph: `Shell` carries two suppression counters — `errexit_suppressed_depth` and `err_trap_suppressed_depth` — raised through `suppress_both` / `suppress_errexit_only` and read by `errexit_suppressed()` / `err_trap_suppressed()`. Four contexts raise them: a non-last `&&`/`||` command, an `if`/`elif` condition, a `while`/`until` condition (all both), and a negated pipeline (errexit always; the ERR trap only when errexit is on, per bash). Note that the exemption propagates INTO the command's body, which is what makes `set -e; f() { false; echo x; }; f || echo or` print `x`.

- [ ] **Step 4: Blog entry**

`site/content/blog/<slug>.mdx`, frontmatter `title` (≤120), `date: 2026-08-05`, `summary` (≤300), `tags`, `version: "v355"`, `draft: false`. Lead with the user-visible bug — a script that guards a failing step with `|| handler` died instead of running the handler under `set -e` — with real before/after from a pre-v355 binary. Then the "one counter for two ideas" story and the one asymmetric cell. Validate:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use node >/dev/null \
  && ( ulimit -v 12000000; node_modules/.bin/velite --strict )
```

Expected: exit 0.

- [ ] **Step 5: Open the PR and hand over**

```bash
git push -u origin v355-suppression-split
gh pr create --base main --title 'v355: split ERR-trap suppression from errexit suppression (#480)' --body '...'
```

The body must carry: the contract table, the sharp-end before/after for `set -e`, the harness count, the sweep result, the bash-suite PASS-set diff outcome with any movement explained, and that #468/#469/#470 close alongside #480. Poll `gh pr checks <N>` until both workflows finish and pass. Do **not** self-merge — a `vNN` iteration is the user's to merge.

---

## Self-Review

**Spec coverage.** Contract table rows → Task 2's harness sections, fixed by Tasks 3 (errexit + `&&`/`||` + `-E`) and 4 (`!`). Design §1 (two counters + helpers) → Task 1. §2 (where each is raised) → Task 1 Step 5 for the three condition sites, Task 4 for the negate site. §3 (the new and-or scope) → Task 3. §4 (what does not change) → Task 2's "must NOT change" rows plus Task 3 Step 4. Verification items 1-6 → Task 2, Task 3 Step 4, Task 5 Steps 1-2 and 5. Non-goals → Global Constraints (#476 must not be absorbed) and the untouched `is_last` / `is_negated_pipeline` guards.

**Placeholders.** The only `...` is the `gh pr create --body` in Task 5 Step 5, whose required content is enumerated immediately below it. The blog slug is deliberately unnamed — it is the author's choice — but its frontmatter fields and validation command are exact.

**Type consistency.** `errexit_suppressed_depth` / `err_trap_suppressed_depth` and the six helpers are defined in Task 1 and used by those exact names in Tasks 3, 4 and the unit tests. `suppress_both` pairs with `unsuppress_both`, `suppress_errexit_only` with `unsuppress_errexit_only`; Task 4 uses the `negate_suppresses_err_trap` local to keep the pairing balanced.

**One risk worth stating.** Task 3's scope must be dropped immediately after `run_command`, not at the end of the function, because five early returns sit between them. Getting that wrong leaks suppression into the caller and would show up as `set -e` silently not working after the first `&&` — a failure mode the harness would catch only indirectly, which is why the step spells it out.
