# huck v15: PTY Interactive Test Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a PTY-based golden-path regression suite for huck's interactive features — tab completion, history recall, and Ctrl-C handling — using the `expectrl` crate.

**Architecture:** A single new test file `tests/pty_interactive.rs` holds a small harness (spawn-in-PTY helper, keystroke helpers, key constants) and ~13 golden-path tests. v15 changes **no `src/` code**; the only production-tree change is adding `expectrl` to `[dev-dependencies]`.

**Tech Stack:** Rust 2024 edition, `expectrl` (new dev-dependency), `tempfile` (already a dev-dependency).

**Reference:** Design spec at `docs/superpowers/specs/2026-05-21-huck-pty-test-harness-design.md`.

**Note on `expectrl` API:** The exact method names/signatures of `expectrl` (spawn, send, expect, timeout) depend on the version `cargo add` resolves. Task 1 concentrates **all** `expectrl` API calls inside four helper functions; Tasks 2–4 use only those helpers. If `expectrl`'s API differs from the example code below, adapt **only the Task 1 helpers** — the helper signatures are the stable contract the tests depend on.

---

## File Map

- **Create:** `tests/pty_interactive.rs` — harness helpers + ~13 golden-path tests
- **Modify:** `Cargo.toml` — add `expectrl` to `[dev-dependencies]`
- **Modify:** `README.md` — v15 row, PTY-suite note, test count
- **No `src/` changes.**

---

## Task 1: `expectrl` dependency, harness, and smoke test

Add the dependency, create `tests/pty_interactive.rs` with the harness helpers, and write the single smoke test that validates the harness end to end.

**Files:**
- Modify: `Cargo.toml`
- Create: `tests/pty_interactive.rs`

- [ ] **Step 1: Add the `expectrl` dev-dependency**

Run: `cargo add expectrl --dev`

This pins whatever current version `cargo` resolves. Confirm `Cargo.toml` `[dev-dependencies]` now lists `expectrl` alongside `tempfile`.

- [ ] **Step 2: Create `tests/pty_interactive.rs` with the harness and smoke test**

Create the file with this content. The four helper functions (`try_spawn`, `send`, `expect`, `expect_eof`) are the only place `expectrl` is touched — if the `expectrl` API differs from what is written here, adapt these helpers and nothing else.

```rust
//! PTY-based golden-path tests for huck's interactive features
//! (tab completion, history recall, Ctrl-C handling).
//!
//! These need a real pseudo-terminal so rustyline runs in interactive
//! mode. If PTY allocation fails (a restricted sandbox), each test
//! logs a skip notice and returns — a pass. A genuinely broken huck
//! binary is still caught by the piped-stdin integration suites.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use expectrl::Session;

// Keystroke encodings sent over the PTY master.
const TAB: &str = "\t";
const ENTER: &str = "\r";
const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const CTRL_C: &str = "\x03";
const CTRL_D: &str = "\x04";

/// Spawns the huck binary attached to a fresh PTY, in `cwd`, with the
/// given environment overrides applied on top of the inherited env.
/// Returns `None` (after logging) if PTY allocation fails — the test
/// then skips.
fn try_spawn(cwd: &Path, env: &[(&str, &str)]) -> Option<Session> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    cmd.current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match Session::spawn(cmd) {
        Ok(mut session) => {
            session.set_expect_timeout(Some(Duration::from_secs(10)));
            Some(session)
        }
        Err(e) => {
            eprintln!("pty_interactive: skipping — no PTY available: {e}");
            None
        }
    }
}

/// Sends raw bytes (text or control sequences) to the PTY.
fn send(session: &mut Session, bytes: &str) {
    session
        .send(bytes)
        .unwrap_or_else(|e| panic!("send {bytes:?} failed: {e}"));
}

/// Reads the PTY stream until `needle` appears, or panics on timeout.
/// `needle` is matched literally (not as a regex).
fn expect(session: &mut Session, needle: &str) {
    session
        .expect(needle)
        .unwrap_or_else(|e| panic!("expected {needle:?} but: {e}"));
}

/// Reads until the session ends (the child exited and the PTY closed).
fn expect_eof(session: &mut Session) {
    session
        .expect(expectrl::Eof)
        .unwrap_or_else(|e| panic!("expected session EOF but: {e}"));
}

/// Builds a `(HISTFILE=...)` env pointing into `dir`, isolating
/// history per test. Returns the env vec; the caller keeps `dir`
/// alive for the test's duration.
fn histfile_env(dir: &Path) -> Vec<(&'static str, String)> {
    let hist = dir.join("huck_history");
    vec![("HISTFILE", hist.to_string_lossy().into_owned())]
}

/// Converts an owned-value env vec to the borrowed form `try_spawn`
/// expects.
fn env_refs(env: &[(&'static str, String)]) -> Vec<(&str, &str)> {
    env.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[test]
fn pty_huck_starts_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
    expect_eof(&mut session);
}
```

