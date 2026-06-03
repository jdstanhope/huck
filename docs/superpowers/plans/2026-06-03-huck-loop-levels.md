# huck v79 — `break N` / `continue N` Loop Levels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash's `break N` / `continue N` multi-level loop control plus the bash-style "outside any loop" diagnostic + exit 1 (was silent exit 0).

**Architecture:** Three coordinated changes. (1) `ExecOutcome::LoopBreak` / `LoopContinue` gain a `u32` level payload (1-based, never 0). (2) New `Shell.loop_depth: u32` field; each loop runner (`run_for` / `run_while` / `run_arith_for`) increments on entry / decrements on exit via a single-return-path wrapper; `call_function` saves+restores it so `break` inside a function called from a loop correctly errors. (3) `break`/`continue` builtins check depth, cap level, and emit either the variant or a diagnostic. Loops use the decrement-and-bubble pattern.

**Tech Stack:** Rust 1.85+; no new dependencies.

**Branch:** `v79-loop-levels` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-03-huck-loop-levels-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

```bash
git checkout -b v79-loop-levels
```

Expected: `Switched to a new branch 'v79-loop-levels'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Baseline:", sum}'`
Expected: 2282 (current main).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` with no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/builtins.rs` | `ExecOutcome::LoopBreak(u32)` / `LoopContinue(u32)` payload. New `builtin_break` / `builtin_continue` helpers + refactored `builtin_return` + shared `parse_loop_level`. Match-site updates throughout the file. ~10 unit tests in a new `mod loop_levels_tests` block. | 1 |
| `src/shell_state.rs` | New `Shell.loop_depth: u32` field; init 0 in `Shell::new()`. | 1 |
| `src/executor.rs` | Match-site updates for the new payload (~13 sites). `run_for` / `run_while` / `run_arith_for` split into single-return-path wrappers (`run_*_inner` holds the existing body) that inc/dec `shell.loop_depth`. Body match arms gain the `LoopBreak(1)` / `LoopBreak(n)` decrement-and-bubble pattern. `call_function` saves+restores `loop_depth`. ~8 unit tests in a new `mod loop_levels_executor_tests` block. | 2 |
| `tests/loop_levels_integration.rs` | NEW. 6 binary-driven integration tests. | 3 |
| `tests/scripts/loop_levels_diff_check.sh` | NEW. 8 bash-diff fragments byte-identical to bash 5.2. | 3 |
| `docs/bash-divergences.md` | M-30 flipped to `[fixed v79]` with corrected text (no longer falsely groups `return N` with break/continue); change-log entry; Summary table tier count + "Last updated" stamp refresh. | 3 |
| `README.md` | New v79 iteration row. | 3 |

---

## Task 1: AST payload + Shell.loop_depth + builtin rewrites

**Files:**
- Modify: `src/builtins.rs` — ExecOutcome payload; new builtin helpers; match-site updates; ~10 unit tests
- Modify: `src/shell_state.rs` — `loop_depth` field

