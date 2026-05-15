# shuck Command Substitution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `$(...)` and `` `...` `` command substitution to shuck — captured stdout (with trailing newlines stripped) replaces the expression; unquoted substitutions word-split like `$VAR`; the parent shell's `$?` reflects the substituted command's exit; assignments and `export`/`unset` inside `$(...)` execute against a cloned `Shell` and don't leak.

**Architecture:** A new `WordPart::CommandSub { sequence: command::Sequence, quoted: bool }` makes the AST recursive (Sequence contains Words that contain CommandSubs that contain Sequences). The lexer scans `$(...)` by balancing parens with quote/escape awareness, scans backticks with bash escape rules, then recursively tokenizes+parses the body into a `Sequence`. The executor grows an `execute_capturing` entry point — a sink-threading refactor lets terminal-stage stdout go to a `Vec<u8>` instead of `io::stdout()`. `expand.rs` adds `run_substitution`, which clones the parent `Shell`, runs the inner sequence with capture, strips trailing newlines, and updates the parent's `$?`.

**Tech Stack:** Rust (edition 2024). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-15-shuck-command-substitution-design.md`

---

## File Structure

| File | Change |
|------|--------|
| `src/executor.rs` | Add `enum StdoutSink<'a> { Terminal, Capture(&'a mut Vec<u8>) }`. Refactor `execute` to delegate to `execute_inner(seq, shell, sink)`. Add `execute_capturing(seq, shell) -> (String, i32)`. Thread `sink: &mut StdoutSink` through `run_pipeline`, `run_single`, `run_multi_stage`, `run_exec_single`, `run_subprocess`. Terminal builtin writes go to either `io::stdout()` or the capture buffer; terminal subprocesses get `Stdio::piped()` and drain stdout into the buffer. New tests for `execute_capturing`. |
| `src/lexer.rs` | Add `LexError::UnterminatedSubstitution`, `LexError::SubstitutionLexError(Box<LexError>)`, `LexError::SubstitutionParseError(crate::command::ParseError)`. Add `WordPart::CommandSub { sequence: crate::command::Sequence, quoted: bool }`. Add `scan_paren_substitution` and `scan_backtick_substitution` helpers. Hook `$(...)` into the `$` arm (outside and inside double quotes). Hook backticks into a new arm in both contexts. ~25 new tests. Imports `crate::command::{self, ParseError, Sequence}`. |
| `src/command.rs` | Refactor `try_split_assignment` to move-semantics (`Word -> Result<(String, Word), Word>`) so it doesn't need to clone `Sequence` from `WordPart::CommandSub`. Update `finalize_stage` accordingly. Add ~3 parser tests for CommandSub placement. |
| `src/expand.rs` | Change `expand` signature from `(&Word, &Shell)` to `(&Word, &mut Shell)` and `expand_assignment` similarly. Add two new arms for `WordPart::CommandSub`. Add `run_substitution(&Sequence, &mut Shell) -> String` (clones shell, calls `executor::execute_capturing`, strips trailing newlines, updates parent `$?`). Add `strip_trailing_newlines`. ~7 new tests. |
| `src/shell.rs` | Change `lex_error_message` return type to `String`. Add arms for the 3 new `LexError` variants (recursive render for the wrapped variants). Update single call site in `process_line`. |
| `src/shell_state.rs` | Add `#[derive(Clone)]` to `Shell`. |
| `src/builtins.rs` | No changes. |

**Why the task order:** Task 1 lands the executor refactor in isolation — `execute_capturing` exists but nothing calls it yet. Task 2 lands the AST + expand wiring + `run_substitution`, with tests that construct synthetic CommandSub Words (no lexer participation needed). Tasks 3 and 4 add lexer recognition for `$(...)` and backticks respectively; after each, that form works end-to-end. Task 5 is the comprehensive smoke test. Per-task verification is `cargo test` (binary-only crate — never `cargo test --lib`). The strict gate for every task is **0 failed** and **0 warnings**.

---

## Task 1: Executor sink-threading refactor

Add the plumbing that lets the executor capture terminal-stage stdout into a buffer instead of writing it to the inherited terminal. No new user-visible behavior — `execute` continues to behave exactly as before. The new `execute_capturing` is exercised by tests only.

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Replace `src/executor.rs` entirely** with the version below.

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{
    Connector, ExecCommand, Pipeline, Redirect, Sequence, SimpleCommand,
};
use crate::expand::{expand, expand_assignment};
use crate::shell_state::Shell;

/// Where the terminal stage of a top-level pipeline sends its stdout when
/// there's no explicit `> file` redirect.
pub enum StdoutSink<'a> {
    Terminal,
    Capture(&'a mut Vec<u8>),
}

pub fn execute(seq: &Sequence, shell: &mut Shell) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    execute_inner(seq, shell, &mut sink)
}

/// Runs a sequence with stdout captured to a buffer. The returned status is
/// the last command's exit code (`ExecOutcome::Exit` and `Continue` are both
/// treated as a normal status here — `exit N` inside `$(...)` terminates the
/// substitution with status N, not the parent shuck).
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = {
        let mut sink = StdoutSink::Capture(&mut buf);
        execute_inner(seq, shell, &mut sink)
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first, shell, sink);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, pipeline) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_pipeline(pipeline, shell, sink);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0], shell, sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink)
    }
}

// ----- resolved command (post-expansion) ------------------------------------

struct ResolvedCommand {
    program: String,
    args: Vec<String>,
    stdin: Option<String>,
    stdout: Option<ResolvedRedirect>,
    stderr: Option<ResolvedRedirect>,
}

enum ResolvedRedirect {
    Truncate(String),
    Append(String),
}

fn expand_single(word: &crate::lexer::Word, shell: &mut Shell) -> Result<String, ()> {
    let fields = expand(word, shell);
    if fields.len() == 1 {
        Ok(fields.into_iter().next().unwrap())
    } else {
        eprintln!("shuck: ambiguous redirect");
        Err(())
    }
}

fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = expand(&cmd.program, shell);
    if prog_fields.is_empty() {
        eprintln!("shuck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();
    for word in &cmd.args {
        args.extend(expand(word, shell));
    }
    let stdin = match &cmd.stdin {
        Some(word) => Some(expand_single(word, shell).map_err(|()| 1)?),
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(Redirect::Truncate(w)) => {
            Some(ResolvedRedirect::Truncate(expand_single(w, shell).map_err(|()| 1)?))
        }
        Some(Redirect::Append(w)) => {
            Some(ResolvedRedirect::Append(expand_single(w, shell).map_err(|()| 1)?))
        }
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(Redirect::Truncate(w)) => {
            Some(ResolvedRedirect::Truncate(expand_single(w, shell).map_err(|()| 1)?))
        }
        Some(Redirect::Append(w)) => {
            Some(ResolvedRedirect::Append(expand_single(w, shell).map_err(|()| 1)?))
        }
        None => None,
    };
    Ok(ResolvedCommand { program, args, stdin, stdout, stderr })
}

// ----- redirect file handling -----------------------------------------------

struct StageFiles {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

fn open_stage_files(cmd: &ResolvedCommand) -> Result<StageFiles, ()> {
    let stdin = match &cmd.stdin {
        Some(path) => match File::open(path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {path}: {e}");
                return Err(());
            }
        },
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(redirect) => match open_resolved(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", resolved_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(redirect) => match open_resolved(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", resolved_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    Ok(StageFiles { stdin, stdout, stderr })
}

fn open_resolved(redirect: &ResolvedRedirect) -> io::Result<File> {
    match redirect {
        ResolvedRedirect::Truncate(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path),
        ResolvedRedirect::Append(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(path),
    }
}

fn resolved_path(redirect: &ResolvedRedirect) -> &str {
    match redirect {
        ResolvedRedirect::Truncate(p) | ResolvedRedirect::Append(p) => p,
    }
}

fn status_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1))
}

// ----- single command -------------------------------------------------------

fn run_single(cmd: &SimpleCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell, sink),
        SimpleCommand::Assign { name, value } => {
            shell.set(name, expand_assignment(value, shell));
            ExecOutcome::Continue(0)
        }
    }
}

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };
    let files = match open_stage_files(&resolved) {
        Ok(f) => f,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&resolved.program) {
        match files.stdout {
            Some(mut file) => {
                builtins::run_builtin(&resolved.program, &resolved.args, &mut file, shell)
            }
            None => match sink {
                StdoutSink::Terminal => {
                    let mut out = io::stdout();
                    builtins::run_builtin(&resolved.program, &resolved.args, &mut out, shell)
                }
                StdoutSink::Capture(buf) => {
                    builtins::run_builtin(&resolved.program, &resolved.args, *buf, shell)
                }
            },
        }
    } else {
        run_subprocess(&resolved, files, shell, sink)
    }
}

fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());
    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    let want_capture = matches!(sink, StdoutSink::Capture(_));
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    } else if want_capture {
        process.stdout(Stdio::piped());
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    match process.spawn() {
        Ok(mut child) => {
            if let StdoutSink::Capture(buf) = sink {
                if let Some(mut child_stdout) = child.stdout.take() {
                    let _ = io::copy(&mut child_stdout, *buf);
                }
            }
            match child.wait() {
                Ok(status) => ExecOutcome::Continue(status_code(&status)),
                Err(e) => {
                    eprintln!("shuck: {}: {e}", cmd.program);
                    ExecOutcome::Continue(1)
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            eprintln!("shuck: command not found: {}", cmd.program);
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            eprintln!("shuck: {}: {e}", cmd.program);
            ExecOutcome::Continue(1)
        }
    }
}

// ----- multi-stage pipeline -------------------------------------------------

enum Carry {
    None,
    ChildStdout(ChildStdout),
    Buffer(Vec<u8>),
}

enum Stage {
    Done(i32),
    Process(Child),
}

fn run_multi_stage(
    commands: &[SimpleCommand],
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let mut resolved_stages: Vec<Option<ResolvedCommand>> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                resolved_stages.push(None);
            }
            SimpleCommand::Exec(exec) => match resolve(exec, shell) {
                Ok(r) => resolved_stages.push(Some(r)),
                Err(code) => return ExecOutcome::Continue(code),
            },
        }
    }
    let mut all_files: Vec<Option<StageFiles>> = Vec::with_capacity(resolved_stages.len());
    for r in &resolved_stages {
        match r {
            None => all_files.push(None),
            Some(r) => match open_stage_files(r) {
                Ok(f) => all_files.push(Some(f)),
                Err(()) => return ExecOutcome::Continue(1),
            },
        }
    }

    let n = resolved_stages.len();
    let mut stages: Vec<Stage> = Vec::with_capacity(n);
    let mut carry = Carry::None;

    for (i, (resolved, files)) in resolved_stages.iter().zip(all_files).enumerate() {
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

        let cmd = match resolved {
            Some(r) => r,
            None => {
                drop(incoming);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                let _ = files;
                continue;
            }
        };
        let files = files.expect("non-Assign stage must have StageFiles");

        if builtins::is_builtin(&cmd.program) {
            drop(incoming);

            if cmd.program == "cd" || cmd.program == "exit" {
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                continue;
            }

            let mut buffer: Vec<u8> = Vec::new();
            let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer, shell);
            let mut status = match outcome {
                ExecOutcome::Continue(code) => code,
                ExecOutcome::Exit(code) => code,
            };
            match files.stdout {
                Some(mut file) => {
                    if let Err(e) = file.write_all(&buffer) {
                        eprintln!("shuck: {}: {e}", cmd.program);
                        status = 1;
                    }
                    if !is_last {
                        carry = Carry::Buffer(Vec::new());
                    }
                }
                None => {
                    if is_last {
                        match sink {
                            StdoutSink::Terminal => {
                                if let Err(e) = io::stdout().write_all(&buffer) {
                                    eprintln!("shuck: {}: {e}", cmd.program);
                                    status = 1;
                                }
                            }
                            StdoutSink::Capture(buf) => {
                                buf.extend_from_slice(&buffer);
                            }
                        }
                    } else {
                        carry = Carry::Buffer(buffer);
                    }
                }
            }
            stages.push(Stage::Done(status));
            continue;
        }

        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);
        process.env_clear();
        process.envs(shell.exported_env());

        let mut pending_input: Option<Vec<u8>> = None;
        if let Some(file) = files.stdin {
            process.stdin(Stdio::from(file));
        } else {
            match incoming {
                Carry::None => {}
                Carry::ChildStdout(child_stdout) => {
                    process.stdin(Stdio::from(child_stdout));
                }
                Carry::Buffer(bytes) => {
                    process.stdin(Stdio::piped());
                    pending_input = Some(bytes);
                }
            }
        }

        let pipe_onward = !is_last && cmd.stdout.is_none();
        let want_terminal_capture =
            is_last && cmd.stdout.is_none() && matches!(sink, StdoutSink::Capture(_));
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward || want_terminal_capture {
            process.stdout(Stdio::piped());
        }

        if let Some(file) = files.stderr {
            process.stderr(Stdio::from(file));
        }

        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("shuck: command not found: {}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(127));
                continue;
            }
            Err(e) => {
                eprintln!("shuck: {}: {e}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(1));
                continue;
            }
        };

        if let Some(bytes) = pending_input {
            if let Some(mut child_stdin) = child.stdin.take() {
                let _ = child_stdin.write_all(&bytes);
            }
        }

        if pipe_onward {
            carry = Carry::ChildStdout(child.stdout.take().expect("stdout was set to piped"));
        } else if !is_last {
            carry = Carry::Buffer(Vec::new());
        } else if want_terminal_capture {
            if let StdoutSink::Capture(buf) = sink {
                if let Some(mut child_stdout) = child.stdout.take() {
                    let _ = io::copy(&mut child_stdout, *buf);
                }
            }
        }

        stages.push(Stage::Process(child));
    }

    let mut last_status = 0;
    for stage in stages {
        match stage {
            Stage::Done(code) => last_status = code,
            Stage::Process(mut child) => {
                last_status = match child.wait() {
                    Ok(status) => status_code(&status),
                    Err(e) => {
                        eprintln!("shuck: {e}");
                        1
                    }
                };
            }
        }
    }
    ExecOutcome::Continue(last_status)
}

