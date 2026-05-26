# v28: Subshell Syntax `(list)` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement POSIX subshell syntax `(list)` — runs the inner Sequence in a forked subshell with isolated side effects. Closes M-11 from `docs/bash-divergences.md`.

**Architecture:** New `Command::Subshell { body: Box<Sequence> }` AST variant; parser dispatches `Token::Op(LParen)` at command-start to a new subshell-body parser; continuation classifier learns a `Subshell` reason for unterminated `(cmd`; executor's top-level `run_command` Subshell arm forks via the existing v25 `fork_and_run_in_subshell` helper, which grows a small child-side dispatch to run the inner `Sequence` directly (avoids recursive fork in pipeline-stage subshells).

**Tech Stack:** Rust 1.95; existing huck modules (`src/command.rs`, `src/continuation.rs`, `src/executor.rs`). No lexer changes — `(` and `)` already tokenize as `Operator::LParen`/`Operator::RParen` (v21).

**Spec:** `docs/superpowers/specs/2026-05-26-huck-subshell-syntax-design.md`.

**Branch:** `v28-subshell-syntax` (off `main` at commit `fae1e4d`).

**Baseline:** 1082 tests pass, 0 clippy warnings.

---

## File structure

- `src/command.rs` — new `Command::Subshell { body: Box<Sequence> }` variant; new `ParseError::EmptySubshell`/`ParseError::UnterminatedSubshell`; parser dispatch for `LParen` at command-start.
- `src/continuation.rs` — `ContinuationReason::Subshell`; classifier maps `ParseError::UnterminatedSubshell`; joiner = `"; "`.
- `src/executor.rs` — `run_command` arm for `Command::Subshell` (forks at top level via the v25 helper); `fork_and_run_in_subshell` gains a child-side match on `Command::Subshell` to execute the inner Sequence directly.
- `tests/subshell_integration.rs` (new) — end-to-end coverage.
- `tests/pty_interactive.rs` — 2 new PTY tests.
- `docs/bash-divergences.md` — M-11 fixed; change-log entry.
- `README.md` — v28 status row.

---

## Task 1: AST + parser

After this task, `(echo hi)` parses into `Command::Subshell { body: ... }`. Empty `()` errors. Unterminated `(cmd` errors. Subshells work as pipeline stages (any position).

**Files:**
- Modify: `src/command.rs` (Command enum + ParseError variants + parser dispatch).
- Executor: add `unreachable!` arm for `Command::Subshell` so the build passes; Task 3 wires execution.

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `1082 0` and `0`.

- [ ] **Step 2: Add `Command::Subshell` and error variants**

In `src/command.rs`:
```rust
pub enum Command {
    Pipeline(Pipeline),
    Simple(SimpleCommand),
    If(IfClause),
    While(WhileClause),
    For(ForClause),
    Case(CaseClause),
    BraceGroup(BraceGroup),
    Subshell { body: Box<Sequence> },    // NEW
    FunctionDef { name: String, body: Box<Command> },
}

pub enum ParseError {
    // existing...
    EmptySubshell,           // NEW: `()`
    UnterminatedSubshell,    // NEW: `(cmd` (no closing `)`)
}
```

If `ParseError` already has `Display` / a user-facing message in `src/shell.rs::parse_error_message`, add cases for the two new variants there too. Look for the existing pattern (similar handling for `UnterminatedIf`/`UnterminatedFunction` etc.) and follow it.

- [ ] **Step 3: Failing parser tests**

In `src/command.rs::tests`:

```rust
#[test]
fn parse_subshell_simple() {
    let tokens = tokenize("(echo hi)").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Subshell { body } = parsed.first else {
        panic!("expected Subshell, got {:?}", parsed.first)
    };
    // body is a Sequence with one command (the `echo hi` pipeline).
    assert_eq!(body.rest.len(), 0);
}

#[test]
fn parse_subshell_with_sequence() {
    let tokens = tokenize("(cmd1; cmd2)").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Subshell { body } = parsed.first else { panic!() };
    assert_eq!(body.rest.len(), 1);   // first + 1 more = 2 commands
}

#[test]
fn parse_subshell_with_and_or() {
    let tokens = tokenize("(true && echo hi)").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Subshell { body } = parsed.first else { panic!() };
    // Body's first command + the And-connected rest.
    assert!(body.rest.iter().any(|(conn, _)| matches!(conn, Connector::And)));
}

#[test]
fn parse_subshell_nested() {
    let tokens = tokenize("((echo hi))").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Subshell { body: outer } = parsed.first else { panic!() };
    let Command::Subshell { .. } = outer.first else {
        panic!("expected nested Subshell, got {:?}", outer.first)
    };
}

#[test]
fn parse_subshell_empty_errors() {
    let tokens = tokenize("()").unwrap();
    let err = parse(tokens).expect_err("expected ParseError::EmptySubshell");
    assert!(matches!(err, ParseError::EmptySubshell), "got {:?}", err);
}

#[test]
fn parse_subshell_unterminated_errors() {
    let tokens = tokenize("(echo hi").unwrap();
    let err = parse(tokens).expect_err("expected ParseError::UnterminatedSubshell");
    assert!(matches!(err, ParseError::UnterminatedSubshell), "got {:?}", err);
}

#[test]
fn parse_subshell_as_pipeline_first_stage() {
    let tokens = tokenize("(echo hi) | cat").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    assert!(matches!(p.commands[0], Command::Subshell { .. }));
}

#[test]
fn parse_subshell_as_pipeline_later_stage() {
    let tokens = tokenize("echo hi | (cat)").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    assert!(matches!(p.commands[1], Command::Subshell { .. }));
}

#[test]
fn parse_subshell_does_not_conflict_with_function_def() {
    // `f() (echo hi)` is a function definition whose body is a subshell.
    // The parser must dispatch on IDENT + `(` for function-def, not LParen-alone-at-start.
    let tokens = tokenize("f() (echo hi)").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::FunctionDef { name, body } = parsed.first else {
        panic!("expected FunctionDef, got {:?}", parsed.first)
    };
    assert_eq!(name, "f");
    assert!(matches!(*body, Command::Subshell { .. }),
        "function body should be a Subshell, got {:?}", body);
}
```

Run: `cargo test --bin huck parse_subshell` — expect failures.

- [ ] **Step 4: Implement the parser dispatch**

Find the existing top-level Command dispatcher in `src/command.rs` (probably `parse_command` — the function that decides whether the first token starts an `if`, `while`, `for`, `case`, `{`, function-def, or simple command). Add an arm for `Token::Op(Operator::LParen)` at the start (before the IDENT-then-LParen function-def case is checked):

```rust
Some(Token::Op(Operator::LParen)) => {
    iter.next();   // consume LParen
    // Parse the inner sequence; stop on RParen.
    let body = parse_sequence_until(iter, &[Token::Op(Operator::RParen)])?;
    // Expect closing RParen.
    match iter.next() {
        Some(Token::Op(Operator::RParen)) => {},
        _ => return Err(ParseError::UnterminatedSubshell),
    }
    if body.is_empty_subshell() {
        return Err(ParseError::EmptySubshell);
    }
    Ok(Command::Subshell { body: Box::new(body) })
}
```

`parse_sequence_until` may already exist with a stop-token parameter; if not, adapt or write a small wrapper. `is_empty_subshell` is just a check on the parsed Sequence (e.g., `seq.first` is some sentinel like a no-op SimpleCommand with empty program; or check explicitly during parsing — return EmptySubshell if we immediately see RParen after LParen without any inner content). Implementer's discretion on the exact shape.

**Function-def disambiguation**: the existing function-def dispatch checks `IDENT (` (peek the next two tokens). The new Subshell dispatch checks `(` at command-start — these don't overlap because function-def has the IDENT first. Verify your dispatch order respects this.

**Case-pattern disambiguation**: case patterns appear ONLY inside `case X in <pat>) ...`, which is parsed by `parse_case` (which consumes `(` and `)` itself). The top-level Command dispatcher never enters `parse_case`'s pattern context, so no conflict.

- [ ] **Step 5: Add executor `unreachable!` placeholder**

In `src/executor.rs::run_command` (or wherever the top-level Command dispatch lives), add an arm:

```rust
Command::Subshell { .. } => {
    unreachable!("Command::Subshell execution lands in Task 3; \
                  parser produces this now but the executor doesn't route it yet")
}
```

Similarly anywhere `Command` is exhaustively matched (e.g., `pipeline_is_pure_builtin` — wait, that was removed in v26. Check for any other exhaustive `Command` matches; the compiler will catch missing arms).

- [ ] **Step 6: Verify build + parser tests + clippy**

```bash
cargo build 2>&1 | tail -3
cargo test --bin huck parse_subshell 2>&1 | tail -15
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 9 parser tests pass; full suite 1082 + 9 = 1091, 0 fails, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "ast+parse: \`(list)\` subshell syntax

New Command::Subshell { body: Box<Sequence> } variant. Parser dispatches
Token::Op(LParen) at command-start (distinct from function-def's IDENT+LParen
and case-pattern's inside-case-clause LParen). EmptySubshell and
UnterminatedSubshell errors added. Executor unreachable!()s until Task 3."
```

---

## Task 2: Continuation classifier — Subshell reason

After this task, `(cmd<ENTER>` triggers continuation; the next line is collected; closing `)` ends the buffer.

**Files:** `src/continuation.rs`.

- [ ] **Step 1: Failing tests**

In `src/continuation.rs::tests`:

```rust
#[test]
fn classify_subshell_unclosed_is_incomplete() {
    assert_eq!(
        classify("(echo hi"),
        Completeness::Incomplete(ContinuationReason::Subshell)
    );
}

#[test]
fn classify_subshell_closed_is_complete() {
    assert_eq!(classify("(echo hi)"), Completeness::Complete);
}

#[test]
fn joiner_for_subshell_is_semi_space() {
    assert_eq!(joiner_for(ContinuationReason::Subshell, ""), "; ");
}
```

Run: expect failures (no `Subshell` variant yet).

- [ ] **Step 2: Add the variant and routing**

In `src/continuation.rs`:
```rust
pub enum ContinuationReason {
    Backslash,
    Operator,
    OpenQuote,
    Compound,
    Heredoc,
    Subshell,   // NEW
}
```

In `classify`, add a match arm BEFORE the generic compound-command fallback:
```rust
match parse(tokens) {
    Ok(_) => Completeness::Complete,
    Err(ParseError::UnterminatedSubshell) => {
        Completeness::Incomplete(ContinuationReason::Subshell)
    }
    Err(ParseError::UnterminatedIf | ParseError::UnterminatedLoop
        | ParseError::UnterminatedCase | ParseError::UnterminatedBrace
        | ParseError::UnterminatedFunction) => {
        Completeness::Incomplete(ContinuationReason::Compound)
    }
    Err(_) => Completeness::Error,
}
```

(Place the Subshell arm before or alongside the existing Compound arm — Subshell is its own reason for clarity, though functionally similar.)

In `joiner_for`:
```rust
ContinuationReason::Subshell => "; ",
```

- [ ] **Step 3: Verify**

```bash
cargo test --bin huck classify_subshell joiner_for_subshell 2>&1 | tail -5
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3 new tests pass; full suite ~1094, 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "continuation: Subshell reason for unterminated \`(cmd\`

classify maps ParseError::UnterminatedSubshell to Incomplete(Subshell).
joiner_for returns \"; \" for multi-line collection."
```

---

## Task 3: Executor — top-level fork + helper child-dispatch

After this task, `(echo hi)` actually runs in a forked subshell. Side-effect isolation works. Pipeline composition works (subshells as pipeline stages still fork once, not twice).

**Files:** `src/executor.rs`.

- [ ] **Step 1: Failing smoke test**

Create `tests/subshell_integration.rs`:
```rust
//! End-to-end tests for v28 subshell syntax.

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
fn subshell_basic_echo() {
    let (out, _) = run("(echo hi)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_isolates_var_assignment() {
    let (out, _) = run("FOO=outer\n(FOO=inner; echo in:$FOO)\necho out:$FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "in:inner"), "got: {out}");
    assert!(out.lines().any(|l| l.trim() == "out:outer"), "got: {out}");
}
```

Run: expect failures (the executor `unreachable!`s).

- [ ] **Step 2: Implement `Command::Subshell` in `run_command`**

In `src/executor.rs::run_command`, replace the Task 1 `unreachable!` arm with:

```rust
Command::Subshell { body: _ } => {
    // Fork via the existing v25 helper. The helper's child-side dispatch
    // (Step 3) handles Command::Subshell by running the inner Sequence
    // directly rather than recursing through run_command (which would
    // re-fork).
    let stdin_fd = libc::STDIN_FILENO;
    let stdout_fd = match sink {
        StdoutSink::Terminal => libc::STDOUT_FILENO,
        StdoutSink::Capture(_) => {
            // For top-level capture (rare — usually command substitution),
            // create a pipe; parent reads after the child exits.
            // ... (implementer: handle via existing capture-pipe infra).
        }
    };
    let stderr_fd = libc::STDERR_FILENO;
    let pid = fork_and_run_in_subshell(
        cmd, shell, stdin_fd, stdout_fd, stderr_fd, 0, &[]
    )?;
    let status = waitpid_blocking(pid)?;
    ExecOutcome::Continue(status)
}
```

(Adapt to whatever waitpid/status helpers already exist for top-level forks — likely the same pattern used by `run_exec_single` for externals.)

- [ ] **Step 3: Add child-side dispatch in `fork_and_run_in_subshell`**

In `src/executor.rs::fork_and_run_in_subshell`, find the child-side body-execution call (currently `run_command(cmd, shell, &mut sink)`). Wrap with a match:

```rust
let outcome = match cmd {
    Command::Subshell { body } => execute(body, shell, &mut sink),
    other => run_command(other, shell, &mut sink),
};
```

This is the critical anti-recursion guard: when the pipeline-stage path calls `fork_and_run_in_subshell` with a `Command::Subshell`, the child no longer calls `run_command` (which would re-fork) — it runs the body directly via `execute`. Net: one fork per Subshell, regardless of whether it's top-level or in a pipeline.

`execute` is the existing top-level Sequence executor (takes `&Sequence` and runs it through pipeline + connector logic). It returns `ExecOutcome` which the helper translates to a child exit status (existing logic).

- [ ] **Step 4: Verify the smoke tests pass + full suite**

```bash
cargo test --test subshell_integration 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 2 smoke tests pass; full suite ~1096, 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: fork on Command::Subshell; helper child-dispatch for inner Sequence

run_command's new Subshell arm forks via the v25 fork_and_run_in_subshell
helper. The helper's child-side gains a match: Command::Subshell runs its
body Sequence directly via execute() — avoiding the recursive fork that
would otherwise fire when a subshell is used as a pipeline stage. Net:
one fork per Subshell."
```

---

## Task 4: Full integration test suite + PTY + docs

Cover every spec test-table row + 2 PTY tests + doc updates.

**Files:**
- Extend: `tests/subshell_integration.rs`.
- Modify: `tests/pty_interactive.rs` (2 new tests).
- Modify: `docs/bash-divergences.md` (M-11 fixed + change-log).
- Modify: `README.md` (v28 row).

- [ ] **Step 1: Add remaining integration tests**

Append to `tests/subshell_integration.rs` (the 2 smoke tests from Task 3 already exist):

```rust
#[test]
fn subshell_isolates_cd() {
    // Capture cwd before and after a subshell `cd`; should be identical.
    let (out, _) = run("pwd > /tmp/v28_cd_before_$$\n(cd /tmp)\npwd > /tmp/v28_cd_after_$$\ndiff /tmp/v28_cd_before_$$ /tmp/v28_cd_after_$$ && echo SAME\nrm -f /tmp/v28_cd_before_$$ /tmp/v28_cd_after_$$\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "SAME"), "got: {out}");
}

#[test]
fn subshell_isolates_function_def() {
    // Function defined inside subshell is gone after subshell exits.
    let (out, err) = run("(f() { echo defined; }; f)\nf 2>&1\necho post-status=$?\nexit\n");
    // Inside subshell: `defined` line printed.
    assert!(out.lines().any(|l| l.trim() == "defined"), "got out: {out} err: {err}");
    // Outside: f is not defined; either stderr says "not found" or post-status != 0.
    let combined = format!("{out}{err}");
    assert!(combined.contains("not found") || out.contains("post-status=127") || out.contains("post-status=1"),
        "expected function-not-found indicator, got out: {out} err: {err}");
}

#[test]
fn subshell_exit_status_propagates() {
    let (out, _) = run("(exit 7)\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "7"), "got: {out}");
}

#[test]
fn subshell_with_sequence() {
    let (out, _) = run("(echo a; echo b)\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| l.trim() == "a" || l.trim() == "b").collect();
    assert_eq!(lines, vec!["a", "b"], "got: {out}");
}

#[test]
fn subshell_with_and_or() {
    let (out, _) = run("(true && echo ok)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn subshell_in_pipeline_first_stage() {
    let (out, _) = run("(echo hi) | cat\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_in_pipeline_last_stage() {
    let (out, _) = run("echo hi | (cat)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_nested_double_fork() {
    let (out, _) = run("((echo nested))\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "nested"), "got: {out}");
}

#[test]
fn subshell_backgrounded() {
    let tmp = format!("/tmp/v28_bg_{}", std::process::id());
    let script = format!(
        "(echo bg) > {tmp} &\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "bg"), "got: {out}");
}

#[test]
fn subshell_inherits_vars_from_parent() {
    let (out, _) = run("FOO=hi\n(echo got:$FOO)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "got:hi"), "got: {out}");
}

#[test]
fn subshell_in_function_body() {
    let (out, _) = run("f() { (echo from-subshell-in-func); }\nf\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "from-subshell-in-func"), "got: {out}");
}

#[test]
fn subshell_with_heredoc_inside() {
    let (out, _) = run("(cat <<EOF\nbody\nEOF\n)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "body"), "got: {out}");
}

#[test]
fn subshell_with_here_string_inside() {
    let (out, _) = run("(cat <<< hi)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}
```

- [ ] **Step 2: Add PTY tests**

In `tests/pty_interactive.rs`, append:

```rust
#[test]
fn pty_subshell_continuation_prompt_appears() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "(echo hi");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, ")");
    send(&mut session, ENTER);
    expect(&mut session, "hi");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_subshell_ctrl_c_aborts_body_collection() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "(echo hi");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    settle();
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
    // Buffer was discarded — confirm by running a fresh command.
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Find M-11 entry under "Compound commands" (or wherever it lives). Replace its body to `[fixed (2026-05-26)]` form with description:

```markdown
- **M-11: Subshells `( list )`** — `[fixed (2026-05-26)]` high. Now supported: `(list)` runs the inner sequence in a forked subshell with isolated side effects. Reuses v25's fork machinery; the helper's child-side dispatch handles Subshell-as-pipeline-stage without a recursive double-fork. Top-level `(cmd)`, pipeline stages, backgrounded `(cmd) &`, nested `((cmd))`, and composition with heredocs/here-strings all work.
```

Update Tier 2 summary count (drops by 1).

Add change-log entry:
```markdown
- **2026-05-26**: M-11 (subshell syntax `(list)`) shipped as v28.
```

- [ ] **Step 4: Update `README.md`**

Add v28 row to the status table:
```
| v28       | Subshell syntax (`(list)`)                               |
```

If subshells appear in "Not yet implemented" prose, remove the mention.

- [ ] **Step 5: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: ~1109 tests pass (1096 + 13 integration + 2 PTY = 1111), 0 fails, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "v28: subshell syntax — integration tests + PTY + docs

13 integration tests covering isolation (cd/vars/functions), exit
propagation, sequence/and-or bodies, pipeline composition (both stages),
nested double-fork, backgrounded, inheritance, function-body
composition, heredoc/here-string composition. 2 PTY tests for
continuation prompt + Ctrl-C abort. M-11 marked fixed in audit doc;
README v28 row added."
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the final cross-cutting opus review. After approval:

```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v28-subshell-syntax
git -C /home/john/projects/shuck branch -d v28-subshell-syntax
```

---

## Self-review checklist

1. **Spec coverage**: every spec section maps to a task.
   - Lexer changes (none) → no task.
   - AST changes → Task 1.
   - Parser changes → Task 1.
   - Continuation classifier → Task 2.
   - Executor changes → Task 3.
   - Edge cases → Task 4 integration tests.
   - Doc updates → Task 4.

2. **Placeholders**: every step shows concrete code. The `parse_sequence_until` helper in Task 1 Step 4 is described but its exact signature depends on the implementer reading the existing `parse_sequence` (which may already accept a stop-token list per v21/v22 work).

3. **Type consistency**: `Command::Subshell { body: Box<Sequence> }` everywhere; parser builds it; classifier uses ParseError::UnterminatedSubshell → Incomplete(Subshell); executor's `run_command` arm forks via the v25 helper; helper's child-side runs `execute(body)`.

4. **Order dependencies**:
   - Task 1 must precede everything (introduces the AST variant and parser).
   - Task 2 is independent of Task 3 (classifier vs executor).
   - Task 3 depends on Task 1 (executor sees the new variant).
   - Task 4 depends on all of 1-3.

5. **Backward-compat callouts**: no breaking changes. `(...)` was a parse error pre-v28; now a valid command. Existing tests don't exercise it; no test should break. The v25 `fork_and_run_in_subshell` helper gains a child-side dispatch that's purely additive (an extra match arm); no existing callers are affected.