**Goal:** Compile-clean state where the new variants exist and the new builtins use `Shell.loop_depth`. Executor still compiles via mechanical match-site updates (it doesn't implement the decrement-and-bubble logic yet — Task 2). All 2282 baseline tests continue passing under the new shape. New builtin behaviors covered by ~10 unit tests.

### Steps

- [ ] **Step 1: Add `loop_depth` field to `Shell`**

Edit `src/shell_state.rs`. Find the `pub struct Shell {` declaration. Add the new field:

```rust
pub struct Shell {
    // ... existing fields ...
    /// Tracks current loop-nesting depth. Incremented by run_for /
    /// run_while / run_arith_for via single-return-path wrappers,
    /// decremented on exit. Saved+restored across call_function.
    /// Used by `break` / `continue` builtins to validate they're in
    /// a loop and to cap the level argument to actual depth.
    pub loop_depth: u32,
}
```

In `impl Shell { pub fn new() -> Self {`, add the field initializer:

```rust
            loop_depth: 0,
```

- [ ] **Step 2: Change `ExecOutcome::LoopBreak` / `LoopContinue` to carry `u32` payload**

Edit `src/builtins.rs`. Find `pub enum ExecOutcome` (line 11):

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32),         // level: 1-based, capped to loop_depth by builtin
    LoopContinue(u32),
    FunctionReturn(i32),
}
```

This breaks every pattern-match on `LoopBreak`/`LoopContinue`. The next steps fix each site mechanically.

- [ ] **Step 3: Build to see the compile errors**

Run: `cargo build 2>&1 | grep -E "non-exhaustive|missing match arm|cannot find" | head -20`
Expected: ~20 compile errors at the pattern-match sites listed in the spec's match-site inventory.

The strategy: convert each site mechanically. For sites that PROPAGATE (the variant just bubbles up), change `LoopBreak | LoopContinue` to `LoopBreak(_) | LoopContinue(_)`. For sites that PRODUCE (the builtins) or CONSUME (the loop runners), use the level explicitly.

- [ ] **Step 4: Update propagation sites in `src/executor.rs`**

There are several "treat LoopBreak/Continue as propagation" sites. Update each:

`src/executor.rs:78` (in `execute_capturing`):
```rust
ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_) => 0,
```

`src/executor.rs:88` (in `execute_sequence_body`'s first check):
```rust
if matches!(
    status,
    ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_)
        | ExecOutcome::FunctionReturn(_)
)
```

`src/executor.rs:124` (in `execute_sequence_body`'s loop):
```rust
ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_)
```

`src/executor.rs:261` (in `run_while` condition handling):
```rust
ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_)
    | ExecOutcome::FunctionReturn(_) => {
    return cond;
}
```

`src/executor.rs:467-468` (in pipeline-stage forwarding):
```rust
ExecOutcome::LoopBreak(n) => return ExecOutcome::LoopBreak(n),
ExecOutcome::LoopContinue(n) => return ExecOutcome::LoopContinue(n),
```

`src/executor.rs:497` (one of the subshell propagation arms):
```rust
ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_)
```

`src/executor.rs:511` (the other one):
```rust
ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_)
```

`src/executor.rs:3240` (test helper):
```rust
ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_) => 0,
```

- [ ] **Step 5: Stub the loop-runner consume sites in `src/executor.rs`**

The three loop runners (`run_for` at line 274/278, `run_while` at line 320/324, `run_arith_for_inner` at line 399/403) have body-result match arms. For now (Task 1 scope), just preserve the OLD behavior by treating `LoopBreak(_)` like the old `LoopBreak` (break this loop) and `LoopContinue(_)` like the old `LoopContinue` (continue this loop). Task 2 replaces these with the real decrement-and-bubble logic.

At each site, change `ExecOutcome::LoopBreak =>` to `ExecOutcome::LoopBreak(_) =>` (and same for Continue). The body of each arm stays unchanged.

**`run_while` body match** (around line 274-281):
```rust
ExecOutcome::LoopBreak(_) => {
    last = ExecOutcome::Continue(0);
    break;
}
ExecOutcome::LoopContinue(_) => {
    last = ExecOutcome::Continue(0);
    // fall through — the loop re-tests the condition
}
```

**`run_for` body match** (around line 320-327):
```rust
ExecOutcome::LoopBreak(_) => {
    last = ExecOutcome::Continue(0);
    break;
}
ExecOutcome::LoopContinue(_) => {
    last = ExecOutcome::Continue(0);
    // fall through — advance to the next value
}
```

**`run_arith_for` body match** (around line 399-406):
```rust
ExecOutcome::LoopBreak(_) => {
    last = ExecOutcome::Continue(0);
    break;
}
ExecOutcome::LoopContinue(_) => {
    last = ExecOutcome::Continue(0);
    // fall through to step
}
```

Task 2 will replace each pair with the four-arm decrement-and-bubble pattern.

- [ ] **Step 6: Update the producer sites in `src/builtins.rs`**

Find `run_builtin` (around line 115). The current arms:

```rust
"break" => ExecOutcome::LoopBreak,
"continue" => ExecOutcome::LoopContinue,
```

Replace with function calls (the helpers are added in Step 7):

```rust
"break" => builtin_break(args, shell),
"continue" => builtin_continue(args, shell),
"return" => builtin_return(args, shell),
```

Also remove the existing inline `"return" =>` arm (around line 121-127) — its body moves into `builtin_return` (no behavior change).

Look for other LoopBreak/LoopContinue producers in `src/builtins.rs`:
- `src/builtins.rs:4650` (in test helper): `ExecOutcome::LoopBreak | ExecOutcome::LoopContinue =>` becomes `ExecOutcome::LoopBreak(_) | ExecOutcome::LoopContinue(_) =>`.
- `src/builtins.rs:6105` (assert in existing test): `assert!(matches!(outcome, ExecOutcome::LoopBreak))` becomes `assert!(matches!(outcome, ExecOutcome::LoopBreak(_)))`. **For these existing tests, additionally update to assert `LoopBreak(1)` since a bare `break` (no args) emits level 1.** Same for line 6113 Continue test.

The existing assertion change:

```rust
// Before (line 6105):
assert!(matches!(outcome, ExecOutcome::LoopBreak));
// After:
assert_eq!(outcome, ExecOutcome::LoopBreak(1));
```

(Use `assert_eq!` with the literal so we're explicit that level 1 is expected.)

Same pattern for the Continue test at line 6113:

```rust
// Before:
assert!(matches!(outcome, ExecOutcome::LoopContinue));
// After:
assert_eq!(outcome, ExecOutcome::LoopContinue(1));
```

**Important**: the existing tests that exercise `break` / `continue` may do so via `run_builtin("break", &[], shell)` without setting `shell.loop_depth`. Under the new logic, that path errors with "only meaningful in a loop" + Continue(1). Update the tests to set `shell.loop_depth = 1` before the call so they exercise the in-loop path. See Step 9 for how the new tests pre-set the field.

- [ ] **Step 7: Add the `parse_loop_level` helper + new builtin functions**

Append to `src/builtins.rs` (place near the existing `is_special_builtin` / `is_declaration_command` predicates, or grouped with the other `builtin_*` helpers — your call):

```rust
/// Parses the loop-level argument for `break` / `continue`.
/// `Ok(N)` is the validated positive level (defaults to 1 with no args).
/// `Err(status)` is the exit status to return after the diagnostic
/// has already been printed.
fn parse_loop_level(args: &[String], cmd: &str) -> Result<u32, i32> {
    let Some(arg) = args.first() else { return Ok(1) };
    match arg.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n.min(u32::MAX as i64) as u32),
        Ok(_) => {
            eprintln!("huck: {cmd}: {arg}: loop count out of range");
            Err(128)
        }
        Err(_) => {
            eprintln!("huck: {cmd}: {arg}: numeric argument required");
            Err(128)
        }
    }
}