// ----- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
    use crate::lexer::{Word, WordPart};

    fn lit_word(s: &str) -> Word {
        Word(vec![WordPart::Literal(s.to_string())])
    }

    fn exec(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            program: lit_word(program),
            args: args.iter().map(|a| lit_word(a)).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
        Sequence {
            first: Pipeline { commands: vec![cmd] },
            rest: vec![],
        }
    }

    #[test]
    fn execute_capturing_echo_returns_raw_output_with_newline() {
        // execute_capturing does NOT strip; that happens in expand::run_substitution.
        let seq = one_command_sequence(exec("echo", &["hi"]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "hi\n");
        assert_eq!(status, 0);
    }

    #[test]
    fn execute_capturing_exit_returns_status() {
        let seq = one_command_sequence(exec("exit", &["7"]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "");
        assert_eq!(status, 7);
    }

    #[test]
    fn execute_capturing_empty_echo() {
        let seq = one_command_sequence(exec("echo", &[]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "\n");
        assert_eq!(status, 0);
    }
}
```

- [ ] **Step 2: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 126 tests pass (123 existing + 3 new in `executor`).

- [ ] **Step 3: Commit**

```bash
git add src/executor.rs
git commit -m "feat: add execute_capturing for stdout-into-buffer execution"
```

---

## Task 2: AST + expand + run_substitution wiring

Land everything that depends on the AST gaining `WordPart::CommandSub`: the variant itself, the new `LexError` variants, the dynamic `lex_error_message`, `Shell: Clone`, the `&mut Shell` signature changes for `expand` and `expand_assignment`, the `run_substitution` helper, and the executor-side call-site updates. Refactor `try_split_assignment` to move-semantics so it doesn't need to clone `Sequence`. Tests use synthetic CommandSub Words — the lexer is unchanged in this task (it still can't produce CommandSubs).

**Files:**
- Modify: `src/shell_state.rs`
- Modify: `src/lexer.rs`
- Modify: `src/command.rs`
- Modify: `src/expand.rs`
- Modify: `src/executor.rs`
- Modify: `src/shell.rs`

- [ ] **Step 1: Update `src/shell_state.rs`** — add `Clone` to `Shell`.

Find the line:

```rust
#[derive(Debug)]
pub struct Shell {
```

Replace with:

```rust
#[derive(Debug, Clone)]
pub struct Shell {
```

- [ ] **Step 2: Update `src/lexer.rs`** — add the three new `LexError` variants and the new `WordPart::CommandSub` variant.

Replace this block at the top of `src/lexer.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
}
```

with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
}
```

Find the `WordPart` enum:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
}
```

Replace with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
}
```

- [ ] **Step 3: Update `src/command.rs`** — refactor `try_split_assignment` to take ownership, update `finalize_stage` accordingly.

Find the existing `try_split_assignment` (currently at the top of the file, ~lines 7–43):

```rust
fn try_split_assignment(word: &crate::lexer::Word) -> Option<(String, crate::lexer::Word)> {
    use crate::lexer::WordPart;
    let first = word.0.first()?;
    let text = match first {
        WordPart::Literal(s) => s,
        _ => return None,
    };
    let eq = text.find('=')?;
    let name = &text[..eq];
    if name.is_empty() {
        return None;
    }
    let mut name_chars = name.chars();
    let first_ch = name_chars.next()?;
    if !(first_ch == '_' || first_ch.is_ascii_alphabetic()) {
        return None;
    }
    if !name_chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    let rest_of_first = text[eq + 1..].to_string();
    let mut value_parts: Vec<WordPart> = Vec::with_capacity(word.0.len());
    value_parts.push(WordPart::Literal(rest_of_first));
    for part in word.0.iter().skip(1) {
        let cloned = match part {
            WordPart::Literal(s) => WordPart::Literal(s.clone()),
            WordPart::Var { name, quoted } => WordPart::Var {
                name: name.clone(),
                quoted: *quoted,
            },
            WordPart::LastStatus { quoted } => WordPart::LastStatus { quoted: *quoted },
            WordPart::Tilde => WordPart::Tilde,
        };
        value_parts.push(cloned);
    }
    Some((name.to_string(), crate::lexer::Word(value_parts)))
}
```

Replace with:

```rust
/// If `word` looks like `NAME=value` (a leading `Literal` whose text begins
/// with a valid identifier followed by `=`), returns `Ok((name, value))`
/// where `value` is a `Word` containing the rest of the prefix Literal
/// followed by the remaining original parts (moved, not cloned). Otherwise
/// returns `Err(word)` handing the original back unchanged.
fn try_split_assignment(
    word: crate::lexer::Word,
) -> Result<(String, crate::lexer::Word), crate::lexer::Word> {
    use crate::lexer::WordPart;
    let first = match word.0.first() {
        Some(p) => p,
        None => return Err(word),
    };
    let text = match first {
        WordPart::Literal(s) => s,
        _ => return Err(word),
    };
    let Some(eq) = text.find('=') else {
        return Err(word);
    };
    let name_slice = &text[..eq];
    if name_slice.is_empty() {
        return Err(word);
    }
    let mut name_chars = name_slice.chars();
    let Some(first_ch) = name_chars.next() else {
        return Err(word);
    };
    if !(first_ch == '_' || first_ch.is_ascii_alphabetic()) {
        return Err(word);
    }
    if !name_chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(word);
    }

    // Validation passed — destructure the word, moving parts into the value.
    let crate::lexer::Word(mut parts) = word;
    let first_part = parts.remove(0);
    let text = match first_part {
        WordPart::Literal(s) => s,
        _ => unreachable!("checked above"),
    };
    let (name, rest_of_first) = (text[..eq].to_string(), text[eq + 1..].to_string());
    let mut value_parts: Vec<WordPart> = Vec::with_capacity(parts.len() + 1);
    value_parts.push(WordPart::Literal(rest_of_first));
    value_parts.extend(parts);
    Ok((name, crate::lexer::Word(value_parts)))
}
```

Find the existing `finalize_stage`:

```rust
fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    stdin: Option<crate::lexer::Word>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
) -> SimpleCommand {
    if args.is_empty() && stdin.is_none() && stdout.is_none() && stderr.is_none() {
        if let Some((name, value)) = try_split_assignment(&program) {
            return SimpleCommand::Assign { name, value };
        }
    }
    SimpleCommand::Exec(ExecCommand {
        program,
        args,
        stdin,
        stdout,
        stderr,
    })
}
```

Replace with:

```rust
fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    stdin: Option<crate::lexer::Word>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
) -> SimpleCommand {
    if args.is_empty() && stdin.is_none() && stdout.is_none() && stderr.is_none() {
        match try_split_assignment(program) {
            Ok((name, value)) => return SimpleCommand::Assign { name, value },
            Err(restored) => {
                return SimpleCommand::Exec(ExecCommand {
                    program: restored,
                    args,
                    stdin,
                    stdout,
                    stderr,
                });
            }
        }
    }
    SimpleCommand::Exec(ExecCommand {
        program,
        args,
        stdin,
        stdout,
        stderr,
    })
}
```

- [ ] **Step 4: Update `src/expand.rs`** — change `expand` and `expand_assignment` to take `&mut Shell`, add the two new arms for `WordPart::CommandSub`, add `run_substitution` and `strip_trailing_newlines`.

Replace the entire `src/expand.rs` with:

```rust
use crate::command::Sequence;
use crate::executor;
use crate::lexer::{Word, WordPart};
use crate::shell_state::Shell;

/// Expands a `Word` against the current `Shell` state into 0 or more
/// argument strings. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
pub fn expand(word: &Word, shell: &mut Shell) -> Vec<String> {
    let mut current = String::new();
    let mut has_emitted = false;
    let mut result: Vec<String> = Vec::new();

    for part in &word.0 {
        match part {
            WordPart::Literal(s) => {
                current.push_str(s);
                has_emitted = true;
            }
            WordPart::Tilde => {
                if let Some(home) = shell.get("HOME") {
                    current.push_str(home);
                }
                has_emitted = true;
            }
            WordPart::Var { name, quoted: true } => {
                if let Some(value) = shell.get(name) {
                    current.push_str(value);
                }
                has_emitted = true;
            }
            WordPart::LastStatus { quoted: true } => {
                current.push_str(&shell.last_status().to_string());
                has_emitted = true;
            }
            WordPart::Var { name, quoted: false } => {
                let value = shell.get(name).map(|s| s.to_string()).unwrap_or_default();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::LastStatus { quoted: false } => {
                let value = shell.last_status().to_string();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::CommandSub { sequence, quoted: true } => {
                let output = run_substitution(sequence, shell);
                current.push_str(&output);
                has_emitted = true;
            }
            WordPart::CommandSub { sequence, quoted: false } => {
                let output = run_substitution(sequence, shell);
                emit_split(&output, &mut current, &mut result, &mut has_emitted);
            }
        }
    }

    if has_emitted {
        result.push(current);
    }
    result
}

/// Expands a `Word` for assignment context: word-splitting is suppressed and
/// the result is one string. Each `Var`/`LastStatus`/`CommandSub` part
/// contributes its value verbatim regardless of the `quoted` flag — matching
/// bash, which disables splitting on the right-hand side of `NAME=...`.
pub fn expand_assignment(word: &Word, shell: &mut Shell) -> String {
    let mut result = String::new();
    for part in &word.0 {
        match part {
            WordPart::Literal(s) => result.push_str(s),
            WordPart::Tilde => {
                if let Some(home) = shell.get("HOME") {
                    result.push_str(home);
                }
            }
            WordPart::Var { name, .. } => {
                if let Some(value) = shell.get(name) {
                    result.push_str(value);
                }
            }
            WordPart::LastStatus { .. } => {
                result.push_str(&shell.last_status().to_string());
            }
            WordPart::CommandSub { sequence, .. } => {
                result.push_str(&run_substitution(sequence, shell));
            }
        }
    }
    result
}

/// Runs a sub-sequence as a substituted command: clones the parent `Shell`
/// (so state mutations don't leak), captures stdout via the executor's
/// `execute_capturing`, strips trailing newlines, and propagates the
/// substituted command's exit status into the parent shell's `$?`.
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    strip_trailing_newlines(&output)
}

fn strip_trailing_newlines(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

fn emit_split(
    value: &str,
    current: &mut String,
    result: &mut Vec<String>,
    has_emitted: &mut bool,
) {
    let fields: Vec<&str> = value.split_ascii_whitespace().collect();
    match fields.len() {
        0 => {}
        1 => {
            current.push_str(fields[0]);
            *has_emitted = true;
        }
        _ => {
            current.push_str(fields[0]);
            result.push(std::mem::take(current));
            for f in &fields[1..fields.len() - 1] {
                result.push((*f).to_string());
            }
            *current = fields[fields.len() - 1].to_string();
            *has_emitted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ExecCommand, Pipeline, SimpleCommand};

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal(s.to_string())])
    }

    fn var_unq(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: false }])
    }
    fn var_q(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: true }])
    }

    /// Builds a synthetic Sequence for `echo <args>` — used to drive
    /// CommandSub expansion in unit tests without invoking the lexer.
    fn echo_sequence(args: &[&str]) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        }
    }

    #[test]
    fn expand_literal_word() {
        let mut shell = Shell::new();
        assert_eq!(expand(&lit("hello"), &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(expand(&lit(""), &mut shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal("foo".to_string()),
            WordPart::Literal("bar".to_string()),
        ]);
        assert_eq!(expand(&word, &mut shell), vec!["foobar".to_string()]);
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let mut shell = Shell::new();
        assert!(expand(&var_unq("DEFINITELY_NOT_SET_XYZ"), &mut shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(
            expand(&var_q("DEFINITELY_NOT_SET_XYZ"), &mut shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(expand(&var_q("SHUCK_T"), &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(
            expand(&var_unq("SHUCK_T"), &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_unquoted_var_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "x y".to_string());
        let word = Word(vec![
            WordPart::Literal("a".to_string()),
            WordPart::Var { name: "SHUCK_T".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand(&word, &mut shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand(&word, &mut shell), vec!["42".to_string()]);
    }

    #[test]
    fn expand_tilde_uses_home() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/tmp/shuck_test".to_string());
        let word = Word(vec![
            WordPart::Tilde,
            WordPart::Literal("/foo".to_string()),
        ]);
        assert_eq!(
            expand(&word, &mut shell),
            vec!["/tmp/shuck_test/foo".to_string()]
        );
    }

    #[test]
    fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
        let mut shell = Shell::new();
        assert_eq!(expand(&Word(vec![WordPart::Var {
            name: "DEFINITELY_NOT_SET_REDIR".to_string(),
            quoted: false,
        }]), &mut shell).len(), 0);
    }

    #[test]
    fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T_TWOFIELD", "a b".to_string());
        assert_eq!(expand(&Word(vec![WordPart::Var {
            name: "SHUCK_T_TWOFIELD".to_string(),
            quoted: false,
        }]), &mut shell).len(), 2);
    }

    #[test]
    fn expand_assignment_preserves_interior_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T_PAD", "a  b".to_string());
        let word = Word(vec![WordPart::Var {
            name: "SHUCK_T_PAD".to_string(),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a  b".to_string());
    }

    #[test]
    fn expand_assignment_concatenates_parts() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T_X", "x".to_string());
        let word = Word(vec![
            WordPart::Literal("pre-".to_string()),
            WordPart::Var { name: "SHUCK_T_X".to_string(), quoted: false },
            WordPart::Literal("-post".to_string()),
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "pre-x-post".to_string());
    }

    #[test]
    fn expand_assignment_unset_var_yields_empty_segment() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal("[".to_string()),
            WordPart::Var { name: "DEFINITELY_NOT_SET_ASN".to_string(), quoted: false },
            WordPart::Literal("]".to_string()),
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "[]".to_string());
    }

    // ---- CommandSub tests --------------------------------------------------

    #[test]
    fn expand_command_sub_invokes_inner_echo() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hello"]),
            quoted: false,
        }]);
        assert_eq!(expand(&word, &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_command_sub_unquoted_splits() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(
            expand(&word, &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: true,
        }]);
        assert_eq!(expand(&word, &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_command_sub_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal("pre".to_string()),
            WordPart::CommandSub {
                sequence: echo_sequence(&["x", "y"]),
                quoted: false,
            },
        ]);
        assert_eq!(
            expand(&word, &mut shell),
            vec!["prex".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_strips_trailing_newlines() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hi"]),
            quoted: true,
        }]);
        // echo emits "hi\n"; run_substitution strips → "hi" exactly.
        assert_eq!(expand(&word, &mut shell), vec!["hi".to_string()]);
    }

    #[test]
    fn expand_command_sub_updates_parent_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(0);
        let word = Word(vec![WordPart::CommandSub {
            sequence: exit_sequence(7),
            quoted: true,
        }]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_assignment_command_sub_concatenates_verbatim() {
        // expand_assignment suppresses splitting, so `FOO=$(echo a b)` stores
        // "a b" (one space) as the value — same as bash's IFS=behavior in
        // assignment context. (echo's argument joining already produces "a b"
        // with one space.)
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a b".to_string());
    }
}
```

- [ ] **Step 5: Update `src/executor.rs`** to match the new `&mut Shell` signature of `expand_assignment`.

After Task 1, `run_single`'s Assign arm reads:

```rust
        SimpleCommand::Assign { name, value } => {
            shell.set(name, expand_assignment(value, shell));
            ExecOutcome::Continue(0)
        }
```

`expand_assignment` now takes `&mut Shell`. With `shell` borrowed mutably for `expand_assignment` AND `shell.set`, the borrow checker rejects the single-expression form. Fix by binding the value first:

```rust
        SimpleCommand::Assign { name, value } => {
            let v = expand_assignment(value, shell);
            shell.set(name, v);
            ExecOutcome::Continue(0)
        }
```

`resolve(cmd, shell)`, `expand_single(word, shell)`, and `run_subprocess(cmd, files, shell, sink)` already pass `&mut Shell` (resolve and expand_single take `&mut Shell` from Task 1; run_subprocess still takes `&Shell` and continues to — it doesn't call `expand`). No other call-site changes needed in this file.

- [ ] **Step 6: Update `src/shell.rs`** — change `lex_error_message` to return `String` and handle the 3 new variants.

Replace the existing `lex_error_message`:

```rust
fn lex_error_message(error: LexError) -> &'static str {
    match error {
        LexError::UnterminatedQuote => "unterminated quote",
        LexError::BareAmpersand => "unexpected '&'",
        LexError::InvalidVarName => "invalid variable name in '${...}'",
        LexError::UnterminatedBrace => "unterminated '${...}'",
    }
}
```

with:

```rust
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => "unterminated quote".to_string(),
        LexError::BareAmpersand => "unexpected '&'".to_string(),
        LexError::InvalidVarName => "invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => "unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => "unterminated command substitution".to_string(),
        LexError::SubstitutionLexError(inner) => {
            format!("in command substitution: {}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!("in command substitution: {}", parse_error_message(inner))
        }
    }
}
```

The single call site in `process_line` already uses `{}` formatting and will work with `String` unchanged.

`parse_error_message` currently takes `ParseError` by value. The new `SubstitutionParseError(inner)` arm above also passes by value — that compiles. No change to `parse_error_message` needed.

- [ ] **Step 7: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 133 tests pass (126 from Task 1 + 7 new CommandSub tests in `expand`).

- [ ] **Step 8: Commit**

```bash
git add src/shell_state.rs src/lexer.rs src/command.rs src/expand.rs src/executor.rs src/shell.rs
git commit -m "feat: WordPart::CommandSub + expand-time substitution wiring"
```

---

## Task 3: Lexer recognizes `$(...)`

Add the lexer support for `$(...)` outside any quote and inside double quotes. The body is scanned with paren-balancing, then recursively tokenized and parsed. Errors are wrapped in the new `LexError` variants.

**Files:**
- Modify: `src/lexer.rs`

- [ ] **Step 1: Update `src/lexer.rs`** — add the substitution scanner, hook it into `read_dollar_expansion`, and add lexer tests.

Find the existing `read_dollar_expansion`:

```rust
fn read_dollar_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('{') => {
            chars.next();
            let name = read_braced_var_name(chars)?;
            parts.push(WordPart::Var { name, quoted });
        }
        Some('?') => {
            chars.next();
            parts.push(WordPart::LastStatus { quoted });
        }
        Some(c) if is_name_start(c) => {
            let name = read_var_name(chars);
            parts.push(WordPart::Var { name, quoted });
        }
        _ => {
            parts.push(WordPart::Literal("$".to_string()));
        }
    }
    Ok(())
}
```

Replace with:

```rust
fn read_dollar_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('(') => {
            chars.next(); // consume '('
            let sequence = scan_paren_substitution(chars)?;
            parts.push(WordPart::CommandSub { sequence, quoted });
        }
        Some('{') => {
            chars.next();
            let name = read_braced_var_name(chars)?;
            parts.push(WordPart::Var { name, quoted });
        }
        Some('?') => {
            chars.next();
            parts.push(WordPart::LastStatus { quoted });
        }
        Some(c) if is_name_start(c) => {
            let name = read_var_name(chars);
            parts.push(WordPart::Var { name, quoted });
        }
        _ => {
            parts.push(WordPart::Literal("$".to_string()));
        }
    }
    Ok(())
}

/// Reads the body of a `$(...)` substitution. The opening `$(` is already
/// consumed; this function consumes through the matching `)` at depth 0.
/// Tracks quote and escape state so that `)` inside `'...'`, `"..."`, or
/// after `\` does not close the substitution, and nested `$(...)` increments
/// the depth.
fn scan_paren_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    let mut depth: usize = 0;
    while let Some(c) = chars.next() {
        match c {
            ')' if depth == 0 => {
                return parse_substitution_body(&body);
            }
            ')' => {
                depth -= 1;
                body.push(c);
            }
            '(' => {
                // Bare `(` is just a character. shuck has no subshell
                // `(cmd)` syntax — only `$(` increments depth (handled in
                // the `$` arm below).
                body.push(c);
            }
            '\\' => {
                body.push(c);
                if let Some(next) = chars.next() {
                    body.push(next);
                } else {
                    return Err(LexError::UnterminatedSubstitution);
                }
            }
            '\'' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('\'') => {
                            body.push('\'');
                            break;
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '"' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('"') => {
                            body.push('"');
                            break;
                        }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(next) = chars.next() {
                                body.push(next);
                            } else {
                                return Err(LexError::UnterminatedSubstitution);
                            }
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '$' => {
                body.push(c);
                if let Some(&next) = chars.peek() {
                    if next == '(' {
                        chars.next();
                        body.push('(');
                        depth += 1;
                    }
                }
            }
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}

/// Tokenizes and parses a substitution body, wrapping any errors with the
/// substitution-context `LexError` variants. Empty bodies (whitespace only)
/// produce an empty `Sequence`.
fn parse_substitution_body(body: &str) -> Result<crate::command::Sequence, LexError> {
    let tokens = tokenize(body).map_err(|e| LexError::SubstitutionLexError(Box::new(e)))?;
    let parsed = crate::command::parse(tokens).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}

fn empty_sequence() -> crate::command::Sequence {
    crate::command::Sequence {
        first: crate::command::Pipeline { commands: Vec::new() },
        rest: Vec::new(),
    }
}
```

- [ ] **Step 2: Add lexer tests for `$(...)`** at the bottom of the `mod tests` block in `src/lexer.rs`:

```rust
    fn sub_word(parts: Vec<WordPart>) -> Token {
        Token::Word(Word(parts))
    }

    fn echo_seq(args: &[&str]) -> crate::command::Sequence {
        use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: Word(vec![WordPart::Literal("echo".to_string())]),
                    args: args
                        .iter()
                        .map(|a| Word(vec![WordPart::Literal(a.to_string())]))
                        .collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        }
    }

    #[test]
    fn tokenize_command_sub_basic() {
        assert_eq!(
            tokenize("$(echo hi)").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_quoted_in_double_quotes() {
        assert_eq!(
            tokenize("\"$(echo hi)\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'$(echo hi)'").unwrap(),
            words(&["$(echo hi)"])
        );
    }

    #[test]
    fn tokenize_command_sub_empty() {
        assert_eq!(
            tokenize("$()").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: crate::command::Sequence {
                    first: crate::command::Pipeline { commands: vec![] },
                    rest: vec![],
                },
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_with_paren_inside_double_quotes() {
        // The `)` inside `"..."` does not close the substitution.
        assert_eq!(
            tokenize("$(echo \")\")").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&[")"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_nested() {
        // Outer body is `echo $(echo hi)`; inner is `echo hi`.
        let inner = echo_seq(&["hi"]);
        let inner_word = Word(vec![WordPart::CommandSub {
            sequence: inner,
            quoted: false,
        }]);
        let outer = {
            use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
            Sequence {
                first: Pipeline {
                    commands: vec![SimpleCommand::Exec(ExecCommand {
                        program: Word(vec![WordPart::Literal("echo".to_string())]),
                        args: vec![inner_word],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                    })],
                },
                rest: vec![],
            }
        };
        assert_eq!(
            tokenize("$(echo $(echo hi))").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: outer,
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_unterminated() {
        assert_eq!(
            tokenize("$(echo").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_command_sub_inner_lex_error() {
        // `${1foo}` inside a substitution → InvalidVarName, wrapped.
        let err = tokenize("$(echo ${1foo})").unwrap_err();
        match err {
            LexError::SubstitutionLexError(inner) => {
                assert_eq!(*inner, LexError::InvalidVarName);
            }
            other => panic!("expected SubstitutionLexError, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_inner_parse_error() {
        // `echo |` inside the body → MissingCommand from the parser, wrapped.
        let err = tokenize("$(echo |)").unwrap_err();
        match err {
            LexError::SubstitutionParseError(inner) => {
                assert_eq!(inner, crate::command::ParseError::MissingCommand);
            }
            other => panic!("expected SubstitutionParseError, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_as_program() {
        // `$(echo ls) -la` — the program word is itself a CommandSub.
        let tokens = tokenize("$(echo ls) -la").unwrap();
        assert_eq!(tokens.len(), 2);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
        assert_eq!(tokens[1], w("-la"));
    }

    #[test]
    fn tokenize_command_sub_concatenates_with_literal() {
        // `pre$(echo x)post` → one Word with three parts: Literal, CommandSub, Literal
        let tokens = tokenize("pre$(echo x)post").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], WordPart::Literal(ref s) if s == "pre"));
                assert!(matches!(parts[1], WordPart::CommandSub { .. }));
                assert!(matches!(parts[2], WordPart::Literal(ref s) if s == "post"));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_in_redirect_target() {
        let tokens = tokenize("cat > $(echo /tmp/f)").unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], w("cat"));
        assert_eq!(tokens[1], Token::Op(Operator::RedirOut));
        match &tokens[2] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }
```

- [ ] **Step 3: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 145 tests pass (133 from Task 2 + 12 new lexer tests).

- [ ] **Step 4: Smoke test** (no commit yet — Task 5 is the final smoke test)

```bash
cargo build -q
printf '%s\n' \
  'echo $(echo hello)' \
  'echo "$(echo a b)"' \
  'echo $(echo a b)' \
  'FOO=$(echo bar); echo $FOO' \
  'echo $(echo $(echo nested))' \
  'false; X=$(true); echo $?' \
  'FOO=outer; X=$(FOO=inner; echo $FOO); echo $FOO/$X' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output:
```
hello
a b
a b
bar
nested
0
outer/inner
```

If any line differs, stop and investigate before committing.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat: lexer recognizes \$(...) command substitution"
```

---

## Task 4: Lexer recognizes backtick substitution

Add the backtick form. The scanner uses bash's backtick-specific escape rules: only `\\``, `\\\\`, and `\\$` are special; every other `\\x` is preserved verbatim. After scanning, the body is reused via `parse_substitution_body` (defined in Task 3).

**Files:**
- Modify: `src/lexer.rs`

- [ ] **Step 1: Update `src/lexer.rs`** — add the backtick scanner and hook it into the tokenize loop in both contexts.

**Add this helper** in `src/lexer.rs` immediately after `scan_paren_substitution` (or anywhere below `tokenize`):

```rust
/// Reads the body of a `` `...` `` substitution. The opening backtick is
/// already consumed; this function consumes through the matching unescaped
/// backtick. Applies bash's backtick escape rules:
/// - `\` + `` ` `` -> literal `` ` `` in the body
/// - `\` + `\` -> literal `\` in the body
/// - `\` + `$` -> literal `$` in the body
/// - `\` + any other char `c` -> both `\` and `c` are preserved verbatim
fn scan_backtick_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                return parse_substitution_body(&body);
            }
            '\\' => match chars.next() {
                Some('`') => body.push('`'),
                Some('\\') => body.push('\\'),
                Some('$') => body.push('$'),
                Some(other) => {
                    body.push('\\');
                    body.push(other);
                }
                None => return Err(LexError::UnterminatedSubstitution),
            },
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}
```

**Add a backtick arm in the outer `tokenize` loop.** Find the existing outer match (in `tokenize`'s `while let Some(c) = chars.next()` loop) — specifically the `'~'` arm:

```rust
            '~' if !has_token && tilde_at_word_start(&chars) => {
                has_token = true;
                parts.push(WordPart::Tilde);
            }
```

Immediately AFTER this arm (and before the `'|'` arm), insert:

```rust
            '`' => {
                has_token = true;
                if !current.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut current)));
                }
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
```

**Add a backtick arm inside the double-quote loop.** Find the existing `'"'` arm body in `tokenize`. Inside that loop the cases handled are `Some('"')` (close), `Some('\\')` (escape), `Some('$')` (expansion), then a catch-all. Add a new case for `` ` `` between `Some('$')` and the catch-all. Find:

```rust
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            if !current.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut current)));
                            }
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
                        Some(ch) => current.push(ch),
```

Replace with:

```rust
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            if !current.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut current)));
                            }
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
                        Some('`') => {
                            // Backtick substitution inside double quotes (quoted: true).
                            if !current.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut current)));
                            }
                            let sequence = scan_backtick_substitution(&mut chars)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => current.push(ch),
```

- [ ] **Step 2: Add backtick tests** at the bottom of `mod tests`:

```rust
    #[test]
    fn tokenize_backtick_basic() {
        assert_eq!(
            tokenize("`echo hi`").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_in_double_quotes_is_quoted() {
        assert_eq!(
            tokenize("\"`echo hi`\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_escape_dollar() {
        // `\$FOO` inside backticks → inner body is `$FOO` (the `\$` unescapes
        // before the inner tokenizer sees it). So the inner Sequence has a
        // single command whose first arg expands $FOO.
        let tokens = tokenize("`echo \\$FOO`").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    WordPart::CommandSub { sequence, quoted: false } => {
                        // Inner: echo $FOO → second word's first part is a Var
                        let inner_cmd = &sequence.first.commands[0];
                        match inner_cmd {
                            crate::command::SimpleCommand::Exec(e) => {
                                assert_eq!(e.args.len(), 1);
                                match &e.args[0].0[0] {
                                    WordPart::Var { name, quoted: false } => {
                                        assert_eq!(name, "FOO");
                                    }
                                    other => panic!("expected Var(FOO), got {other:?}"),
                                }
                            }
                            other => panic!("expected Exec, got {other:?}"),
                        }
                    }
                    other => panic!("expected CommandSub, got {other:?}"),
                }
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_escape_backslash() {
        // `\\` inside backticks → inner body is `\`. Inner tokenize sees
        // a trailing backslash; treats it as a literal.
        let tokens = tokenize("`echo \\\\`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    match &sequence.first.commands[0] {
                        crate::command::SimpleCommand::Exec(e) => {
                            // Inner body was `echo \` — backslash at end is literal.
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal(s) => assert_eq!(s, "\\"),
                                other => panic!("expected Literal(\\\\), got {other:?}"),
                            }
                        }
                        other => panic!("expected Exec, got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unescaped_other_backslash_preserved() {
        // `\n` inside backticks → body has `\n` (backslash + n), which the
        // inner tokenize treats as an escape (literal `n`).
        let tokens = tokenize("`echo \\n`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    match &sequence.first.commands[0] {
                        crate::command::SimpleCommand::Exec(e) => {
                            // Inner body `echo \n` — outer tokenizer's `\n` becomes `n`
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal(s) => assert_eq!(s, "n"),
                                other => panic!("expected Literal(n), got {other:?}"),
                            }
                        }
                        other => panic!("expected Exec, got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unterminated() {
        assert_eq!(
            tokenize("`echo hi").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_backtick_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'`echo hi`'").unwrap(),
            words(&["`echo hi`"])
        );
    }
```

- [ ] **Step 3: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 152 tests pass (145 from Task 3 + 7 new backtick tests).

- [ ] **Step 4: Smoke test** (no commit yet — Task 5 is the final smoke test)

```bash
cargo build -q
printf '%s\n' \
  'echo `echo via-backtick`' \
  'echo `echo a b`' \
  'echo "`echo a b`"' \
  'X=`echo hello`; echo $X' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output:
```
via-backtick
a b
a b
hello
```

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat: lexer recognizes backtick command substitution"
```

---

## Task 5: Full smoke test

End-to-end verification of `$(...)` and `` `...` `` combined with v1–v4 features. This task is verification only — no commit.

**Files:** none

- [ ] **Step 1: Run the combined smoke script**

```bash
cargo build -q
printf '%s\n' \
  'echo $(echo hello)' \
  'echo "$(echo a b)"' \
  'echo $(echo a b)' \
  'FOO=$(echo bar); echo $FOO' \
  'PAD=$(echo "a  b"); echo "$PAD"' \
  'echo `echo via-backtick`' \
  'echo $(echo $(echo nested))' \
  'echo `echo \`nested-backtick\``' \
  'false; X=$(true); echo $?' \
  'echo $(false); echo $?' \
  'FOO=outer; X=$(FOO=inner; echo $FOO); echo $FOO/$X' \
  'echo "prefix $(echo a b) suffix"' \
  'echo pre$(echo x)post' \
  'echo done' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output (line-for-line; subprocess output blends in):
```
hello
a b
a b
bar
a  b
via-backtick
nested
nested-backtick
0

1
outer/inner
prefix a b suffix
prexpost
done
```

Notes on the expected output:
- Line 5 (`a  b`): two spaces preserved because `expand_assignment` doesn't word-split.
- Line 9 (`0`): `false` sets `$? = 1`, then the substitution running `true` resets it to 0.
- Line 10 (blank): `$(false)` captures empty stdout; `echo` prints just a newline.
- Line 11 (`1`): `$?` from the prior `$(false)` is 1.
- Line 12 (`outer/inner`): subshell isolation — the assignment inside `$(...)` doesn't leak.
- Line 14 (`prexpost`): single Word with three parts; substitution returns `x`, concatenated.

- [ ] **Step 2: Verify error paths**

```bash
printf 'echo $(\n'             | ./target/debug/shuck
printf 'echo `echo\n'          | ./target/debug/shuck
printf 'echo $(echo |)\n'      | ./target/debug/shuck
printf 'echo $(echo ${1foo})\n' | ./target/debug/shuck
```

Expected (each on its own invocation):
```
shuck: syntax error: unterminated command substitution
shuck: syntax error: unterminated command substitution
shuck: syntax error: in command substitution: expected a command
shuck: syntax error: in command substitution: invalid variable name in '${...}'
```

- [ ] **Step 3: Confirm**

All output matches. If any line differs, stop and fix the relevant module before completing the plan.

---
