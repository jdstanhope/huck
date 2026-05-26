# v29: FD Duplication Redirects — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement POSIX `n>&m` fd-duplication (limited to fds 1 and 2) and bash `&>file` / `&>>file` combined-redirect shortcuts. Closes M-18 from `docs/bash-divergences.md`.

**Architecture:** Four new lexer operators (`DupOut`, `DupErr`, `AndRedirOut`, `AndRedirAppend`) feed a new `Redirect::Dup { fd, source }` AST variant. Parser routes `>&`/`2>&` into stdout/stderr Dup variants and desugars `&>`/`&>>` into a Truncate/Append + Dup pair. Executor resolves the target Word to an i32 pre-fork, then `dup2`s in the child — via `pre_exec` for externals, via direct in-process dup2 for the v25 fork helper. Application order is stdout-then-stderr (a documented divergence for the rare `2>&1 >file` anti-pattern).

**Tech Stack:** Rust 1.95; libc for dup2; existing huck modules (`src/lexer.rs`, `src/command.rs`, `src/executor.rs`). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-26-huck-fd-duplication-design.md`.

**Branch:** `v29-fd-duplication` (off `main` at commit `3d02dd0`).

**Baseline:** 1115 tests pass, 0 clippy warnings.

---

## File structure

- `src/lexer.rs` — 4 new Operator variants + peek-chain extensions in `>`/`2`/`&` arms.
- `src/command.rs` — `Redirect::Dup { fd: i32, source: Word }` variant; parser arms for the 4 operators (`DupOut`, `DupErr` → set fd-Dup; `AndRedirOut`, `AndRedirAppend` → desugar to Truncate/Append + Dup); `lit_word` helper for the synthesized "1".
- `src/executor.rs` — `resolve_fd_target` helper (expand Word + parse i32); pre_exec dup2 in `spawn_external_with_fds`; direct dup2 in `fork_and_run_in_subshell`; replace Task 1's `unreachable!`s.
- `tests/fd_dup_integration.rs` (new) — end-to-end coverage.
- `docs/bash-divergences.md` — M-18 fixed; new Tier-1 entry for the order divergence; change-log entry.
- `README.md` — v29 status row.

---

## Task 1: Lexer + AST + parser (front-end)

After this task, `cmd 2>&1`, `cmd >&2`, `cmd &>file`, `cmd &>>file` all parse into the AST. Executor `unreachable!`s on the new `Dup` variant pending Task 2.

**Files:** `src/lexer.rs`, `src/command.rs`, `src/executor.rs` (placeholders only).

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `1115 0` and `0`.

- [ ] **Step 2: Add 4 Operator variants + `Redirect::Dup`**

In `src/lexer.rs`:
```rust
pub enum Operator {
    // existing...
    DupOut,             // NEW: >&
    DupErr,             // NEW: 2>&
    AndRedirOut,        // NEW: &>
    AndRedirAppend,     // NEW: &>>
}
```

In `src/command.rs`:
```rust
pub enum Redirect {
    Read(Word),
    Truncate(Word),
    Append(Word),
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),
    Dup { fd: i32, source: Word },             // NEW
}
```

- [ ] **Step 3: Failing lexer tests**

In `src/lexer.rs::tests`:
```rust
#[test]
fn tokenize_dup_out_basic() {
    let tokens = tokenize(">&").unwrap();
    assert_eq!(tokens, vec![Token::Op(Operator::DupOut)]);
}

#[test]
fn tokenize_dup_err_basic() {
    let tokens = tokenize("2>&").unwrap();
    assert_eq!(tokens, vec![Token::Op(Operator::DupErr)]);
}

#[test]
fn tokenize_and_redir_out() {
    let tokens = tokenize("&>").unwrap();
    assert_eq!(tokens, vec![Token::Op(Operator::AndRedirOut)]);
}

#[test]
fn tokenize_and_redir_append() {
    let tokens = tokenize("&>>").unwrap();
    assert_eq!(tokens, vec![Token::Op(Operator::AndRedirAppend)]);
}

#[test]
fn tokenize_dup_in_context() {
    let tokens = tokenize("cmd 2>&1").unwrap();
    assert_eq!(tokens.len(), 3);
    assert!(matches!(tokens[0], Token::Word(_)));
    assert!(matches!(tokens[1], Token::Op(Operator::DupErr)));
    assert!(matches!(tokens[2], Token::Word(_)));
}