- [ ] **Step 3: Run the smoke test**

Run: `cargo test --test pty_interactive`
Expected: `pty_huck_starts_and_exits` passes (it spawns huck in a PTY, sees the prompt, sends `exit`, sees the session end). If it fails, the harness or `expectrl` wiring is wrong — fix before proceeding. If it *skips* (logs "no PTY available"), the environment lacks PTY support; note it but the harness code is still considered done.

- [ ] **Step 4: Run the full suite**

Run: `cargo test`
Expected: all tests pass (566 baseline + 1 new = 567).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock tests/pty_interactive.rs
git commit -m "v15 task 1: expectrl dev-dep, PTY harness, smoke test"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/pty-test-harness`
- Baseline: 566 tests passing
- `expectrl` is a Rust `expect`/`pexpect` library. Its `Session` spawns a child in a PTY; `send` writes to the PTY master; `expect` reads until a pattern appears or a timeout fires.
- The `expectrl` API in the helper code is best-effort. Verify against the pinned version's docs. Likely points of difference: `Session::spawn` may instead be `expectrl::session::Session::spawn` or require a different builder; `expect` may need `expectrl::Eof` imported from a submodule; `send` may want `&[u8]`. Adapt the four helpers; keep their signatures.
- The huck prompt is the literal string `huck> ` (with a trailing space).

## Self-Review

- Does `cargo test --test pty_interactive` pass (or skip cleanly)?
- Are all `expectrl` calls confined to `try_spawn`/`send`/`expect`/`expect_eof`?
- Does the full suite still pass?

## Report Format

Status, the `expectrl` version pinned, any API adaptations made to the helpers, test count, commit SHA, any concerns.

---

## Task 2: Tab completion tests

Add five golden-path tests for tab completion. Each spawns its own huck in a PTY.

**Files:**
- Modify: `tests/pty_interactive.rs`

- [ ] **Step 1: Add the tab-completion tests**

Append to `tests/pty_interactive.rs`:

```rust
/// Builds an env with an isolated HISTFILE plus an empty PATH
/// directory, so command completion sees only builtins (deterministic).
/// (`env_refs` is defined in Task 1; reuse it.)
fn isolated_env(dir: &Path) -> Vec<(&'static str, String)> {
    let hist = dir.join("huck_history");
    let empty_path = dir.join("emptybin");
    std::fs::create_dir_all(&empty_path).unwrap();
    vec![
        ("HISTFILE", hist.to_string_lossy().into_owned()),
        ("PATH", empty_path.to_string_lossy().into_owned()),
    ]
}