fn builtin_break(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        eprintln!("huck: break: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(1);
    }
    let level = match parse_loop_level(args, "break") {
        Ok(n) => n,
        Err(status) => return ExecOutcome::Continue(status),
    };
    let capped = level.min(shell.loop_depth);
    ExecOutcome::LoopBreak(capped)
}

fn builtin_continue(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        eprintln!("huck: continue: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(1);
    }
    let level = match parse_loop_level(args, "continue") {
        Ok(n) => n,
        Err(status) => return ExecOutcome::Continue(status),
    };
    let capped = level.min(shell.loop_depth);
    ExecOutcome::LoopContinue(capped)
}

/// `return [N]` builtin. Sets the exit status to N (or `$?` if N is
/// omitted or unparseable) and returns `FunctionReturn(code)` so the
/// enclosing function unwinds. Behavior preserved from the v0 inline
/// implementation — extracted to a named helper for symmetry with
/// builtin_break and builtin_continue.
fn builtin_return(args: &[String], shell: &Shell) -> ExecOutcome {
    let code = match args.first() {
        Some(s) => s.parse::<i32>().unwrap_or_else(|_| shell.last_status()),
        None => shell.last_status(),
    };
    ExecOutcome::FunctionReturn(code)
}
```

Note: `builtin_return` takes `&Shell` (not `&mut`) because the existing inline behavior only READS `shell.last_status()`. The other two need `&mut Shell` so the function signatures can be compatible with the match-arm calls if the existing dispatch requires uniform `&mut shell` — adjust to `&mut Shell` if the borrow checker complains; semantically it's still read-only.

- [ ] **Step 8: Build clean**

Run: `cargo build 2>&1 | tail -10`
Expected: clean build. If non-exhaustive-match errors remain, locate and fix them with the appropriate pattern (`_` for propagators; `(1)` literal or `(n)` capture for consumers/producers).

- [ ] **Step 9: Add 10 builtin unit tests**

Append a new `mod loop_levels_tests` to the bottom of `src/builtins.rs` (or extend an existing test module — but a dedicated module is cleaner). Use the existing pattern from `src/builtins.rs::tests` for `Shell` construction.

```rust
#[cfg(test)]
mod loop_levels_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn break_no_args_emits_level_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&[], &mut sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(1));
    }

    #[test]
    fn break_with_arg_n_emits_level_n_when_in_loop() {
        let mut sh = Shell::new();
        sh.loop_depth = 3;
        let outcome = builtin_break(&["2".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2));
    }

    #[test]
    fn break_caps_to_loop_depth() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_break(&["999".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2));
    }

    #[test]
    fn break_outside_loop_errors_with_status_1() {
        let mut sh = Shell::new();
        // sh.loop_depth = 0 by default.
        let outcome = builtin_break(&[], &mut sh);
        assert_eq!(outcome, ExecOutcome::Continue(1));
    }

    #[test]
    fn break_zero_errors_with_status_128() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&["0".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::Continue(128));
    }

    #[test]
    fn break_negative_errors_with_status_128() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&["-1".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::Continue(128));
    }

    #[test]
    fn break_non_numeric_errors_with_status_128() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&["abc".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::Continue(128));
    }

    #[test]
    fn continue_no_args_emits_level_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_continue(&[], &mut sh);
        assert_eq!(outcome, ExecOutcome::LoopContinue(1));
    }

    #[test]
    fn continue_outside_loop_errors_with_status_1() {
        let mut sh = Shell::new();
        let outcome = builtin_continue(&[], &mut sh);
        assert_eq!(outcome, ExecOutcome::Continue(1));
    }

    #[test]
    fn continue_caps_to_loop_depth() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_continue(&["5".to_string()], &mut sh);
        assert_eq!(outcome, ExecOutcome::LoopContinue(1));
    }
}
```

- [ ] **Step 10: Run new tests**

Run: `cargo test --quiet loop_levels_tests 2>&1 | tail -10`
Expected: 10 tests pass.

- [ ] **Step 11: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 1:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```

Expected: **2292 tests pass** (2282 baseline + 10 new). Clippy clean.

If existing tests fail because they exercised `break`/`continue` without setting `loop_depth`, locate them. Pre-set `shell.loop_depth = 1` before the affected calls (the new logic errors out at depth 0). Document any such update in the commit message.

- [ ] **Step 12: Smoke-test the new error path from the binary**

```bash
echo 'break' | cargo run --quiet 2>&1 | head -3
```

Expected: `huck: break: only meaningful in a `for', `while', or `until' loop` on stderr (the exact backtick/quote glyphs depend on terminal, but the message text matches bash). Exit code 1.

```bash
echo 'continue 0' | cargo run --quiet 2>&1 | head -3
```