#[test]
fn tokenize_redir_out_regression() {
    assert_eq!(tokenize(">").unwrap(), vec![Token::Op(Operator::RedirOut)]);
    assert_eq!(tokenize(">>").unwrap(), vec![Token::Op(Operator::RedirAppend)]);
}

#[test]
fn tokenize_redir_err_regression() {
    assert_eq!(tokenize("2>").unwrap(), vec![Token::Op(Operator::RedirErr)]);
    assert_eq!(tokenize("2>>").unwrap(), vec![Token::Op(Operator::RedirErrAppend)]);
}

#[test]
fn tokenize_background_regression() {
    assert_eq!(tokenize("&").unwrap(), vec![Token::Op(Operator::Background)]);
    assert_eq!(tokenize("&&").unwrap(), vec![Token::Op(Operator::And)]);
}
```

Run: `cargo test --bin huck tokenize_dup tokenize_and_redir tokenize_redir_out_regression tokenize_redir_err_regression tokenize_background_regression` — expect failures for the new operators.

- [ ] **Step 4: Extend lexer dispatch chains**

In `src/lexer.rs::tokenize`:

**`>` arm**:
```rust
'>' => {
    if has_token { /* existing flush */ }
    if chars.peek() == Some(&'>') {
        chars.next();
        tokens.push(Token::Op(Operator::RedirAppend));
    } else if chars.peek() == Some(&'&') {   // NEW
        chars.next();
        tokens.push(Token::Op(Operator::DupOut));
    } else {
        tokens.push(Token::Op(Operator::RedirOut));
    }
}
```

**`2` arm** (existing `2>` recognition extends): after consuming `2>`, add a peek for `&`:
```rust
// After consuming '2>':
if chars.peek() == Some(&'>') {
    chars.next();
    tokens.push(Token::Op(Operator::RedirErrAppend));
} else if chars.peek() == Some(&'&') {       // NEW
    chars.next();
    tokens.push(Token::Op(Operator::DupErr));
} else {
    tokens.push(Token::Op(Operator::RedirErr));
}
```

**`&` arm**:
```rust
'&' => {
    if has_token { /* existing flush */ }
    if chars.peek() == Some(&'&') {
        chars.next();
        tokens.push(Token::Op(Operator::And));
    } else if chars.peek() == Some(&'>') {   // NEW
        chars.next();
        if chars.peek() == Some(&'>') {
            chars.next();
            tokens.push(Token::Op(Operator::AndRedirAppend));
        } else {
            tokens.push(Token::Op(Operator::AndRedirOut));
        }
    } else {
        tokens.push(Token::Op(Operator::Background));
    }
}
```

Run: `cargo test --bin huck tokenize_dup tokenize_and_redir tokenize_redir tokenize_background` — all should pass.

- [ ] **Step 5: Failing parser tests**

In `src/command.rs::tests`:
```rust
#[test]
fn parse_dup_stdout_from_fd2() {
    let tokens = tokenize("cmd >&2").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    let Some(Redirect::Dup { fd, source }) = &e.stdout else { panic!("got {:?}", e.stdout) };
    assert_eq!(*fd, 1);
    // source Word's first part should be Literal "2".
    assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "2"));
}

#[test]
fn parse_dup_stderr_from_fd1() {
    let tokens = tokenize("cmd 2>&1").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    let Some(Redirect::Dup { fd, source }) = &e.stderr else { panic!("got {:?}", e.stderr) };
    assert_eq!(*fd, 2);
    assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "1"));
}

#[test]
fn parse_and_redir_out_desugars() {
    let tokens = tokenize("cmd &>file").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    // stdout = Truncate(file)
    let Some(Redirect::Truncate(file)) = &e.stdout else { panic!("got {:?}", e.stdout) };
    assert!(matches!(&file.0[0], WordPart::Literal { text, .. } if text == "file"));
    // stderr = Dup{fd:2, source:"1"}
    let Some(Redirect::Dup { fd, source }) = &e.stderr else { panic!("got {:?}", e.stderr) };
    assert_eq!(*fd, 2);
    assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "1"));
}

