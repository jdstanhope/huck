# v25: Pipelines as Subshells — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow any `Command` as a pipeline stage with each stage running in its own forked subshell. Fixes M-10 (functions in pipelines), the v17/v18-era compound-in-pipeline limitation, and I-04 (builtins-in-pipelines affect parent — deliberate semantics shift to bash-correct subshell isolation).

**Architecture:** `Pipeline.commands` widens from `Vec<SimpleCommand>` to `Vec<Command>`. A new `fork_and_run_in_subshell` helper (libc::fork direct) handles non-external stages: child runs the body via the existing `execute` machinery against forked shell state, then `_exit`s with the resulting status. External stages keep using `std::process::Command` (hybrid path A from the spec). `run_multi_stage` is rewritten around raw pipe fds for uniform plumbing across both paths.

**Tech Stack:** Rust 1.95, existing huck modules (`src/command.rs`, `src/executor.rs`), `libc` (already a dependency — used for `waitpid`, `setpgid`, `dup2`, etc. throughout the codebase). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-25-huck-pipeline-subshells-design.md`.

**Branch:** `v25-pipeline-subshells` (off `main` at commit `acd50e6`).

**Baseline:** 988 tests pass, 0 clippy warnings.

---

## File structure

- `src/command.rs` — `Pipeline.commands` widens to `Vec<Command>`; parser allows any `Command` after `|`; rejects nested multi-stage `Command::Pipeline`-as-stage.
- `src/executor.rs` — new `fork_and_run_in_subshell` helper using libc::fork; new `classify_stage` + `spawn_external_with_fds`; `run_multi_stage` rewritten around raw pipe fds.
- `tests/pipeline_subshell_integration.rs` (new) — end-to-end coverage.
- `tests/pty_interactive.rs` — 1 new PTY test (Ctrl-Z on a compound-stage pipeline).
- `docs/bash-divergences.md` — M-10 → fixed; I-04 → fixed (no longer divergent); change-log entry.
- `README.md` — v25 status row.

---

## Task 1: AST refactor — `Pipeline.commands: Vec<Command>`

Pure AST refactor. Zero observable behavior change. Parser still emits only `Command::Pipeline { commands: vec![Command::Pipeline(...)] }`-style nesting where each stage's inner Command wraps the prior single-element shape, OR add a `Command::Simple(SimpleCommand)` variant for cleaner stage representation. Implementer chooses.

**Implementer decision point**: two shapes work:
- **Shape A (recommended)**: Add `Command::Simple(SimpleCommand)` variant to `Command`. Pipeline stages become `Command::Simple(...)` for simple commands, `Command::If(...)` for compounds, etc. Clean, no nested-Pipeline-as-stage shenanigans.
- **Shape B**: Keep `Command` unchanged. Pipeline stages can be any `Command`. A simple-command stage is `Command::Pipeline(Pipeline { commands: vec![Command::Pipeline(...)]})` — circular, awkward, but no new variant.

Shape A is cleaner. Recommend it. The cost is updating every `match cmd { Command::Pipeline(...) => ... }` site to add a `Command::Simple(...)` arm.

**Files:**
- Modify: `src/command.rs` (Pipeline struct + Command enum + parser shape)
- Modify: `src/executor.rs` (every `Command::Pipeline` match arm)
- Modify: any test that destructures Pipeline or builds it directly

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `988 0` and `0`.

- [ ] **Step 2: Widen `Pipeline.commands` and (Shape A) add `Command::Simple`**

In `src/command.rs`:

```rust
pub enum Command {
    Pipeline(Pipeline),
    Simple(SimpleCommand),    // NEW (Shape A)
    If(IfClause),
    While(WhileClause),
    For(ForClause),
    Case(CaseClause),
    BraceGroup(BraceGroup),
    FunctionDef { name: String, body: Box<Command> },
}

