# huck v42 — `kill -s` + `kill -n` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-40 by adding `kill -s SIGNAME pid` and
`kill -n SIGNUM pid` long-form signal selection to huck's `kill`
builtin.

**Architecture:** All changes inside `src/builtins.rs`. Extract the
existing per-target send loop into a shared helper
`send_signal_to_targets`; add two new dispatch arms (`kill_with_s_flag`,
`kill_with_n_flag`); update the dispatcher and usage string. No new
types, no new modules.

**Tech Stack:** Rust. Reuses `signal_by_name` and
`crate::traps::killable_signals()` from v41.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-kill-s-design.md`

**Branch:** `v42-kill-s` (to be created in preamble step P.1).

**Commit trailer convention** (every commit in this iteration):

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v42-kill-s
```

Expected: `Switched to a new branch 'v42-kill-s'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Refactor + new dispatch arms + unit tests

**Files:**
- Modify: `src/builtins.rs` — extract `send_signal_to_targets`; add
  `kill_with_s_flag` and `kill_with_n_flag`; update `builtin_kill`
  dispatcher; bump usage strings; append 10 unit tests in
  `mod kill_tests`.

### Step 1.1: Extract `send_signal_to_targets`

In `src/builtins.rs`, find the existing `builtin_kill` function
(starts at line 800, `fn builtin_kill(args: &[String], out: &mut dyn
Write, shell: &mut Shell) -> ExecOutcome {`).

Find the `let mut any_failed = false;` line (currently at line 834)
and the closing `if any_failed { ExecOutcome::Continue(1) } else {
ExecOutcome::Continue(0) }` (currently at line 876). Cut out the
entire block from `let mut any_failed = false;` through the closing
brace before the function's final `}` (inclusive of the
`if any_failed { … }` expression).

Replace it in the original location with a single call:

```rust
    send_signal_to_targets(sig, targets, shell)