#[test]
fn parse_and_redir_append_desugars() {
    let tokens = tokenize("cmd &>>file").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    let Some(Redirect::Append(_)) = &e.stdout else { panic!("got {:?}", e.stdout) };
    let Some(Redirect::Dup { fd, .. }) = &e.stderr else { panic!() };
    assert_eq!(*fd, 2);
}

#[test]
fn parse_dup_with_var_target() {
    // 2>&$FD — source is a Word with a Var part, not a literal.
    let tokens = tokenize("cmd 2>&$FD").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    let Some(Redirect::Dup { source, .. }) = &e.stderr else { panic!() };
    assert!(source.0.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "FD")));
}

#[test]
fn parse_dup_in_pipeline_stage() {
    let tokens = tokenize("cmd 2>&1 | grep").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    let Command::Simple(SimpleCommand::Exec(stage0)) = &p.commands[0] else { panic!() };
    assert!(matches!(&stage0.stderr, Some(Redirect::Dup { .. })));
    let Command::Simple(SimpleCommand::Exec(stage1)) = &p.commands[1] else { panic!() };
    assert!(stage1.stderr.is_none());
}

#[test]
fn parse_combined_dup_and_file_redirect() {
    let tokens = tokenize("cmd >file 2>&1").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    assert!(matches!(&e.stdout, Some(Redirect::Truncate(_))));
    assert!(matches!(&e.stderr, Some(Redirect::Dup { fd: 2, .. })));
}
```

Run: expect failures (parser doesn't have the arms yet).

- [ ] **Step 6: Add parser arms for the 4 operators**

Find the per-stage redirect-consumption code in `src/command.rs`. Add four arms:

```rust
Token::Op(Operator::DupOut) => {
    let target = consume_next_word(iter)?;
    cmd.stdout = Some(Redirect::Dup { fd: 1, source: target });
}
Token::Op(Operator::DupErr) => {
    let target = consume_next_word(iter)?;
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: target });
}
Token::Op(Operator::AndRedirOut) => {
    let target = consume_next_word(iter)?;
    cmd.stdout = Some(Redirect::Truncate(target));
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
}
Token::Op(Operator::AndRedirAppend) => {
    let target = consume_next_word(iter)?;
    cmd.stdout = Some(Redirect::Append(target));
    cmd.stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
}
```

`consume_next_word` is whatever the existing helper is (look at how `RedirOut` / `RedirErr` consume their target — copy the pattern; on missing-word, return `ParseError::MissingRedirectTarget` or equivalent).

`lit_word` helper:
```rust
fn lit_word(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}
```

Place it near other Word-construction helpers in `src/command.rs`.

- [ ] **Step 7: Add executor `unreachable!` placeholder for Dup**

In `src/executor.rs`, find every site that matches on `Redirect` (look in `resolve()`, `open_stage_files`, `spawn_external_with_fds`, `fork_and_run_in_subshell`). Add `Redirect::Dup { .. }` arms:

```rust
// For stdout side:
Some(Redirect::Dup { .. }) => unreachable!(
    "Redirect::Dup handling lands in Task 2; parser produces this now \
     but the executor doesn't route it yet"
),
```

For stderr side (similar). The existing pattern for unreachable! redirects in non-applicable positions (e.g. `Truncate` on stdin) is your model.

- [ ] **Step 8: Verify build + parser tests + clippy**

```bash
cargo build 2>&1 | tail -3
cargo test --bin huck parse_dup parse_and_redir parse_combined 2>&1 | tail -15
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 7 parser tests pass; ~1115 + 8 lexer + 7 parser = 1130; 0 fails, 0 warnings.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "ast+lex+parse: fd-duplication redirects (\`>&\`, \`2>&\`, \`&>\`, \`&>>\`)