pub struct Pipeline {
    // BREAKING CHANGE (v25): was Vec<SimpleCommand>; now Vec<Command>.
    // The parser rejects Command::Pipeline as a stage (nested multi-stage
    // pipelines aren't a POSIX construct at this level).
    pub commands: Vec<Command>,
}
```

- [ ] **Step 3: Update the parser to emit `Command::Simple` for single-stage pipelines AND for each pipeline stage**

Find `parse_pipeline_with_first` (or wherever single-element `Pipeline { commands: vec![simple] }` is built). Replace:

```rust
Pipeline { commands: vec![simple] }
```

with:

```rust
Pipeline { commands: vec![Command::Simple(simple)] }
```

Similarly for the multi-stage case where each subsequent stage was parsed as `SimpleCommand`, wrap in `Command::Simple`.

Compound commands (`if`/`while`/etc.) are NOT wrapped in `Command::Simple` — they keep their own variants. The parser-level dispatcher that wraps simple commands into pipelines should ONLY do so for simple commands. (Compound commands are top-level Commands, not pipeline stages — yet. Task 2 enables compound stages.)

- [ ] **Step 4: Walk the compile-error fanout**

```bash
cargo build 2>&1 | grep "^error\[" | head -50
```

Fix each error. The main sites:
- Executor's `run_pipeline` / `run_multi_stage`: pattern-matches `Pipeline.commands[i]` as `SimpleCommand`. Switch to matching on `Command`. For Task 1 only, handle `Command::Simple(s)` exactly as the old code handled `SimpleCommand`. Add `_ => unreachable!("Task 2 will enable non-Simple stages; Task 1 doesn't")` arms for compound variants.
- Tests building Pipelines directly: wrap each SimpleCommand in `Command::Simple(...)`.

- [ ] **Step 5: Verify zero behavior change**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: clean, 988 0, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(ast): Pipeline.commands widens to Vec<Command>; add Command::Simple

No behavior change. Parser still produces only Command::Simple stages
wrapping SimpleCommand. Sets up the AST shape for Task 2 (parser
accepts compound stages) and Task 5 (executor dispatches per-stage via
fork-subshell for non-external stages)."
```

---

## Task 2: Parser — allow any Command as a pipeline stage

After this task, `echo hi | if true; then cat; fi`, `echo hi | myfunc`, `echo hi | { cat; }`, etc., all PARSE successfully (execution is Task 5).

**Files:** `src/command.rs`.

- [ ] **Step 1: Failing parser tests**

In `src/command.rs::tests`, append:

```rust
#[test]
fn parse_pipeline_with_if_stage() {
    let tokens = tokenize("echo hi | if true; then cat; fi").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    assert!(matches!(p.commands[0], Command::Simple(_)));
    assert!(matches!(p.commands[1], Command::If(_)));
}

#[test]
fn parse_pipeline_with_function_call_stage() {
    // Function call appears as Simple at parse time — resolution is at runtime.
    let tokens = tokenize("echo hi | myfunc").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    assert!(matches!(&p.commands[1], Command::Simple(_)));
}

#[test]
fn parse_pipeline_with_brace_group_stage() {
    let tokens = tokenize("echo hi | { cat; }").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    assert!(matches!(p.commands[1], Command::BraceGroup(_)));
}

#[test]
fn parse_pipeline_with_while_stage() {
    let tokens = tokenize("seq 1 3 | while true; do cat; done").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert!(matches!(p.commands[1], Command::While(_)));
}

#[test]
fn parse_pipeline_with_for_stage() {
    let tokens = tokenize("echo hi | for x in a b; do echo $x; done").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert!(matches!(p.commands[1], Command::For(_)));
}

#[test]
fn parse_pipeline_with_case_stage() {
    let tokens = tokenize("echo a | case foo in a) :; ;; esac").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert!(matches!(p.commands[1], Command::Case(_)));
}
```

Run: `cargo test --bin huck parse_pipeline_with` — expect failures (current parser only accepts simple-command stages after `|`).