```

Then insert this new function **immediately below** `builtin_kill`'s
closing `}` (before `fn builtin_disown` at line 879):

```rust
/// Sends `sig` to each target (`%spec` or PID). Returns `Continue(1)`
/// if any send failed (with errors already on stderr), `Continue(0)`
/// otherwise. Shared between every kill dispatch arm.
fn send_signal_to_targets(
    sig: i32,
    targets: &[String],
    shell: &mut Shell,
) -> ExecOutcome {
    let mut any_failed = false;
    for target in targets {
        if let Some(_rest) = target.strip_prefix('%') {
            let id = match resolve_spec_or_error(target, "kill", shell) {
                Ok(id) => id,
                Err(_) => {
                    any_failed = true;
                    continue;
                }
            };
            let pgid = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => j.pgid,
                None => {
                    eprintln!("huck: kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            let rc = unsafe { libc::killpg(pgid, sig) };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                eprintln!("huck: kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match target.parse::<i32>() {
                Ok(pid) if pid > 0 => {
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = std::io::Error::last_os_error();
                        eprintln!("huck: kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                _ => {
                    eprintln!("huck: kill: {target}: arguments must be process or job IDs");
                    any_failed = true;
                }
            }
        }
    }
    if any_failed {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}
```

- [ ] **Step 1.1: Extract the send-loop**

### Step 1.2: Build to confirm refactor compiles

Run: `cargo build`
Expected: clean. No behavior change yet.

- [ ] **Step 1.2: Build clean after extraction**

### Step 1.3: Run existing kill tests to confirm no regression

Run: `cargo test kill_ -- --nocapture`
Expected: all previously-passing tests still pass (the refactor is a
pure extraction).

- [ ] **Step 1.3: Existing kill tests still green**

### Step 1.4: Bump usage strings

In `src/builtins.rs`, find both occurrences of the existing usage
string in `builtin_kill`:

```rust
eprintln!("huck: kill: usage: kill [-sig] pid | %job ...");
```

There are TWO such lines (one in the `args.len() < 2` arm, one in
the `else` arm for empty `args`). Replace BOTH with:

```rust
eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
```

(Use `Edit` tool with `replace_all: true` for safety, or run two
targeted Edits — your choice.)

- [ ] **Step 1.4: Update usage messages**

### Step 1.5: Add the `-s` and `-n` dispatch hooks

In `builtin_kill`, find the line where `-l` is dispatched:

```rust
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out);
    }
```

**Immediately after** that block (before the `let (sig, targets) =
if let Some(first) = args.first() {` line), insert:

```rust
    match args.first().map(|s| s.as_str()) {
        Some("-s") => return kill_with_s_flag(&args[1..], shell),
        Some("-n") => return kill_with_n_flag(&args[1..], shell),
        _ => {}
    }
```

- [ ] **Step 1.5: Insert `-s`/`-n` dispatch**

### Step 1.6: Add `kill_with_s_flag` helper

Insert **immediately above** `fn send_signal_to_targets` (which you
created in step 1.1):

```rust
/// Handles `kill -s SIGNAME [targets...]`. The `-s` token has already
/// been consumed by the dispatcher; `args` is everything after it.
fn kill_with_s_flag(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let name = match args.first() {
        Some(n) => n,
        None => {
            eprintln!("huck: kill: -s: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let sig = match signal_by_name(name) {
        Some(n) => n,
        None => {
            eprintln!("huck: kill: {name}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    let targets = &args[1..];
    if targets.is_empty() {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(sig, targets, shell)
}
```

- [ ] **Step 1.6: Add `kill_with_s_flag`**

### Step 1.7: Add `kill_with_n_flag` helper

Insert **immediately below** `kill_with_s_flag`, still above
`send_signal_to_targets`:

```rust
/// Handles `kill -n SIGNUM [targets...]`. The `-n` token has already
/// been consumed by the dispatcher; `args` is everything after it.
/// Number must be in `killable_signals()` (matching `kill -l`'s set).
fn kill_with_n_flag(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let num_arg = match args.first() {
        Some(s) => s,
        None => {
            eprintln!("huck: kill: -n: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let n = match num_arg.parse::<i32>() {
        Ok(n) if (1..=64).contains(&n) => n,
        _ => {
            eprintln!("huck: kill: {num_arg}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    if !crate::traps::killable_signals()
        .iter()
        .any(|(_, num)| *num == n)
    {
        eprintln!("huck: kill: {num_arg}: invalid signal specification");
        return ExecOutcome::Continue(1);
    }
    let targets = &args[1..];
    if targets.is_empty() {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(n, targets, shell)
}
```

- [ ] **Step 1.7: Add `kill_with_n_flag`**

### Step 1.8: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.8: Build clean**

### Step 1.9: Add the 10 new unit tests

Find the `mod kill_tests` block in `src/builtins.rs` (starts at line
2102: `#[cfg(test)] mod kill_tests { use super::*; use
crate::shell_state::Shell; ... }`). Append these tests inside that mod
block, before its closing `}`:

```rust
    #[test]
    fn kill_s_with_name_resolves_and_dispatches() {
        // Self-signal SIGWINCH to test process; verifies dispatch
        // path returns success without asserting signal arrival.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "WINCH".to_string(), pid],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_with_sig_prefix_resolves() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "SIGWINCH".to_string(), pid],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_lowercase_name_resolves() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "winch".to_string(), pid],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-s".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_s_invalid_name_returns_status_1() {
        // PID 99999 may or may not exist; the parse error fires before
        // any send is attempted.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "BOGUS".to_string(), "99999".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_s_no_targets_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "TERM".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_n_with_number_resolves_and_dispatches() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &[
                "-n".to_string(),
                libc::SIGWINCH.to_string(),
                pid,
            ],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_n_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_n_invalid_number_returns_status_1() {
        // 99 isn't in killable_signals(); parse error before send.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-n".to_string(), "99".to_string(), "12345".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_dash_sig_short_form_still_works_after_refactor() {
        // Regression: `kill -WINCH <getpid>` should still dispatch
        // through the refactored send_signal_to_targets.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-WINCH".to_string(), pid],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
```

The mod already has `use super::*;` and `use crate::shell_state::Shell;`,
so `signal_by_name`, `run_builtin`, `ExecOutcome`, `Shell`, and `libc`
are all in scope.

- [ ] **Step 1.9: Append the 10 tests**

### Step 1.10: Run the new tests

Run: `cargo test kill_s_ kill_n_ kill_dash_sig -- --nocapture`
Expected: all 10 new tests pass.

The self-signal tests (`*_resolves_and_dispatches`,
`kill_dash_sig_short_form_still_works_after_refactor`) DO send SIGWINCH
to the test process. SIGWINCH default action is to ignore, so this is
safe — the test runner doesn't care about WINCH.

- [ ] **Step 1.10: New tests pass**

### Step 1.11: Run all kill_ tests for regression

Run: `cargo test kill_ -- --nocapture`
Expected: all prior `kill_*` and `kill_l_*` tests still pass.

- [ ] **Step 1.11: All kill_ tests green**

### Step 1.12: Full unit-test suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.12: Full unit suite passes**

### Step 1.13: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.13: Clippy clean**

### Step 1.14: Commit

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: kill -s SIGNAME + kill -n SIGNUM (v42 task 1)

Add long-form signal selection to the kill builtin. Extract the
existing per-target send loop into a shared
send_signal_to_targets(sig, targets, shell) helper so all four
dispatch paths (-s, -n, -<sig>, default-SIGTERM) share the same
sender.

New helpers:
- kill_with_s_flag: parses `-s NAME` via signal_by_name (already
  handles SIG prefix + case-insensitivity from v41 dedup).
- kill_with_n_flag: parses `-n NUM` and validates that the number
  is in killable_signals() (matches kill -l's accepted set).

Error messages:
- `kill -s` (no name) → "option requires an argument" + 2
- `kill -n` (no number) → same + 2
- `kill -s BOGUS pid` → "invalid signal specification" + 1
- `kill -n 99 pid` → same + 1
- `kill -s TERM` (no targets) → usage error 2

Usage string bumped to include both new flags. The existing
`-<sig>` short form and default-SIGTERM behavior unchanged.

10 new unit tests in mod kill_tests cover: -s name/SIG-prefix/
lowercase, -s missing arg / invalid name / no targets, -n number /
missing arg / invalid number, plus a regression test confirming
`kill -<sig>` still dispatches through the refactored helper.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.14: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/kill_s_integration.rs`

Three binary-driven tests using the established harness pattern
(mirrors `tests/kill_l_integration.rs` and
`tests/wait_integration.rs`).

### Step 2.1: Create the integration test file

Create `tests/kill_s_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
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
    )
}

#[test]
fn kill_s_invalid_name_errors_status_1() {
    // `kill -s BOGUS 99999` exits 1 before any send is attempted.
    let (out, _) = run("kill -s BOGUS 99999\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {:?}", out);
}

#[test]
fn kill_s_missing_arg_errors_status_2() {
    let (out, _) = run("kill -s\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {:?}", out);
}

#[test]
fn kill_n_invalid_number_errors_status_1() {
    // 99 isn't in killable_signals(); parse error before send.
    let (out, _) = run("kill -n 99 99999\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {:?}", out);
}
```

`libc` is already in `[dev-dependencies]` from v41 — no Cargo.toml
change needed.

- [ ] **Step 2.1: Create the integration test file**

### Step 2.2: Run the new integration suite

Run: `cargo test --test kill_s_integration -- --nocapture`
Expected: all 3 tests pass.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all pass. PTY flake `pty_compound_stage_pipeline_stops_and_resumes`
may flake under load — re-run in isolation if hit.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/kill_s_integration.rs
git commit -m "$(cat <<'EOF'
test: kill -s + kill -n integration coverage (v42 task 2)

Three binary-driven tests verifying the error paths that don't
require sending real signals: invalid signal name with -s
(status 1), missing arg after -s (status 2), invalid signal number
with -n (status 1). The signal-delivery happy path is exercised by
the Task 1 unit tests via self-signaling SIGWINCH to getpid().

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — flip M-40, change-log entry.
- Modify: `README.md` — v42 row.

### Step 3.1: Flip M-40 in `docs/bash-divergences.md`

Find the M-40 entry in the Job-control section. After v41 it reads:

```markdown
- **M-40: `kill -s SIGNAME`** — `[deferred]` medium. huck: only `-NAME` form (e.g. `-TERM`). bash: accepts `-s TERM`.
```

Replace with:

```markdown
- **M-40: `kill -s SIGNAME` / `kill -n SIGNUM`** — `[fixed v42]` medium. Both bash long-form flags supported: `kill -s NAME pid` (case-insensitive, optional `SIG` prefix) and `kill -n NUM pid` (NUM must be in `killable_signals()`). All four dispatch arms (`-s`, `-n`, `-<sig>`, default-SIGTERM) share a `send_signal_to_targets` helper. Existing `kill -TERM pid` / `kill -15 pid` / `kill pid` paths unchanged.
```

- [ ] **Step 3.1: Flip M-40**

### Step 3.2: Add v42 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most recent
`**2026-05-28**` entry (v41, M-39). Add IMMEDIATELY after it:

```markdown
- **2026-05-28**: M-40 (`kill -s SIGNAME` + `kill -n SIGNUM`) shipped as v42. Extracted the existing per-target send loop into a shared `send_signal_to_targets` helper; added `kill_with_s_flag` and `kill_with_n_flag` dispatch arms. Reuses v41's `signal_by_name` (SIG-prefix + case-insensitive) and `killable_signals()` (16-entry table). Usage string bumped. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v42 row to README

In `README.md`, find the version table. After the v41 row (search for
`| v41       |`), add IMMEDIATELY after it:

```markdown
| v42       | `kill -s SIGNAME` + `kill -n SIGNUM` (M-40)                    |
```

So the final block reads:

```markdown
| v40       | `wait -n` + multi-arg `wait` (M-37 + M-38)                    |
| v41       | `kill -l` (M-39) + README cleanup                              |
| v42       | `kill -s SIGNAME` + `kill -n SIGNUM` (M-40)                    |
```

Match the column padding of v40/v41 — count the spaces before the
closing `|` so the right pipe lines up.

- [ ] **Step 3.3: Add README v42 row**

### Step 3.4: Run the full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo known PTY flake).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-40 fixed; v42 in README

Job-control section: M-40 (`kill -s` + `kill -n`) flipped from
[deferred] to [fixed v42] with descriptive text covering the
case-insensitive / SIG-prefix-stripping name lookup and the
killable_signals() validation for numeric form.

Change log: 2026-05-28 v42 entry summarizing the
send_signal_to_targets extraction and the two new dispatch arms.

README: v42 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`: docs
   preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the full
   diff (`main..v42-kill-s`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory entry with v42.