Four new Operator variants (DupOut, DupErr, AndRedirOut, AndRedirAppend)
extend the existing >/2>/& peek-chains. New Redirect::Dup { fd, source }
AST variant. Parser routes DupOut/DupErr into stdout/stderr Dup, and
desugars AndRedirOut/AndRedirAppend into Truncate/Append + Dup{fd:2,
source:'1'}. Executor placeholder unreachable!()s until Task 2."
```

---

## Task 2: Executor — dup2 in child

After this task, `cmd 2>&1` actually merges stderr into stdout at runtime. Externals (via std::process::Command + pre_exec) and in-process forks (via libc::dup2) both work.

**Files:** `src/executor.rs`.

- [ ] **Step 1: Add `resolve_fd_target` helper**

Place near `expand_assignment` callers or with other small executor helpers:

```rust
/// Expands `source` to a string and parses as an fd number (e.g. "1" or "2").
/// Errors if expansion is not a valid non-negative integer.
fn resolve_fd_target(source: &Word, shell: &mut Shell) -> Result<i32, io::Error> {
    let expanded = expand_assignment(source, shell);
    expanded.parse::<i32>()
        .map_err(|_| io::Error::other(format!("bad fd: {expanded}")))
}
```

Unit test:
```rust
#[test]
fn resolve_fd_target_parses_literal_number() {
    let mut shell = Shell::new();
    let word = lit_word("1");
    assert_eq!(resolve_fd_target(&word, &mut shell).unwrap(), 1);
}

#[test]
fn resolve_fd_target_rejects_non_numeric() {
    let mut shell = Shell::new();
    let word = lit_word("notanumber");
    assert!(resolve_fd_target(&word, &mut shell).is_err());
}
```

- [ ] **Step 2: Failing smoke test (integration)**

Create `tests/fd_dup_integration.rs`:
```rust
//! End-to-end tests for v29 fd-duplication redirects.

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
fn dup_stderr_to_stdout_canonical() {
    // sh -c 'echo stderr-msg >&2' writes to stderr. With 2>&1 from huck,
    // the message should appear on huck's stdout (our run() captures it).
    let (out, _err) = run("sh -c 'echo stderr-msg >&2' 2>&1\nexit\n");
    assert!(out.contains("stderr-msg"), "got stdout: {out}");
}
```

Run: expect failure (executor `unreachable!`s on Dup).

- [ ] **Step 3: Wire dup2 in `spawn_external_with_fds` (externals)**

In `src/executor.rs::spawn_external_with_fds`:
1. Before configuring stdio: check if `cmd.stdout` or `cmd.stderr` is a Dup. If so, resolve target fds via `resolve_fd_target` BEFORE fork.
2. For Dup redirects, use `Stdio::inherit()` for the corresponding fd (we'll dup2 in pre_exec).
3. Add a second `pre_exec` closure (chained after the existing `reset_job_control_signals_in_child`):

```rust
let stdout_dup_target: Option<i32> = match &cmd.stdout {
    Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
    _ => None,
};
let stderr_dup_target: Option<i32> = match &cmd.stderr {
    Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
    _ => None,
};

// ... configure stdio with Stdio::inherit() for Dup sides, normal for others ...