- [ ] **Step 2: Update the parser to dispatch on the next-stage's first token**

Find the multi-stage parsing loop (around `parse_pipeline_with_first` or `parse_pipeline`). After consuming `|`, instead of calling `finalize_stage` (which produces a `SimpleCommand`), dispatch on the next token:

- If the first token is a keyword (`if`, `while`, `until`, `for`, `case`, `{`) or a `function name() …` pattern: parse the compound via the existing `parse_command` (or `parse_if`/`parse_while`/etc.) and wrap as `Command::If(...)`, etc.
- Otherwise: parse as a simple command (existing `finalize_stage` path), wrap as `Command::Simple(...)`.

For the FIRST stage (before any `|`), the existing top-level dispatcher already calls `parse_command` which returns the right `Command` variant. Just ensure that result is treated as the pipeline's first stage (wrapped in `Command::Simple` if it's actually a `SimpleCommand`).

Concretely, the dispatch may already exist for compound commands at the sequence level — just lift the same dispatch into the post-`|` stage parser. Avoid code duplication.

- [ ] **Step 3: Reject nested multi-stage pipeline as a stage**

If after `|` the next dispatch produces `Command::Pipeline(p)` with `p.commands.len() > 1`, return a parse error (`ParseError::NestedPipelineStage` or similar). Add a test:

```rust
#[test]
fn parse_pipeline_rejects_nested_multi_stage() {
    // (subshell syntax `(a | b)` isn't lexed yet — M-11 — so this is
    // moot for now, but the guard exists for future correctness.)
    // For v25, this case can't actually be triggered without subshell
    // syntax; the test can be marked #[ignore] with a TODO.
}
```

(If the case is unreachable today, document it in a code comment and skip the test.)

- [ ] **Step 4: Verify**

```bash
cargo test --bin huck parse_pipeline_with 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 5-6 new parser tests pass; full suite 988 + ~6 = ~994, 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "parse: allow any Command type as a pipeline stage

After `|` (or as the first stage), dispatch on the next token: keywords
introduce compound stages (Command::If/While/For/Case/BraceGroup);
otherwise the stage is parsed as Command::Simple. Execution is Task 5;
this commit only widens what parses successfully."
```

---

## Task 3: Fork infrastructure — `fork_and_run_in_subshell` helper

Adds the libc::fork-based subshell helper. After this task, the helper exists, has its own integration test, but is not yet wired into the pipeline executor.

**Files:** `src/executor.rs` + a new integration test file `tests/pipeline_subshell_fork_integration.rs` (temporary harness; deleted in Task 6 once the main integration suite covers everything).

- [ ] **Step 1: Add the helper signature and a smoke test**

In `src/executor.rs`, add the function signature with a `todo!()` body:

```rust
use std::os::unix::io::RawFd;

/// Forks a subshell and runs `cmd` in the child with the supplied stdio
/// fds dup2'd to 0/1/2. After the body runs, the child `_exit`s with the
/// resulting status. Returns the child pid in the parent.
///
/// `parent_fds_to_close` lists pipe fds the parent holds that this child
/// must close (else EOF propagation fails downstream).
///
/// `pgid_target`: 0 = become own pgrp leader; >0 = join this pgrp.
pub fn fork_and_run_in_subshell(
    cmd: &Command,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    todo!("v25 Task 3")
}
```

Add a smoke test in `tests/pipeline_subshell_fork_integration.rs`:

```rust
//! Integration test for v25 Task 3's fork helper. Verifies the fork +
//! child-runs-builtin + parent-reads-output pipeline using libc::pipe.
//! This file goes away in Task 6 once the main integration suite covers
//! the same ground.

#[test]
fn fork_runs_builtin_and_parent_reads_output() {
    // The test runs `huck -c 'echo hi | cat'` (once pipelines work) and
    // verifies the output is "hi". Until Task 5 wires the pipeline, this
    // test is #[ignore]'d.
}
```