#[test]
fn tab_completes_builtin() {
    let dir = tempfile::tempdir().unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // PATH is empty, so `ec` can only complete to the builtin `echo`.
    send(&mut session, "ec");
    send(&mut session, TAB);
    // The completed text `echo` appears in the redrawn line only if
    // completion fired.
    expect(&mut session, "echo");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_double_tab_lists() {
    let dir = tempfile::tempdir().unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Double-Tab at an empty prompt lists all commands; with an empty
    // PATH that is exactly the builtins.
    send(&mut session, TAB);
    send(&mut session, TAB);
    expect(&mut session, "echo");
    expect(&mut session, "history");
    send(&mut session, CTRL_C);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_filename() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ptyfile_unique.txt"), b"").unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo ptyfile_un");
    send(&mut session, TAB);
    // The completed filename appears only if file completion fired.
    expect(&mut session, "ptyfile_unique.txt");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_directory_slash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("ptydir_unique")).unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo ptydir_un");
    send(&mut session, TAB);
    // A completed directory is shown with a trailing slash.
    expect(&mut session, "ptydir_unique/");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_variable() {
    let dir = tempfile::tempdir().unwrap();
    let hist = dir.path().join("huck_history");
    let env: Vec<(&str, &str)> = vec![
        ("HISTFILE", hist.to_str().unwrap()),
        ("HUCKPTYVAR", "ptyvarvalue"),
    ];
    let Some(mut session) = try_spawn(dir.path(), &env) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo $HUCKPTY");
    send(&mut session, TAB);
    send(&mut session, ENTER);
    // If `$HUCKPTY` completed to `$HUCKPTYVAR`, the command printed the
    // value `ptyvarvalue` — a string never typed, so it can only be
    // command output.
    expect(&mut session, "ptyvarvalue");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 2: Run the tab-completion tests**

Run: `cargo test --test pty_interactive`
Expected: all 6 tests pass (1 smoke + 5 tab). If a test times out, inspect the panic message (it includes the awaited needle) and confirm completion behavior manually in a real terminal.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (567 + 5 = 572).

- [ ] **Step 4: Commit**

```bash
git add tests/pty_interactive.rs
git commit -m "v15 task 2: PTY tab-completion tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/pty-test-harness`
- Baseline: 567 tests passing
- An empty `PATH` directory makes command completion deterministic — only the 13 builtins are candidates, so `ec` unambiguously completes to `echo`.
- The completed text (`echo`, `ptyfile_unique.txt`, `ptydir_unique/`) appears in the PTY stream only if completion actually fired — that is the proof. For the variable test, running the command and expecting the *value* `ptyvarvalue` is an even stronger proof (the value is never typed).

## Self-Review

- Do all 6 `pty_interactive` tests pass (or skip cleanly)?
- Is each test's assertion a string that only appears if the feature worked?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 3: History recall tests

Add three golden-path tests for arrow-key history recall.

**Files:**
- Modify: `tests/pty_interactive.rs`

- [ ] **Step 1: Add the history-recall tests**

Append to `tests/pty_interactive.rs`:

```rust
#[test]
fn up_arrow_recalls_previous() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo recallmarker");
    send(&mut session, ENTER);
    expect(&mut session, "recallmarker"); // sync past the command
    expect(&mut session, "huck> ");       // sync to the next prompt
    send(&mut session, UP);
    // If up-arrow recalled the entry, the line is redrawn as the full
    // previous command.
    expect(&mut session, "echo recallmarker");
    send(&mut session, ENTER);
    expect(&mut session, "recallmarker"); // it ran again
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn up_arrow_twice_recalls_older() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo olderone");
    send(&mut session, ENTER);
    expect(&mut session, "olderone");
    expect(&mut session, "huck> ");
    send(&mut session, "echo newertwo");
    send(&mut session, ENTER);
    expect(&mut session, "newertwo");
    expect(&mut session, "huck> ");
    // Two ups should land on the older command.
    send(&mut session, UP);
    send(&mut session, UP);
    expect(&mut session, "echo olderone");
    send(&mut session, ENTER);
    expect(&mut session, "olderone");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn down_arrow_navigates_forward() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo firstcmd");
    send(&mut session, ENTER);
    expect(&mut session, "firstcmd");
    expect(&mut session, "huck> ");
    send(&mut session, "echo secondcmd");
    send(&mut session, ENTER);
    expect(&mut session, "secondcmd");
    expect(&mut session, "huck> ");
    // Up, up lands on `echo firstcmd`; down moves forward to
    // `echo secondcmd`.
    send(&mut session, UP);
    send(&mut session, UP);
    expect(&mut session, "echo firstcmd");
    send(&mut session, DOWN);
    expect(&mut session, "echo secondcmd");
    send(&mut session, ENTER);
    expect(&mut session, "secondcmd");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 2: Run the history tests**

Run: `cargo test --test pty_interactive`
Expected: all 9 tests pass (1 smoke + 5 tab + 3 history).

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (572 + 3 = 575).

- [ ] **Step 4: Commit**

```bash
git add tests/pty_interactive.rs
git commit -m "v15 task 3: PTY history-recall tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/pty-test-harness`
- Baseline: 572 tests passing
- The recall proof: after syncing past the command output *and* the following prompt, an up-arrow that recalls the entry causes rustyline to redraw the line. `expect("echo recallmarker")` then matches *new* stream bytes — if recall did not fire, the line stays empty and the `expect` times out.
- Markers are unique per test (`recallmarker`, `olderone`/`newertwo`, `firstcmd`/`secondcmd`) so matches cannot collide across the session.

## Self-Review

- Do all 9 `pty_interactive` tests pass (or skip cleanly)?
- Does each history test sync past BOTH the command output and the following prompt before sending an arrow key?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 4: Ctrl-C handling tests

Add four golden-path tests for signal/EOF handling.

**Files:**
- Modify: `tests/pty_interactive.rs`

- [ ] **Step 1: Add the Ctrl-C / Ctrl-D tests**

Append to `tests/pty_interactive.rs`:

```rust
#[test]
fn ctrl_c_empty_prompt_survives() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, CTRL_C);
    // The shell must still be alive: a command sent afterwards runs.
    send(&mut session, "echo aftersigint");
    send(&mut session, ENTER);
    expect(&mut session, "aftersigint");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn ctrl_c_clears_partial_line() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Type a partial line with NO Enter, then Ctrl-C.
    send(&mut session, "echo partialXYZ");
    send(&mut session, CTRL_C);
    // Run `pwd`. If Ctrl-C cleared the partial line, `pwd` runs alone
    // and prints the cwd. If it did NOT clear, the line would be
    // `echo partialXYZpwd` and the cwd path would never be printed.
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    // The temp dir's unique random component appears only if `pwd`
    // ran clean — it is never part of the typed input.
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn ctrl_c_breaks_out_of_wait() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Background a long sleep so `wait` blocks.
    send(&mut session, "sleep 30 &");
    send(&mut session, ENTER);
    expect(&mut session, "[1]"); // background job notification
    expect(&mut session, "huck> ");
    send(&mut session, "wait");
    send(&mut session, ENTER);
    // Ctrl-C must break the blocking `wait` and return to the prompt.
    send(&mut session, CTRL_C);
    send(&mut session, "echo afterwait");
    send(&mut session, ENTER);
    expect(&mut session, "afterwait");
    send(&mut session, "exit");
    send(&mut session, ENTER);
    // The orphaned `sleep 30` is reparented to init and exits on its
    // own — harmless.
}

#[test]
fn ctrl_d_empty_prompt_exits() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Ctrl-D (EOF) at an empty prompt exits the shell.
    send(&mut session, CTRL_D);
    expect_eof(&mut session);
}
```

- [ ] **Step 2: Run the Ctrl-C tests**

Run: `cargo test --test pty_interactive`
Expected: all 13 tests pass (1 smoke + 5 tab + 3 history + 4 Ctrl-C).

Note: `expect("[1]")` matches the literal string `[1]`. If the helper's `expect` treats its argument as a regex in the pinned `expectrl` version, escape the brackets or switch to a literal-match call — adjust the `expect` helper in Task 1 if so, not this test.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (575 + 4 = 579).

- [ ] **Step 4: Commit**

```bash
git add tests/pty_interactive.rs
git commit -m "v15 task 4: PTY Ctrl-C and Ctrl-D tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/pty-test-harness`
- Baseline: 575 tests passing
- `ctrl_c_breaks_out_of_wait` reproduces the v6 regression: if Ctrl-C does NOT break `wait`, the shell stays blocked, `echo afterwait` is never processed, and the `expect("afterwait")` times out — a real failure.
- `ctrl_c_clears_partial_line` uses `pwd` plus the temp dir's random basename as the proof: that string can only appear if the partial line was cleared and `pwd` ran on its own.

## Self-Review

- Do all 13 `pty_interactive` tests pass (or skip cleanly)?
- Does `ctrl_c_breaks_out_of_wait` genuinely block on `wait` before the Ctrl-C?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 5: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v15 row to the status table**

Append after the v14 row:

```
| v15       | PTY-based interactive test harness                      |
```

Match the table's column alignment.

- [ ] **Step 2: Note the PTY suite in the development/testing text**

Find the section of `README.md` that describes testing (near the "Tests live alongside each module" line, or the development-workflow section). Add a sentence:

```markdown
Interactive features (tab completion, history recall, Ctrl-C) are
covered by a PTY-driven golden-path suite in `tests/pty_interactive.rs`
using the `expectrl` crate; it skips gracefully where no PTY is
available.
```

- [ ] **Step 3: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts across all test binaries. Update the `cargo test               # full test suite (NNN tests)` line. Expected total is 566 baseline + 13 new = 579 — use the actual number.

- [ ] **Step 4: Update the Dependencies section if present**

If `README.md` has a Dependencies section listing crates, add `expectrl` as a dev-dependency note (e.g. "`expectrl` — PTY-driven interactive tests (dev-only)").

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v15 task 5: README — add v15 row and PTY-suite note"
```

---

## Final review checkpoint

After Task 5:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo test --test pty_interactive` — the 13 PTY tests pass (or skip cleanly with a logged notice)
- [ ] `cargo clippy --tests -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Run the PTY suite a few times in a row to confirm it is not flaky: `for i in 1 2 3; do cargo test --test pty_interactive || break; done`
- [ ] Final review the whole branch as a single diff before merging to main