Expected: stderr with `huck: continue: only meaningful in a 'for', 'while', or 'until' loop` — wait, actually `continue 0` runs the same depth-check FIRST (which errors with "only meaningful" since loop_depth=0). The malformed-N check only fires after the depth check passes. That's fine — the depth diagnostic is the more important one to surface.

If you want to also exercise the malformed-N path from the binary, define a loop first:

```bash
echo 'for i in 1; do break 0; done; echo $?' | cargo run --quiet
```

Expected: `huck: break: 0: loop count out of range` on stderr, exit `128` on stdout. (Wait — Task 1 just makes the BUILTIN emit `Continue(128)`. Whether the for-loop body then returns that as the loop's exit, AND whether the top-level surfaces it as `$?`, depends on Task 2's behavior. Task 1's loop-body match arm at line 320 currently coerces `LoopBreak(_)` → `Continue(0)` and other Continue values pass through. So `break 0` returns `Continue(128)` (not LoopBreak), the for-loop's `last` becomes `Continue(128)`, the for-loop ends, and `echo $?` prints `128`.

That works! Verify it does.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v79 task 1: ExecOutcome payload + Shell.loop_depth + builtin rewrites

ExecOutcome::LoopBreak and LoopContinue gain a u32 level payload
(1-based; capped to shell.loop_depth by the builtin so loop runners
never see overshoot). New Shell.loop_depth: u32 field, init 0 in
Shell::new(). The 20+ match sites across executor.rs and builtins.rs
are mechanically updated — propagation sites use `_` for the
payload; loop runners temporarily preserve old behavior with
`LoopBreak(_)` (Task 2 replaces with the decrement-and-bubble logic).

New builtin_break / builtin_continue helpers + extracted builtin_return
(no behavior change for return). Shared parse_loop_level helper handles
default N=1, n>=1 validation, and the two bash-style error messages
("numeric argument required" / "loop count out of range") with status
128. Depth check before parsing so `break` outside any loop returns
status 1 with the "only meaningful in a for/while/until loop"
diagnostic (was silent exit 0).

10 new unit tests in mod loop_levels_tests cover both builtins
end-to-end: no-args level 1, with-arg level N, cap-to-depth,
outside-loop error, zero/negative/non-numeric errors.

Two pre-existing builtin tests (line 6105 / 6113) updated to
assert_eq! on LoopBreak(1) / LoopContinue(1) and to set
shell.loop_depth=1 before the call.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Loop runner decrement-and-bubble + function-boundary save/restore

**Files:**
- Modify: `src/executor.rs` — split `run_for` / `run_while` / `run_arith_for` into wrapper + `*_inner`; add real decrement-and-bubble match arms; modify `call_function` to save/restore `shell.loop_depth`.
- Test: ~8 unit tests in a new `mod loop_levels_executor_tests` block in `src/executor.rs`.

**Goal:** End-to-end multi-level break/continue working. `break N` in a 3-level nest correctly unwinds N levels. `continue 2` from an inner loop runs the outer loop's step. `break` inside a function called from a loop errors (because the function call zeroed `loop_depth`).

### Steps

- [ ] **Step 1: Wrap `run_for` with the loop_depth inc/dec wrapper**

Edit `src/executor.rs`. Find `fn run_for` (around line 296). Rename it to `run_for_inner`:

```rust
fn run_for_inner(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    // (existing body unchanged)
}
```

Add a new `run_for` wrapper directly above:

```rust
fn run_for(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_for_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}
```

- [ ] **Step 2: Replace the `run_for_inner` body match arms with decrement-and-bubble**

Inside `run_for_inner`, find the body-result match (around line 318-333, now shifted by Step 1's wrapper). Replace the `LoopBreak(_)` and `LoopContinue(_)` arms with the four-arm pattern:

```rust
match execute_sequence_body(&clause.body, shell, sink) {
    ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),

    ExecOutcome::LoopBreak(1) => {
        last = ExecOutcome::Continue(0);
        break;
    }
    ExecOutcome::LoopBreak(n) => {
        return ExecOutcome::LoopBreak(n - 1);
    }

    ExecOutcome::LoopContinue(1) => {
        last = ExecOutcome::Continue(0);
        // fall through — advance to the next value
    }
    ExecOutcome::LoopContinue(n) => {
        return ExecOutcome::LoopContinue(n - 1);
    }

    ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
    ExecOutcome::Continue(c) => {
        last = ExecOutcome::Continue(c);
    }
}
```

**Important**: when `run_for_inner` returns `LoopBreak(n-1)` / `LoopContinue(n-1)`, the wrapper STILL runs the `shell.loop_depth -= 1` step (because it's after the inner call). That's correct — we ARE exiting this loop, so the depth should decrement.

- [ ] **Step 3: Wrap `run_while` similarly**

Find `fn run_while` (around line 240-some, just above run_for). Apply the same pattern:

```rust
fn run_while(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_while_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_while_inner(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    // (existing body)
}
```

Replace `run_while_inner`'s body-result match arms (formerly at line 274-281) with the four-arm pattern:

```rust
match execute_sequence_body(&clause.body, shell, sink) {
    ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),

    ExecOutcome::LoopBreak(1) => {
        last = ExecOutcome::Continue(0);
        break;
    }
    ExecOutcome::LoopBreak(n) => {
        return ExecOutcome::LoopBreak(n - 1);
    }

    ExecOutcome::LoopContinue(1) => {
        last = ExecOutcome::Continue(0);
        // fall through — the loop re-tests the condition
    }
    ExecOutcome::LoopContinue(n) => {
        return ExecOutcome::LoopContinue(n - 1);
    }

    ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
    ExecOutcome::Continue(c) => {
        last = ExecOutcome::Continue(c);
    }
}
```

- [ ] **Step 4: Wrap `run_arith_for` similarly**

Find `fn run_arith_for` (around line 354 per v78). Same pattern:

```rust
fn run_arith_for(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_arith_for_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_arith_for_inner(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    // (existing body — currently in run_arith_for)
}
```

Replace `run_arith_for_inner`'s body-result match arms (formerly around line 397-411) with the four-arm pattern. Note this loop has a step phase after the body match — make sure `LoopContinue(1)` falls through to the step, while `LoopContinue(n>1)` returns immediately (skipping this loop's step):

```rust
match execute_sequence_body(&clause.body, shell, sink) {
    ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),

    ExecOutcome::LoopBreak(1) => {
        last = ExecOutcome::Continue(0);
        break;
    }
    ExecOutcome::LoopBreak(n) => {
        return ExecOutcome::LoopBreak(n - 1);
    }

    ExecOutcome::LoopContinue(1) => {
        last = ExecOutcome::Continue(0);
        // fall through — to step (existing arith-for behavior)
    }
    ExecOutcome::LoopContinue(n) => {
        return ExecOutcome::LoopContinue(n - 1);
        // (returning here SKIPS this loop's step — matches bash:
        // continue 2 from inner skips inner step, outer loop's step
        // still runs when the bubbled LoopContinue(1) reaches it.)
    }

    ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
    ExecOutcome::Continue(c) => {
        last = ExecOutcome::Continue(c);
    }
}
```

- [ ] **Step 5: Modify `call_function` to save/restore `shell.loop_depth`**

Edit `src/executor.rs::call_function` (around line 1661). Add the save+restore around the existing body. Find the existing structure:

```rust
pub(crate) fn call_function(
    name: &str,
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    shell.local_scopes.push(std::collections::HashMap::new());

    let result = run_command(&body, shell, sink);

    // ... existing teardown ...
}
```

Add the loop_depth save right after the existing saves, set to 0 inside the function:

```rust
pub(crate) fn call_function(
    name: &str,
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    let saved_loop_depth = std::mem::replace(&mut shell.loop_depth, 0);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    shell.local_scopes.push(std::collections::HashMap::new());

    let result = run_command(&body, shell, sink);

    // ... existing RETURN trap + local scope pop (unchanged) ...

    shell.function_arg0.pop();
    shell.positional_args = saved;
    shell.loop_depth = saved_loop_depth;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}
```

(Place the restore just before the existing `positional_args` restore, or after — order doesn't matter, just consistency. The `match result { ... }` at the end is unchanged.)

- [ ] **Step 6: Build clean + run existing tests**

```bash
cargo build 2>&1 | tail -5
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 2 implementation:", sum}'
```

Expected: 2292 tests pass (unchanged — Task 2 implementation doesn't add tests yet; that's Step 7).

- [ ] **Step 7: Add 8 executor unit tests**

Append a new `mod loop_levels_executor_tests` block at the bottom of `src/executor.rs`. The pattern uses `crate::shell::process_line(input, &mut shell, false)` to drive end-to-end behavior; look at the existing `mod arith_for_tests` block for the helper-import pattern.

```rust
#[cfg(test)]
mod loop_levels_executor_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn break_in_inner_loop_exits_inner_only() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2; do for j in a b; do if [ \"$j\" = \"b\" ]; then break; fi; done; done",
            &mut sh,
            false,
        );
        // After: outer iterates both 1 and 2; each time inner exits at j=b after running j=a once.
        // No direct way to count iterations without a counter — but loop_depth should be 0 after exit.
        assert_eq!(sh.loop_depth, 0, "loop_depth not restored after nested-for break");
    }

    #[test]
    fn break_2_in_inner_loop_exits_both() {
        let mut sh = Shell::new();
        // Counter to verify outer loop didn't iterate again.
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2; do for j in a b; do break 2; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // x should still be 0 — break 2 exits before x=$((x+1)) runs.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn break_999_caps_in_two_loops() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2; do for j in a b; do break 999; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // Same as break 2 — cap to depth=2.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn continue_2_in_inner_loop_runs_outer_step() {
        let mut sh = Shell::new();
        // continue 2 from inner: skip rest of inner, advance outer
        // (which here is a for-loop so "step" = next value).
        // Counter sums outer iterations that did NOT trigger continue 2.
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2 3; do for j in a; do continue 2; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // x should be 0 — `continue 2` skips the x=... line each outer iteration.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn break_inside_function_called_from_loop_errors() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "f() { break; }; for i in 1 2; do f; done; echo done",
            &mut sh,
            false,
        );
        // The break inside f errors (loop_depth=0 inside the function);
        // for-loop continues; we should reach echo done. After all this,
        // loop_depth is back to 0.
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_zero_after_loop_exits() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2 3; do :; done",
            &mut sh,
            false,
        );
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_zero_after_nested_loop_exits() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2; do for j in a b; do :; done; done",
            &mut sh,
            false,
        );
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_restored_after_function_return() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "f() { for j in a b; do :; done; }; for i in 1 2; do f; done",
            &mut sh,
            false,
        );
        // Both outer for-loop (depth +1) and inner function-then-for
        // should leave loop_depth at 0.
        assert_eq!(sh.loop_depth, 0);
    }
}
```

If `Shell::lookup_var` returns a different shape (e.g., wraps in Variable), adjust the assertions per existing convention (`grep -n "fn lookup_var" src/shell_state.rs`).

- [ ] **Step 8: Run the new tests**

Run: `cargo test --quiet loop_levels_executor_tests 2>&1 | tail -12`
Expected: 8 tests pass.

- [ ] **Step 9: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 2:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```

Expected: **2300 tests pass** (2292 + 8 new). Clippy clean.

- [ ] **Step 10: Smoke-test the binary**

```bash
echo 'for i in 1 2 3; do for j in a b c; do echo "$i$j"; if [ "$j" = "b" ]; then break 2; fi; done; done' | cargo run --quiet
```

Expected: `1a\n1b\n` only (break 2 exits both loops after first inner-b).

```bash
echo 'for i in 1 2 3; do for j in a b; do if [ "$j" = "a" ]; then continue 2; fi; echo "$i$j"; done; done' | cargo run --quiet
```

Expected: empty output (continue 2 skips all inner-b iterations).

```bash
echo 'f() { break; }; for i in 1; do f; done; echo done' | cargo run --quiet
```

Expected: `done` on stdout; `huck: break: only meaningful in a 'for', 'while', or 'until' loop` (exact glyphs vary) on stderr.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v79 task 2: loop runner decrement-and-bubble + call_function save/restore

run_for, run_while, and run_arith_for split into single-return-path
wrappers that inc/dec shell.loop_depth around their inner body. Each
inner function's body-result match arms now use the four-arm
decrement-and-bubble pattern:

- LoopBreak(1)  → this loop is consumed; sets last=Continue(0); break.
- LoopBreak(n)  → bubble LoopBreak(n-1) to the outer loop.
- LoopContinue(1) → fall through to next iteration (and step, in
                    arith-for).
- LoopContinue(n) → bubble LoopContinue(n-1); skip this loop's step.

Because the builtin caps level to loop_depth, the bubble can never
reach level 0 at the wrong place — the outermost relevant loop sees
n=1 and consumes it.

call_function (src/executor.rs) saves shell.loop_depth at entry,
sets it to 0 for the function body, restores on exit. This means
`break` inside a function called from a loop correctly errors as
out-of-loop instead of escaping the caller's loop.

8 new unit tests in mod loop_levels_executor_tests exercise the
multi-level semantics end-to-end via crate::shell::process_line:
break_in_inner_loop, break_2, break_999 (cap), continue_2,
break_inside_function (with depth restored), loop_depth_zero after
single/nested/with-function exits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/loop_levels_integration.rs` — 6 binary-driven integration tests.
- Create: `tests/scripts/loop_levels_diff_check.sh` — executable bash-diff harness, 8 fragments.
- Modify: `docs/bash-divergences.md` — flip M-30 to `[fixed v79]` with corrected entry text (no longer falsely groups `return N`); change-log entry; Summary table tier count + "Last updated" stamp refresh.
- Modify: `README.md` — v79 iteration row.

**Goal:** End-to-end bash-compat verification; documentation accurately describes what was deferred (break/continue level only) vs what was always correct (return N).

### Steps

- [ ] **Step 1: Create `tests/loop_levels_integration.rs`**

```rust
//! Integration tests for v79 break N / continue N loop levels.
//! Drives the `huck` binary via stdin and asserts on stdout/exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn break_2_in_nested_for_exits_both() {
    let script = r#"for i in 1 2; do for j in a b; do echo "$i$j"; break 2; done; done
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "1a\n");
}

#[test]
fn continue_2_in_nested_for_advances_outer() {
    let script = r#"for i in 1 2 3; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo "$i$j"; done; done
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    // For each outer i, the inner iterates j=a (printed), then j=b
    // triggers continue 2 (skips rest of inner; advances outer).
    assert_eq!(out, "1a\n2a\n3a\n");
}

#[test]
fn break_overshoot_caps_to_depth() {
    let script = "for i in 1; do break 999; done; echo ok\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
}

#[test]
fn break_outside_any_loop_errors() {
    let (_, err, code) = run_huck("break\necho should_not_see\n");
    // The break errors with status 1, but the script continues to
    // the next command (echo). Final exit code is 0 (echo's status).
    assert_eq!(code, 0);
    assert!(
        err.contains("only meaningful in a"),
        "stderr should mention 'only meaningful': {err:?}",
    );
}

#[test]
fn break_inside_function_called_from_loop_errors() {
    let script = r#"f() { break; }
for i in 1; do f; done
echo ok
"#;
    let (out, err, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
    assert!(err.contains("only meaningful"), "stderr: {err:?}");
}

#[test]
fn mixed_for_while_break_2() {
    let script = r#"i=0
for outer in 1 2 3; do
    while [ "$i" -lt 5 ]; do
        i=$((i+1))
        if [ "$i" -ge 2 ]; then break 2; fi
    done
done
echo "i=$i"
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "i=2\n");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test loop_levels_integration --quiet 2>&1 | tail -10`
Expected: 6 tests pass.

- [ ] **Step 3: Create `tests/scripts/loop_levels_diff_check.sh`**

```bash
#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for v79 `break N` / `continue N`
# loop levels and the bash-style "outside loop" diagnostic. Each
# fragment runs through `bash` and `huck` via stdin (huck has no -c
# flag); outputs must be byte-identical.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(printf '%s\n' "$fragment" | bash 2>&1; echo "EXIT:$?")
    huck_out=$(printf '%s\n' "$fragment" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. break 2 in nested for.
check "break 2 nested for" \
      'for i in 1 2; do for j in a b; do echo $i$j; break 2; done; done'

# 2. continue 2 in nested for.
check "continue 2 nested for" \
      'for i in 1 2 3; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo $i$j; done; done'

# 3. break overshoot caps to depth.
check "break overshoot cap" \
      'for i in 1; do break 999; done; echo ok'

# 4. break outside loop — error message + exit code.
check "break outside loop" \
      'break; echo $?'

# 5. continue outside loop.
check "continue outside loop" \
      'continue; echo $?'

# 6. break with non-numeric arg.
check "break abc error" \
      'for i in 1; do break abc; done; echo $?'

# 7. break with zero arg.
check "break 0 error" \
      'for i in 1; do break 0; done; echo $?'

# 8. break with negative arg.
check "break -1 error" \
      'for i in 1; do break -1; done; echo $?'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
```

Make executable:

```bash
chmod +x tests/scripts/loop_levels_diff_check.sh
```

- [ ] **Step 4: Build and run the harness**

```bash
cargo build --quiet
tests/scripts/loop_levels_diff_check.sh
```

Expected: `Total: 8, Pass: 8, Fail: 0`.

**Common failure modes**:

- **Diagnostic glyph mismatch**: bash uses ASCII backticks/single-quotes in `only meaningful in a 'for'...`. huck should match exactly. If huck emits Unicode glyphs (`'` vs `'`), fix the eprintln string. The plan's Step 7 of Task 1 specifies the message text — verify it uses straight ASCII.

- **Exit code wrapping**: bash returns 1 for outside-loop, 128 for malformed-N. The harness's `echo $?` captures the LAST command's status — for `break; echo $?` the `break` produces status 1 (from huck) / 1 (from bash); then `$?` is 1. Verify both emit identical "1" on stdout AND identical diagnostic on stderr.

- **`break -1`**: bash parses `-1` as a separate flag — actually no, bash's `break -1` triggers "loop count out of range" because the integer is parsed and rejected as non-positive. Verify huck does the same.

If any fragment fails, investigate the diff. If a divergence is intentional, document it with a `# DIVERGES: <why>` comment and exclude.

- [ ] **Step 5: Update `docs/bash-divergences.md` — flip M-30**

Edit `docs/bash-divergences.md`. Find:

```
- **M-30: `break N` / `continue N` / `return N` (level)** — `[deferred]` medium. huck: argument silently ignored beyond 1. bash: exits N enclosing loops.
```

Replace with:

```
- **M-30: `break N` / `continue N` loop levels** — `[fixed v79]` medium. `break [N]` exits N enclosing loops (default 1, capped to actual loop depth); `continue [N]` continues the Nth enclosing loop. New `ExecOutcome::LoopBreak(u32)` / `LoopContinue(u32)` payload (1-based level). New `Shell.loop_depth: u32` field incremented by `run_for` / `run_while` / `run_arith_for` via single-return-path wrappers (`run_*_inner` holds the body); saved+restored across `call_function` so `break` in a function called from a loop correctly errors as "only meaningful in a `for', `while', or `until' loop" + exit 1 (previously silent exit 0). Loop runners use a decrement-and-bubble pattern: `LoopBreak(1)` is consumed by this loop; `LoopBreak(n>1)` bubbles as `n-1` to the outer loop. `continue N` from an inner loop skips this loop's step but the outer loop's step (e.g., `for ((i++; ...))` advancement) still runs. New `builtin_break` / `builtin_continue` / extracted `builtin_return` helpers + shared `parse_loop_level` in `src/builtins.rs`. Exit status 128 for `break 0` / `break -1` / `break abc` (loop count out of range / numeric argument required, bash compat). Excess trailing args silently ignored (matches bash). **Note**: the previous M-30 title misleadingly grouped `return N` with break/continue. `return N` was already correct in v0 — N is the exit status, not a loop level. The `return abc` error-path divergence (bash: status 128 "numeric argument required"; huck: silent fallback to `$?`) is tracked separately as a possible future low-impact item.
```

- [ ] **Step 6: Add the change-log entry**

Find the change-log section at the bottom of `docs/bash-divergences.md`. Add a new dated entry chronologically (after the v78 entry):

```
- **2026-06-03**: M-30 (`break N` / `continue N` loop levels) shipped as v79. Also closes a previously-uncatalogued gap: `break` / `continue` outside any loop now produces a bash-style diagnostic + exit 1 (was silent exit 0). `ExecOutcome::LoopBreak` and `LoopContinue` gain a `u32` level payload (1-based; capped to actual loop depth by the builtin so loop runners never see overshoot). New `Shell.loop_depth: u32` field; incremented by `run_for` / `run_while` / `run_arith_for` (saturating ops); saved+restored across `call_function` so a `break` in a function called from a loop correctly errors as out-of-loop. Loop runners use the decrement-and-bubble pattern: `LoopBreak(1)` is consumed by this loop; `LoopBreak(n>1)` bubbles as `n-1`. Same for Continue. New `builtin_break` / `builtin_continue` / extracted `builtin_return` helpers in `src/builtins.rs`. Bash status 128 for `break 0` / `break -1` / `break abc` (loop count out of range / numeric argument required). Excess trailing args silently ignored (matches bash). `return N` was already correct in v0; the M-30 entry's title misleadingly grouped it with break/continue — entry text now corrects this. 10 builtin unit tests + 8 executor unit tests + 6 integration tests + 8 bash-diff fragments byte-identical to bash 5.2 (huck's 7th harness). Total grows by ~32.
```

- [ ] **Step 7: Refresh Summary table tier count + "Last updated" stamp**

Run:

```bash
grep -c '^- \*\*M-' docs/bash-divergences.md
grep '^- \*\*M-' docs/bash-divergences.md | grep -c '\[deferred\]'
```

M-30 flipped from deferred to fixed; deferred count should decrease by 1. Update the Tier 2 row's Notes column to append `; M-30 fixed by v79`. Tier 2 count column stays the same (M-30 was always in the list; status flipped).

Update the "Last updated" line (around line 3) to `Last updated: 2026-06-03 (after v79 break N / continue N)`.

- [ ] **Step 8: Update `README.md`**

Edit `README.md`. Find the iteration table. Add a v79 row immediately after v78, matching the existing column structure:

```
| v79       | `break N` / `continue N` loop levels (M-30)                              |
```

(Adjust spacing to match the existing v78 row's format. The third-column ID is `M-30`.)

- [ ] **Step 9: Final full test suite + harness run**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 3:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
cargo build --quiet && tests/scripts/loop_levels_diff_check.sh
```

Expected:
- **2306 tests pass** (2300 + 6 integration).
- Clippy clean.
- Bash-diff: 8/8 byte-identical.

Also confirm the 6 prior bash-diff harnesses still pass:

```bash
for h in arrays ifs test_combinators completion function_keyword arith_for; do
    tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```

Expected: each reports `Total: N, Pass: N, Fail: 0` (no regression).

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v79 task 3: integration tests + bash-diff harness + docs

* tests/loop_levels_integration.rs (new): 6 binary-driven tests
  covering break 2 in nested for, continue 2 outer-advance, break
  overshoot caps, break outside any loop (diagnostic + script
  continues), break inside function-called-from-loop, mixed
  for+while with break 2.

* tests/scripts/loop_levels_diff_check.sh (new, +x): huck's 7th
  bash-diff harness, 8 fragments byte-identical to bash 5.2 covering
  break/continue N nested, overshoot cap, outside-loop diagnostics,
  malformed-N status 128.

* docs/bash-divergences.md:
  - M-30 flipped from [deferred] to [fixed v79] with full surface
    description AND corrected entry text — the original title falsely
    grouped `return N` (already correct in v0) with break/continue.
    Entry now notes return's correctness and tracks return's
    error-path divergence as a possible future L-* item.
  - 2026-06-03 change-log entry added.
  - Summary table Notes column + "Last updated" stamp refreshed.

* README.md: v79 iteration row appended below v78.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist

Before merging the branch, the controller dispatches a final code-reviewer over the whole branch diff. Specific things to verify:

- [ ] **All 2306 tests pass on the branch.**
- [ ] **Clippy clean (`cargo clippy --all-targets`).**
- [ ] **The bash-diff harness reports 8/8.**
- [ ] **All other bash-diff harnesses still pass** (arrays, ifs, test_combinators, completion, function_keyword, arith_for).
- [ ] **`break` outside any loop produces stderr + exit 1.**
- [ ] **`break` inside a function called from a loop produces stderr + exit 1** (loop_depth correctly zeroed in call_function).
- [ ] **`break 2` in 2-deep nest exits both loops.**
- [ ] **`break 999` in 2-deep nest caps to 2.**
- [ ] **`continue 2` from inner loop runs outer step + advances.**
- [ ] **`shell.loop_depth` is 0 after every loop exits** (no leaks).
- [ ] **`break 0` / `break -1` / `break abc` produce exit 128 + diagnostic.**
- [ ] **`return N` continues to work** (unchanged from v0).
- [ ] **`docs/bash-divergences.md` M-30 entry comprehensive and accurate;** correctly notes return's earlier correctness.

## Merge

After review fixes land, merge with `--no-ff`:

```bash
git checkout main
git merge --no-ff v79-loop-levels -m "Merge v79: break N / continue N loop levels (M-30)"
git push origin main
git branch -d v79-loop-levels
```

Then update the long-running memory files (`huck_iterations.md` + `MEMORY.md`) per the iteration workflow.