(The smoke test can be `#[ignore]` until Task 5 if needed; the value of Task 3 is the helper itself + a unit-style test of fork behavior.)

For a true unit test of the fork helper, build a small artificial Command (a no-op like `Command::Simple(echo_command)`) and call `fork_and_run_in_subshell` with `stdin/stdout/stderr` pointing at a pipe pair, then in the parent read the pipe and waitpid the child. This is gnarly to express as a unit test; the implementer should aim for ONE such test that proves the helper basically works.

- [ ] **Step 2: Implement `fork_and_run_in_subshell`**

```rust
pub fn fork_and_run_in_subshell(
    cmd: &Command,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // CHILD: async-signal-safe-ish operations only until we dive into
        // `execute`. huck is single-threaded so this is fine.
        unsafe {
            // 1. Reset job-control signals.
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::signal(libc::SIGTTIN, libc::SIG_DFL);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            // 2. Join the pgrp.
            libc::setpgid(0, pgid_target);
            // 3. dup2 the stdio fds.
            if stdin_fd != 0 { libc::dup2(stdin_fd, 0); }
            if stdout_fd != 1 { libc::dup2(stdout_fd, 1); }
            if stderr_fd != 2 { libc::dup2(stderr_fd, 2); }
            // 4. Close the originals (if not already 0/1/2).
            for fd in [stdin_fd, stdout_fd, stderr_fd] {
                if fd > 2 { libc::close(fd); }
            }
            // 5. Close every other pipe fd the parent held.
            for &fd in parent_fds_to_close {
                if fd != stdin_fd && fd != stdout_fd && fd != stderr_fd {
                    libc::close(fd);
                }
            }
        }
        // 6. Run the body. `execute_command` is the existing dispatcher
        //    that takes a &Command and returns ExecOutcome.
        let mut sink = StdoutSink::Terminal;
        let outcome = run_command(cmd, shell, &mut sink);
        // 7. Translate outcome to i32, mask to 8 bits.
        let status: i32 = match outcome {
            ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
            ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
            ExecOutcome::FunctionReturn(n) => n,
        };
        let status = status.rem_euclid(256);
        unsafe { libc::_exit(status) };
    }
    // PARENT.
    unsafe {
        // Defensive setpgid (race with child's setpgid).
        libc::setpgid(pid, pgid_target);
    }
    Ok(pid)
}
```

Notes:
- `run_command` is the existing dispatcher that takes a `&Command` and returns `ExecOutcome`. If the function exists under a different name (`execute_command`, `dispatch_command`, etc.), use that. Read the existing code to find it.
- The child must NOT call any path that writes to history or attempts cleanup that the parent would do (e.g., `history.save()`). The `_exit` bypasses Drop, so the parent's history isn't touched.
- If `run_command` doesn't exist (maybe `execute` is the dispatcher), call that — but `execute` takes a `Sequence`, not a `Command`. The child needs to run a single Command, so the helper may need to wrap: `let seq = Sequence { first: cmd.clone(), rest: vec![], background: false }; execute(&seq, shell, …)`. Pick the most direct path; clone the Command if needed (it's a per-fork operation, not a hot path).

- [ ] **Step 3: Add the unit test that exercises the helper**

In `src/executor.rs::tests`, write a test that:
1. Creates a `libc::pipe` pair.
2. Builds a small `Command::Simple(echo_command)`.
3. Calls `fork_and_run_in_subshell` with stdout = pipe.write.
4. In the parent, reads from pipe.read into a buffer.
5. `libc::waitpid`s the child.
6. Asserts the buffer contains the expected output.

This is the canonical "the helper works" test. Mark with a comment explaining the pattern so future helpers can reuse it.

- [ ] **Step 4: Verify**