unsafe {
    process.pre_exec(move || {
        if let Some(fd) = stdout_dup_target {
            if libc::dup2(fd, 1) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if let Some(fd) = stderr_dup_target {
            if libc::dup2(fd, 2) < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    });
}
```

The existing pre_exec for signal-reset stays; multiple `process.pre_exec(...)` calls compose (Rust's std runs them in registration order in the child after fork, before exec).

Order: stdout-dup BEFORE stderr-dup. This matches the canonical `>file 2>&1` semantic (stdout opens file first, then stderr dups from fd 1 which is now the file).

- [ ] **Step 4: Wire dup2 in `fork_and_run_in_subshell` (in-process)**

For huck's own in-process fork path, the child runs Rust code directly (no exec). Apply the same dup2 logic AFTER the existing dup2(stdio_fd, 0/1/2) and BEFORE running the command body.

The pattern depends on where Dup is routed in this path — Dup redirects in pipeline-stage commands need handling. Look at how `open_stage_files` currently routes stdin/stdout/stderr; for Dup variants, after the existing stdio dup2s in the child, perform an additional `libc::dup2(target_fd, fd)` for any Dup.

Resolution can happen before fork (in parent) and the resolved i32 passed into the child via the existing closure-capture pattern.

If the in-process path doesn't currently route stdout/stderr Redirects (everything goes through fd-fds), the implementer needs to thread Dup info through. Look at how heredoc body is threaded (`DeferredHeredoc(Word)` → expanded in parent → bytes written to child); analogous for Dup (`Dup{...}` → target fd resolved in parent → dup2 applied in child).

- [ ] **Step 5: Replace the Task 1 `unreachable!` arms**

In every executor site where `Redirect::Dup { .. }` was a Task 1 placeholder, replace with the real handling per the above. For sites that should NOT see Dup (e.g. Dup on stdin — `cmd <&n` is OUT OF SCOPE per spec), leave an `unreachable!` with a more specific message.

- [ ] **Step 6: Verify the smoke test + full suite**

```bash
cargo test --test fd_dup_integration 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: smoke test passes; full suite ~1132, 0 fails, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "exec: dup2 in child for fd-duplication redirects

resolve_fd_target expands Dup's source Word and parses to i32 in the
parent. For external spawn (spawn_external_with_fds), a chained pre_exec
closure runs libc::dup2 in the child after stdio setup and before exec.
For in-process fork (fork_and_run_in_subshell), the child applies the
same dup2 directly after its existing stdio dup2.

Application order: stdout-dup before stderr-dup. This matches canonical
\`>file 2>&1\` semantics; the rare \`2>&1 >file\` anti-pattern is a
documented divergence (huck preserves field-based redirect storage, so
source order is not preserved)."
```

---

## Task 3: Integration test suite

Cover every spec test-table row.

**Files:** `tests/fd_dup_integration.rs`.

- [ ] **Step 1: Add remaining integration tests**

Append to `tests/fd_dup_integration.rs` (smoke test from Task 2 already covers `dup_stderr_to_stdout_canonical`):

```rust
#[test]
fn dup_stdout_to_stderr() {
    let tmp = format!("/tmp/v29_dup_stdout_{}", std::process::id());
    let script = format!(
        "echo hi 1>&2 2> {tmp}\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn combined_redirect_canonical_form() {
    let tmp = format!("/tmp/v29_combined_{}", std::process::id());
    let script = format!(
        "sh -c 'echo out; echo err >&2' >{tmp} 2>&1\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn and_redir_out_to_file() {
    let tmp = format!("/tmp/v29_andout_{}", std::process::id());
    let script = format!(
        "sh -c 'echo out; echo err >&2' &>{tmp}\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn and_redir_append_to_file() {
    let tmp = format!("/tmp/v29_andappend_{}", std::process::id());
    let script = format!(
        "echo first > {tmp}\nsh -c 'echo second; echo err >&2' &>>{tmp}\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "3"), "got: {out}");
}

#[test]
fn dup_in_pipeline_stage() {
    let (out, _) = run("sh -c 'echo a; echo b >&2' 2>&1 | grep -c .\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn dup_with_inline_assignment() {
    let (out, _) = run("FOO=hi sh -c 'echo $FOO >&2' 2>&1\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn dup_with_subshell_inner_form() {
    // Outer form `(cmd) 2>&1` requires compound-command redirects (separate gap).
    // Inner form `(cmd 2>&1)` works via existing subshell + dup composition.
    let (out, _) = run("(sh -c 'echo from-sub >&2' 2>&1)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "from-sub"), "got: {out}");
}

#[test]
fn dup_runtime_bad_fd_target() {
    // Non-numeric target → runtime error.
    let (out, err) = run("sh -c true 2>&notanumber\nexit\n");
    let combined = format!("{out}{err}");
    assert!(combined.contains("bad fd") || combined.contains("notanumber"),
        "expected bad-fd error, got out: {out} err: {err}");
}

#[test]
fn echo_to_stderr_shorthand() {
    let tmp = format!("/tmp/v29_shorthand_{}", std::process::id());
    let script = format!(
        "echo error >&2 2> {tmp}\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "error"), "got: {out}");
}

#[test]
fn dup_with_var_target_at_runtime() {
    // 2>&$FD with FD=1 — target Word has a Var part; expansion yields "1".
    let (out, _) = run("FD=1 sh -c 'echo varfd >&2' 2>&$FD\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "varfd"), "got: {out}");
}
```

- [ ] **Step 2: Verify**

```bash
cargo test --test fd_dup_integration 2>&1 | tail -15
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 10 integration tests pass; full suite ~1142, 0 fails, 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: full v29 fd-duplication integration coverage

10 new tests covering: canonical 2>&1 (stderr to stdout), 1>&2 (stdout
to stderr), >file 2>&1 (both to file), bash &>file shortcut, bash
&>>file append shortcut, dup-in-pipeline-stage, dup with inline
assignment, dup inside subshell, runtime bad-fd error, echo >&2
shorthand, var-target ($FD) expansion."
```

---

## Task 4: Docs + final tidy

Mark M-18 fixed; add v29 row; document the `2>&1 >file` order divergence.

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Update `docs/bash-divergences.md`**

Find the M-18 entry. Replace its body to:
```markdown
- **M-18: fd-duplication `n>&m` and `&>file`** — `[fixed (2026-05-26)]` high. Now supported: `2>&1` (POSIX), `1>&2`, `&>file` (bash), `&>>file` (bash). Limited to fds 1 and 2 (arbitrary `n>&m` with n≠1,2 may parse but isn't claimed to work). `>&-` (close-fd) is out of scope. **Known divergence**: huck applies stdout-redirect before stderr-redirect (field-based AST loses source order); `cmd 2>&1 >file` (rare anti-pattern) produces both-to-file rather than bash's stderr-to-terminal. Canonical `>file 2>&1` works correctly.
```

Update Tier 2 summary count (drops by 1).

Add a new Tier 4 (or Tier 3 if Intentional fits better) entry for the order divergence so it's tracked:
```markdown
### L-XX: Redirect source-order not preserved (`2>&1 >file` anti-pattern)
- **Status**: intentional (v29)
- **Severity**: low
- **huck**: `cmd 2>&1 >file` is treated identically to `cmd >file 2>&1` — both fds end up at the file. The field-based ExecCommand AST (stdin/stdout/stderr) stores at most one redirect per fd and can't preserve source order.
- **bash**: `cmd 2>&1 >file` puts stderr to the terminal and stdout to the file (because `2>&1` dups stderr to the CURRENT stdout, which is the terminal at that point, then `>file` redirects stdout). The canonical form is `cmd >file 2>&1`.
- **Why intentional**: source-order preservation requires refactoring ExecCommand to `redirects: Vec<(SourceFd, Redirect)>` — a substantial change. The canonical form covers >99% of real usage.
- **Workaround**: write `cmd >file 2>&1` (or `cmd &>file`).
```

(Use whatever ID is next in the L-/I- sequence.)

Add change-log entry:
```markdown
- **2026-05-26**: M-18 (fd-duplication redirects) shipped as v29. Documented order-divergence for `2>&1 >file` anti-pattern as L-XX.
```

- [ ] **Step 2: Update `README.md`**

Add v29 row to the status table:
```
| v29       | FD-duplication redirects (`2>&1`, `&>file`)              |
```

Remove `2>&1` from any "Not yet implemented" paragraph if present.

- [ ] **Step 3: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: same counts as Task 3 (no code changes); 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: M-18 fixed; L-XX order divergence tracked; v29 in README

M-18 entry marked [fixed (2026-05-26)] with a clear callout to the
canonical >file 2>&1 form. New L-XX (or similar) entry tracks the
deliberate divergence for the rare \`2>&1 >file\` anti-pattern. README
status table gets v29 row."
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
git -C /home/john/projects/shuck merge --ff-only v29-fd-duplication
git -C /home/john/projects/shuck branch -d v29-fd-duplication
```

---

## Self-review checklist

1. **Spec coverage**: every spec section maps to a task.
   - Lexer + AST + Parser → Task 1.
   - Executor → Task 2.
   - Edge cases + Tests → Task 3.
   - Doc updates → Task 4.

2. **Placeholders**: every step shows concrete code. The "look at how RedirOut consumes its target" and "look at how heredoc body is threaded" prompts in Tasks 1-2 are navigation hints — the implementer reads the existing code to find the matching pattern.

3. **Type consistency**: `Redirect::Dup { fd: i32, source: Word }` flows from parser through resolve_fd_target through dup2. `fd` is 1 or 2; `source` is a Word that expands to an fd-number string.

4. **Order dependencies**:
   - Task 1 must precede everything.
   - Task 2 depends on Task 1.
   - Task 3 depends on Task 2 (integration tests need execution to work).
   - Task 4 is independent of code; can ship any time after Task 3.

5. **Backward-compat callouts**: no breaking changes. The 4 new lexer operators don't affect existing tokenization (lexer dispatch is additive). The Redirect enum gains a variant; consumers add an arm. Existing tests don't exercise the new operators and shouldn't break.
