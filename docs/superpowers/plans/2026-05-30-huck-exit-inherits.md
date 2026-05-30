# huck v57 — `exit` inherits `$?` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make bare `exit` (no args) inherit `$?` like bash.

**Architecture:** One-line semantic change in `builtin_exit`
(widen signature to take `&Shell`; replace `Exit(0)` no-args arm
with `Exit(shell.last_status())`).

**Tech Stack:** Rust. No new deps.

**Spec:** `docs/superpowers/specs/2026-05-30-huck-exit-inherits-design.md`

**Branch:** `v57-exit-inherits`.

**Commit trailer:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1**

```bash
git checkout main
git pull --ff-only
git checkout -b v57-exit-inherits
```

Spec + this plan are committed as the first commit by the
controller before Task 1.

---

## Task 1: Fix + 2 unit tests + 2 integration tests

**Files:**
- Modify `src/builtins.rs` — signature + body + dispatch.
- Create `tests/exit_inherits_integration.rs`.

### Step 1.1: Update `builtin_exit` signature + body

Current at `src/builtins.rs:255-266`:

```rust
fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                eprintln!("huck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}
```

Replace with:

```rust
fn builtin_exit(args: &[String], shell: &Shell) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(shell.last_status()),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                eprintln!("huck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}
```

`shell.last_status()` is already public per v54 and used widely.

- [ ] **Step 1.1**

### Step 1.2: Update the dispatch arm

In `run_builtin`'s match block, the current arm is:

```rust
"exit" => builtin_exit(args),
```

Change to:

```rust
"exit" => builtin_exit(args, shell),
```

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.3**

### Step 1.4: Append 2 unit tests

At the end of `src/builtins.rs` (after the existing v56 `mod
printf_tests`):

```rust
#[cfg(test)]
mod exit_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn exit_no_args_inherits_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let outcome = builtin_exit(&[], &shell);
        assert!(matches!(outcome, ExecOutcome::Exit(42)));
    }

    #[test]
    fn exit_no_args_inherits_zero_when_clean() {
        let shell = Shell::new();
        let outcome = builtin_exit(&[], &shell);
        assert!(matches!(outcome, ExecOutcome::Exit(0)));
    }
}
```

`set_last_status` exists on Shell. If it has a different name in
the codebase, adapt.

- [ ] **Step 1.4**

### Step 1.5: Create `tests/exit_inherits_integration.rs`

Match `tests/read_integration.rs`'s helper shape (returns
`(stdout, stderr, exit_code: i32)` triple):

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn bare_exit_after_false_returns_1() {
    let (_out, _err, rc) = run_capture("false\nexit\n");
    assert_eq!(rc, 1, "expected exit code 1 (inheriting `false`'s status); got {rc}");
}

#[test]
fn bare_exit_after_true_returns_0() {
    let (_out, _err, rc) = run_capture("true\nexit\n");
    assert_eq!(rc, 0, "expected exit code 0 (inheriting `true`'s status); got {rc}");
}
```

- [ ] **Step 1.5**

### Step 1.6: Run tests

```bash
cargo test --bin huck exit_tests
cargo test --test exit_inherits_integration -- --nocapture
cargo test --tests
cargo clippy --all-targets -- -D warnings
```

Expected: 2 new unit tests pass; 2 new integration tests pass;
full integration suite green (PTY flake tolerated).

**Watch for** existing tests that may have depended on the old
"bare `exit` always exits 0" behavior. If any prior integration
test ran `<failing-cmd>; exit` and asserted exit 0, it'll now
fail correctly. Investigate any such failure and either:
- Update the test if its assertion was wrong (it was relying on
  the bug).
- Or, if a test genuinely needs to exit 0 regardless, change it
  to `exit 0` explicitly.

- [ ] **Step 1.6**

### Step 1.7: Commit Task 1

```bash
git add src/builtins.rs tests/exit_inherits_integration.rs
git commit -m "$(cat <<'EOF'
builtin: exit inherits $? when called without args (v57)

Match bash/POSIX: bare `exit` exits with the last command's
status, not 0. `exit N` (explicit) unchanged.

Widen `builtin_exit`'s signature to `(&[String], &Shell)` and
change the no-args arm from `Exit(0)` to
`Exit(shell.last_status())`. Update the dispatch in `run_builtin`
to pass `shell`. `last_status` is already public.

The kernel's `_exit` truncates to a byte regardless, so no
explicit mod-256 needed on the inherited form (matching bash —
bash also doesn't mod-256 here; only the explicit-N form does).

Surfaced by v56's printf implementer: integration test
`printf_no_args_usage_error` had to use `rc=$?` capture instead
of relying on `exit` to propagate printf's status-2. v57 fixes
the underlying behavior; that test is unchanged (still
correct).

2 unit tests in mod exit_tests (no_args_inherits_last_status;
no_args_inherits_zero_when_clean) + 2 integration tests in
tests/exit_inherits_integration.rs (false-then-exit returns 1;
true-then-exit returns 0).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/builtins.rs tests/exit_inherits_integration.rs`.

- [ ] **Step 1.7**

---

## Task 2: Docs

**Files:**
- Modify `docs/bash-divergences.md`.
- Modify `README.md`.

### Step 2.1: Add M-74 entry

After M-73 (v56 `printf`):

```markdown
- **M-74: `exit` inherits `$?`** — `[fixed v57]` low. Bare `exit` (no args) now exits with the current `$?` instead of always 0, matching bash/POSIX. `exit N` (explicit) unchanged. Surfaced by v56's printf implementer: `printf_no_args_usage_error` had to use `rc=$?` capture instead of relying on `exit` to propagate printf's status-2 to the process exit code. `builtin_exit`'s signature widened from `(args)` to `(args, &Shell)` so the no-args arm can read `shell.last_status()`. The kernel's `_exit` truncates to a byte regardless, so no explicit mod-256 on the inherited form (matches bash).
```

- [ ] **Step 2.1**

### Step 2.2: Add v57 change-log entry

In `## Change log` after v56:

```markdown
- **2026-05-30**: M-74 (`exit` inherits `$?`) shipped as v57. One-line semantic fix in `src/builtins.rs::builtin_exit` — no-args arm now returns `ExecOutcome::Exit(shell.last_status())` instead of `Exit(0)`. Dispatch in `run_builtin` updated to pass `shell`. 2 unit tests in `mod exit_tests` + 2 binary-driven integration tests in `tests/exit_inherits_integration.rs`. No new L-* divergences.
```

- [ ] **Step 2.2**

### Step 2.3: Add v57 row to README

After the v56 row:

```markdown
| v57       | `exit` inherits `$?` (M-74)                                    |
```

Match v56 column padding.

- [ ] **Step 2.3**

### Step 2.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake
tolerated).

- [ ] **Step 2.4**

### Step 2.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.5**

### Step 2.6: Commit Task 2

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-74 (exit inherits \$?) fixed v57

New M-74 entry in docs/bash-divergences.md tracks the one-line
semantic fix to builtin_exit's no-args arm. References how v56's
printf implementer surfaced the divergence.

Change log: 2026-05-30 v57 entry summarizing the fix.

README: v57 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is three commits ahead of `main`: docs preamble + 2
   task commits.
4. Dispatch a final cross-task code-reviewer subagent over
   `main..v57-exit-inherits`.
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