```bash
cargo test --bin huck fork_ 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: fork helper test passes; full suite ~995, 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: fork_and_run_in_subshell helper (libc::fork direct)

Child: reset job signals, setpgid, dup2 the stdio fds, close inherited
pipe fds, run the Command body via the existing dispatcher, _exit with
status (8-bit masked). Parent: defensive setpgid, return the child pid
for the caller's wait loop. Unit test forks an echo-stage and reads its
output through a pipe, confirming basic correctness."
```

---

## Task 4: Stage classification + `spawn_external_with_fds`

Adds the `classify_stage` decision and adapts the external-command spawn path to take raw fds (so it composes with the new pipe plumbing in Task 5).

**Files:** `src/executor.rs`.

- [ ] **Step 1: Add `classify_stage`**

```rust
enum StageKind<'a> {
    External(&'a SimpleCommand),
    InProcess(&'a Command),
}

fn classify_stage<'a>(cmd: &'a Command, shell: &Shell) -> StageKind<'a> {
    if let Command::Simple(simple) = cmd
        && let SimpleCommand::Exec(exec) = simple
        && let Some(prog) = exec.program_static_text()
        && !shell.functions.contains_key(&prog)
        && !crate::builtins::is_builtin(&prog)
    {
        return StageKind::External(simple);
    }
    StageKind::InProcess(cmd)
}
```

Where `program_static_text` is a helper on `ExecCommand` that returns `Some(String)` if the program word is a single unquoted Literal part (best-effort static resolution; dynamic words like `$cmd` return `None`).

If `program_static_text` doesn't exist, add it:

```rust
impl ExecCommand {
    pub fn program_static_text(&self) -> Option<String> {
        if self.program.0.len() == 1 {
            if let WordPart::Literal { text, .. } = &self.program.0[0] {
                return Some(text.clone());
            }
        }
        None
    }
}
```

Unit-test `classify_stage` for the major cases:
- `Command::Simple(SimpleCommand::Exec(echo))` where echo is an external → External.
- `Command::Simple(SimpleCommand::Exec(cd))` (cd is a builtin) → InProcess.
- `Command::Simple(SimpleCommand::Exec(myfunc))` where myfunc is in shell.functions → InProcess.
- `Command::If(...)` → InProcess.
- `Command::Simple(SimpleCommand::Exec($dyn))` (dynamic program) → InProcess (best-effort: fork the subshell, which will resolve at exec time).

- [ ] **Step 2: Add `spawn_external_with_fds`**

The existing external-command spawn in `run_multi_stage` builds `std::process::Command`, sets env/args, configures stdio via `Stdio::piped()`/`Stdio::from(File)`, and spawns. For Task 5, we need to call this with raw fds (the pipe ends from `libc::pipe`).

Wrap the existing logic:

```rust
fn spawn_external_with_fds(
    cmd: &SimpleCommand,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    // Build std::process::Command as today; for stdio, convert raw fds
    // to Stdio via Stdio::from(unsafe { OwnedFd::from_raw_fd(fd) }).
    // pre_exec: close parent_fds_to_close and setpgid.
    ...
}
```

Notes:
- `Stdio::from_raw_fd` consumes the fd (transfers ownership to the Stdio). So the caller must NOT close the fd after passing it; std::process::Command handles closing in the parent after spawn.
- Use `process.pre_exec(|| { ... })` (already used in huck for `reset_job_control_signals_in_child`) to close `parent_fds_to_close` in the child. setpgid is already handled by `process.process_group(pgid_target)`.
- Return the child's pid via `child.id() as i32`.
- The Child handle should be `mem::forget`'d (matching the existing pattern from B-09's `forget_process_children`) since we'll waitpid manually.

- [ ] **Step 3: Verify**

```bash
cargo test --bin huck classify_stage spawn_external 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: classify_stage tests pass; full suite ~1000, 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "exec: classify_stage + spawn_external_with_fds helpers

classify_stage: best-effort static resolution to decide External (single
SimpleCommand::Exec resolving to an external) vs InProcess (everything
else — builtins, functions, compounds, function-defs, dynamic-program
simples).

spawn_external_with_fds: wraps the existing std::process::Command path
with raw-fd stdio so it composes with the libc::pipe plumbing in Task 5.
pre_exec closes the parent's pipe fds in the child."
```

---

## Task 5: Rewrite `run_multi_stage` around raw pipe fds + fork dispatch

The big integration step. After this task, all the pipeline shapes from the spec work end-to-end.

**Files:** `src/executor.rs::run_multi_stage`.

- [ ] **Step 1: Failing integration tests in a new test file**

Create `tests/pipeline_subshell_integration.rs`:

```rust
//! End-to-end tests for v25 pipelines-as-subshells.

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
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn pipeline_function_call_as_stage() {
    // Smallest "function in pipeline" test.
    let (out, _) = run("myfunc() { sed s/h/H/; }\necho hello | myfunc\nexit\n");
    assert!(out.contains("Hello"), "got: {out}");
}

#[test]
fn pipeline_if_clause_as_stage() {
    let (out, _) = run("echo hi | if true; then cat; fi\nexit\n");
    assert!(out.contains("hi"), "got: {out}");
}

#[test]
fn pipeline_brace_group_as_stage() {
    let (out, _) = run("echo hi | { cat; }\nexit\n");
    assert!(out.contains("hi"), "got: {out}");
}
```

Run: `cargo test --test pipeline_subshell_integration` — expect failures (executor still unreachables on non-Simple stages).

- [ ] **Step 2: Rewrite the loop**

Replace `run_multi_stage`'s current per-stage spawn machinery with the raw-fd version sketched in the spec's "Per-stage execution" section. Pseudo-structure:

```rust
fn run_multi_stage(commands: &[Command], shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let interactive = matches!(sink, StdoutSink::Terminal);
    let n = commands.len();
    let mut first_pid: Option<i32> = None;
    let mut stage_pids: Vec<i32> = Vec::new();

    let mut prev_pipe_read: Option<RawFd> = None;
    let mut parent_holds: Vec<RawFd> = Vec::new();

    for (i, stage_cmd) in commands.iter().enumerate() {
        let is_last = i == n - 1;

        // 1. Apply inline assignments in parent.
        let snap = apply_inline_assignments_for_stage(stage_cmd, shell);

        // 2. Resolve redirects (file opens).
        let files = open_stage_redirects(stage_cmd, shell)?;

        // 3. Build stdin/stdout/stderr fds.
        let stdin_fd = files.stdin_fd.unwrap_or(prev_pipe_read.take().unwrap_or(libc::STDIN_FILENO));
        let stdout_fd = if let Some(fd) = files.stdout_fd {
            fd
        } else if !is_last {
            let (r, w) = pipe_pair()?;
            prev_pipe_read = Some(r);
            parent_holds.push(r);
            parent_holds.push(w);
            w
        } else if let StdoutSink::Capture(_) = sink {
            // Capture last stage's stdout via a pipe.
            let (r, w) = pipe_pair()?;
            ...
            w
        } else {
            libc::STDOUT_FILENO
        };
        let stderr_fd = files.stderr_fd.unwrap_or(libc::STDERR_FILENO);

        // 4. Spawn.
        let pgid_target = first_pid.unwrap_or(0);
        let pid = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => {
                spawn_external_with_fds(simple, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &parent_holds)?
            }
            StageKind::InProcess(cmd) => {
                fork_and_run_in_subshell(cmd, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &parent_holds)?
            }
        };
        if first_pid.is_none() { first_pid = Some(pid); }
        stage_pids.push(pid);

        // 5. Restore inline assignments.
        restore_inline_assignments(snap, shell);

        // 6. Close in parent: the pipe write end this stage took
        //    (downstream pipe ends are closed when the next stage takes them).
        close_in_parent(&mut parent_holds, stdout_fd);
    }

    // 7. Close any remaining parent_holds (the final pipe read if last stage was capture).
    drain_parent_holds(&mut parent_holds);

    // 8. Wait via the existing B-09 helper.
    if interactive {
        // ... give terminal to pgid, wait, restore terminal ...
    } else {
        // Non-interactive wait — same as today.
    }
}
```

The detailed bookkeeping for `parent_holds` (which fds are closed when) is the hardest part — get it wrong and you get hangs (EOF doesn't propagate) or zombies (waitpid blocks). Use an `OwnedFd`-style RAII wrapper if helpful.

- [ ] **Step 3: Handle the heredoc-into-first-stage case**

v24's heredoc body bytes are written to the child's stdin via the `pending_input` plumbing. Post-rewrite, that becomes: write the bytes to the pipe end the child reads from, BEFORE the child has a chance to fill the pipe and block. The simplest approach: write the bytes from the parent immediately after forking the child (parent has the write end of the pipe-from-heredoc).

Or simpler: use the existing `pending_input` write-after-spawn pattern, just rewired through the raw fds.

The implementer should verify the existing v24 heredoc tests STILL PASS after the rewrite. They should, since the plumbing is logically equivalent.

- [ ] **Step 4: Verify the new tests + everything pre-v25**

```bash
cargo test --test pipeline_subshell_integration 2>&1 | tail -15
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: all 3 new integration tests pass, full suite 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: run_multi_stage rewritten around raw pipe fds + per-stage fork dispatch

Every multi-stage pipeline now uses libc::pipe pairs throughout; each
stage is classified (External via std::process::Command, or InProcess
via fork_and_run_in_subshell) and gets its own subshell. Builtins,
functions, and compound commands all work as pipeline stages.

Side effect: I-04 divergence fixed — `cd /tmp | pwd` no longer mutates
the parent shell's cwd; the cd runs in a forked subshell.

v23 inline-assignment scoping preserved (apply before fork, restore
after spawn). v24 heredoc plumbing preserved (body bytes flow through
the raw pipe to the child's stdin)."
```

---

## Task 6: Full integration test suite + doc updates

Cover every spec case end-to-end and update the audit doc.

**Files:**
- Extend: `tests/pipeline_subshell_integration.rs` with the full spec test table.
- Add: 1 PTY test in `tests/pty_interactive.rs` for stop/resume on a compound-stage pipeline.
- Modify: `docs/bash-divergences.md` — M-10 → fixed; I-04 → fixed (or move to a "fixed by later iteration" note); change-log entry.
- Modify: `README.md` — v25 status row.
- Delete: `tests/pipeline_subshell_fork_integration.rs` (the Task 3 temporary harness), if it's still present.

- [ ] **Step 1: Add the full spec test table**

In `tests/pipeline_subshell_integration.rs`, add all the remaining tests from the spec's Tests table:
- `pipeline_while_loop_as_stage`
- `pipeline_for_loop_as_stage`
- `pipeline_case_as_stage`
- `pipeline_function_def_as_stage_is_noop`
- `pipeline_builtin_side_effect_does_not_leak` (`cd /tmp | true; pwd` → original dir)
- `pipeline_var_assignment_does_not_leak`
- `pipeline_exit_in_first_stage_does_not_exit_shell`
- `pipeline_compound_with_redirect` (heredoc on compound stage)
- `pipeline_function_inherits_inline_assignment`
- `pipeline_three_stages_compound_middle`

For each, write the script and the expected output. Use external commands that exist on Linux (`cat`, `sed`, `seq`, `grep`, `printf`) and avoid huck-not-implemented builtins like `read`.

- [ ] **Step 2: Add the PTY stop/resume test**

In `tests/pty_interactive.rs`, append:

```rust
#[test]
fn pty_compound_stage_pipeline_stops_and_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Start a pipeline with a compound stage that sleeps.
    send(&mut session, "cat | if true; then sleep 5; fi");
    send(&mut session, ENTER);
    settle();
    // Ctrl-Z stops both stages.
    send(&mut session, "\x1a");   // SIGTSTP / Ctrl-Z
    expect(&mut session, "Stopped");
    expect(&mut session, "huck> ");
    // kill the job so the test doesn't hang.
    send(&mut session, "kill %1");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 3: Update `docs/bash-divergences.md`**

- M-10 (Functions as pipeline stages): change `[deferred]` to `[fixed (2026-05-25)]`. Description: "Pipeline stages of any Command type — including function calls — run in forked subshells per POSIX 2.12."
- I-04 (Pure-builtin `cmd &` runs synchronously in parent / builtins in pipelines affect parent): the BUILTINS-IN-PIPELINES half is now fixed. Split or update the entry — keep the `cmd &` part as still-intentional (separate issue) and add a note that the pipeline part is fixed as of v25.
- Add change-log entry: `**2026-05-25**: M-10 (functions in pipelines) + the compound-commands-as-pipeline-stages limitation shipped as v25. I-04 partially fixed (builtins in pipelines now correctly subshell-scoped).`

- [ ] **Step 4: Update `README.md`**

Add a v25 row to the status table:
```
| v25       | Pipelines as subshells (functions, compounds, builtins   |
|           | all run in forked subshells per POSIX)                   |
```

(Adjust column widths to match the existing table.)

- [ ] **Step 5: Remove the Task 3 temporary harness**

```bash
rm tests/pipeline_subshell_fork_integration.rs   # if it exists
```

(Skip if the file was never created or already deleted.)

- [ ] **Step 6: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: full suite 0 fails, 0 warnings. Test count ~1010-1015 (988 baseline + ~6 parser + ~5 unit + ~12 integration + 1 PTY = ~1012).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "v25: pipelines as subshells — integration tests + docs

Full test table covering all stage shapes (simple, compound, function,
function-def, mixed three-stage). PTY test verifies Ctrl-Z stop/resume
on a compound-stage pipeline still works via B-09's pgrp wait. Audit
doc M-10 marked fixed; I-04 updated to reflect the partial fix."
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the **final cross-cutting opus reviewer** over the whole `v25-pipeline-subshells` branch diff. After approval:

```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v25-pipeline-subshells
git -C /home/john/projects/shuck branch -d v25-pipeline-subshells
```

---

## Self-review checklist

1. **Spec coverage**: every section in the spec maps to a task.
   - AST → Task 1.
   - Parser → Task 2.
   - Fork infra → Task 3.
   - Per-stage execution + classification → Tasks 4 + 5.
   - Edge cases → tested across Task 5/6 integration tests.
   - History/job-control → covered by Task 6 PTY test + the unchanged B-09 path.

2. **Placeholders**: every step shows concrete code or a clear contract. The biggest implementer-judgment area is the raw-fd plumbing bookkeeping in Task 5 — flagged as "the hardest part" with a recommendation to use RAII fd wrappers.

3. **Type consistency**: `Pipeline.commands: Vec<Command>` flows from Task 1 through every later task. `Command::Simple(SimpleCommand)` (Shape A) is the recommended addition; alternative Shape B is documented in case the implementer prefers.

4. **Order dependencies**:
   - Task 1 (AST) must precede everything.
   - Task 2 (parser) sits on Task 1.
   - Task 3 (fork helper) is standalone; can run any time after Task 1 (it uses `Command` from the new AST shape).
   - Task 4 (classify + external-spawn-with-fds) depends on Tasks 1 + 3.
   - Task 5 (run_multi_stage rewrite) depends on Tasks 1-4.
   - Task 6 depends on Task 5.

5. **Backward-compat callouts**: the I-04 semantic shift is explicitly documented as deliberate (the user chose it). Existing v23 inline-assignment tests and v24 heredoc tests should ALL continue to pass after the rewrite (they exercise the same observable behavior; the implementation underneath changes but the contracts hold).
