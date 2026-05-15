# shuck Variables and Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add shell variables, `$VAR` / `${VAR}` / `$?` / `~` expansion, `export` and `unset` builtins, and bash-style unquoted word splitting to `shuck`.

**Architecture:** A new `Shell` struct holds all shell state (variables with exported flags, last status). `Token::Word(String)` becomes `Token::Word(Word)` where `Word` is a `Vec<WordPart>` carrying `Literal`/`Var`/`LastStatus`/`Tilde` parts with per-expansion quoting metadata. A new `expand` module turns `(Word, &Shell)` into `Vec<String>` (0+ args, with whitespace splitting on unquoted expansions). The executor takes `&mut Shell`, expands each word per command (so `FOO=bar; echo $FOO` sees live state), and spawns children with `env_clear().envs(shell.exported_env())`.

**Tech Stack:** Rust (edition 2024). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-15-shuck-variables-expansion-design.md`

---

## File Structure

| File | Change |
|------|--------|
| `src/shell_state.rs` | **New.** `Shell` struct (vars HashMap with `exported` flag, last_status), `Shell::new()` seeded from `std::env::vars()`. Unit tests. |
| `src/main.rs` | Add `mod shell_state;`. |
| `src/lexer.rs` | New `Word` / `WordPart` types. `Token::Word(Word)`. State machine recognizes `$VAR` / `${VAR}` / `$?` / `~`. New `LexError::InvalidVarName` and `UnterminatedBrace`. Tests rewritten + new tests for expansion lexing. |
| `src/expand.rs` | **New.** `expand(&Word, &Shell) -> Vec<String>` with the full word-splitting algorithm. Unit tests. |
| `src/command.rs` | `Redirect`/`ExecCommand` hold `Word` instead of `String`. New `SimpleCommand::{Assign, Exec}`. Parser detects assignment. Tests rewritten + new tests for assignment. |
| `src/executor.rs` | Takes `&mut Shell`. Resolves Words via `expand` per command. Handles `SimpleCommand::Assign`. Spawns children with `env_clear().envs(shell.exported_env())`. |
| `src/builtins.rs` | `run_builtin` gains `&mut Shell`. `cd` reads `HOME` via `shell.get`. New `export` and `unset` builtins + their tests. |
| `src/shell.rs` | `process_line` takes `&mut Shell`. `run` constructs `Shell::new()`; last_status lives on it. New `LexError` variants surfaced via `lex_error_message`. |

**Why the task order:** This is a large multi-file change. Task 1 introduces `Shell` and threads it everywhere with no behavioral change. Task 2 lands the type migration (structured `Word`, `SimpleCommand`, `expand` module, executor uses `expand`) — the lexer still emits only `Literal` parts and the parser still produces only `Exec`, but the new architecture is in place. Task 3 turns on lexer recognition of `$`/`~` and the new lex errors — expansion starts working for `Exec` commands. Task 4 adds parser-side assignment detection and the executor's `Assign` handling — `FOO=bar; echo $FOO` works end-to-end. Task 5 adds `export`/`unset`. Task 6 is comprehensive smoke verification. Each task leaves the crate compiling and all unit tests green. Per-task verification is `cargo test` (binary-only crate — never `cargo test --lib`).

---

## Task 1: Shell state and threading

Introduce a `Shell` struct that owns all shell variables (each marked `exported` or not) and the last exit status. Thread `&mut Shell` through the executor and builtins. `cd` switches from `env::var("HOME")` to `shell.get("HOME")`. Subprocess spawning becomes `env_clear().envs(shell.exported_env())`. No user-visible behavior changes (the initial `Shell` is seeded from the inherited process env, so children see the same env as before).

**Files:**
- Create: `src/shell_state.rs`
- Modify: `src/main.rs`, `src/shell.rs`, `src/executor.rs`, `src/builtins.rs`

- [ ] **Step 1: Create `src/shell_state.rs` with this content**

```rust
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct Variable {
    value: String,
    exported: bool,
}

/// Per-session shell state: variables (each either exported or not) and the
/// last command's exit status. The initial set of variables is seeded from
/// the process environment shuck inherited at startup, every one marked
/// exported.
#[derive(Debug)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable { value, exported: true });
        }
        Self { vars, last_status: 0 }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.value.as_str())
    }

    /// Sets a variable's value, preserving its existing `exported` flag (or
    /// creating it as unexported if it didn't exist).
    pub fn set(&mut self, name: &str, value: String) {
        match self.vars.get_mut(name) {
            Some(existing) => existing.value = value,
            None => {
                self.vars.insert(name.to_string(), Variable { value, exported: false });
            }
        }
    }

    /// Marks an existing variable as exported. If it doesn't exist, creates
    /// it with an empty value, already exported.
    pub fn export(&mut self, name: &str) {
        self.vars
            .entry(name.to_string())
            .and_modify(|v| v.exported = true)
            .or_insert_with(|| Variable {
                value: String::new(),
                exported: true,
            });
    }

    /// Sets a variable's value AND marks it exported.
    pub fn export_set(&mut self, name: &str, value: String) {
        self.vars.insert(
            name.to_string(),
            Variable { value, exported: true },
        );
    }

    pub fn unset(&mut self, name: &str) {
        self.vars.remove(name);
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    /// Iterates only the exported variables, suitable for passing to a child
    /// process's `Command::envs`.
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.as_str()))
    }
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_captures_inherited_env_as_exported() {
        let shell = Shell::new();
        // PATH is reliably present in test environments.
        assert!(shell.get("PATH").is_some(), "PATH should be inherited");
        let path_exported = shell.exported_env().any(|(k, _)| k == "PATH");
        assert!(path_exported);
    }

    #[test]
    fn set_creates_unexported_var() {
        let mut shell = Shell::new();
        shell.set("SHUCK_TEST_SET", "value".to_string());
        assert_eq!(shell.get("SHUCK_TEST_SET"), Some("value"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_SET");
        assert!(!in_exported);
    }

    #[test]
    fn set_preserves_existing_exported_flag() {
        let mut shell = Shell::new();
        shell.export_set("SHUCK_TEST_KEEP", "v1".to_string());
        shell.set("SHUCK_TEST_KEEP", "v2".to_string());
        assert_eq!(shell.get("SHUCK_TEST_KEEP"), Some("v2"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_KEEP");
        assert!(in_exported);
    }

    #[test]
    fn export_marks_existing_exported() {
        let mut shell = Shell::new();
        shell.set("SHUCK_TEST_EX", "value".to_string());
        shell.export("SHUCK_TEST_EX");
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_EX");
        assert!(in_exported);
    }

    #[test]
    fn export_creates_empty_when_missing() {
        let mut shell = Shell::new();
        shell.export("SHUCK_TEST_EMPTY");
        assert_eq!(shell.get("SHUCK_TEST_EMPTY"), Some(""));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_EMPTY");
        assert!(in_exported);
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("SHUCK_TEST_REMOVE", "v".to_string());
        shell.unset("SHUCK_TEST_REMOVE");
        assert_eq!(shell.get("SHUCK_TEST_REMOVE"), None);
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_REMOVE");
        assert!(!in_exported);
    }

    #[test]
    fn last_status_round_trip() {
        let mut shell = Shell::new();
        assert_eq!(shell.last_status(), 0);
        shell.set_last_status(42);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn exported_env_excludes_unexported() {
        let mut shell = Shell::new();
        shell.set("SHUCK_TEST_HIDDEN", "v".to_string());
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_TEST_HIDDEN");
        assert!(!in_exported);
    }
}
```

- [ ] **Step 2: Add the module to `src/main.rs`**

Replace the entire file with:

```rust
mod builtins;
mod command;
mod executor;
mod expand;
mod lexer;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
```

(`mod expand;` is here too in preparation for Task 2 — but the `src/expand.rs` file doesn't exist yet, which would be a compile error. Until Task 2 creates `src/expand.rs`, **leave that line out** and use the version below instead. Replace the entire `src/main.rs` with this for Task 1:)

```rust
mod builtins;
mod command;
mod executor;
mod lexer;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
```

- [ ] **Step 3: Update `src/shell.rs`**

**Edit 1 — add the import.** After the existing `use crate::lexer::{self, LexError};` line, add:

```rust
use crate::shell_state::Shell;
```

**Edit 2 — replace the local last_status with a Shell.** In `pub fn run()`, replace this block:

```rust
    // Tracks the exit status of the last command, so Ctrl-D (EOF) exits with
    // it — standard shell behavior, and the consumer of `ExecOutcome::Continue`'s
    // status. Room to grow into `$?` later.
    let mut last_status: i32 = 0;

    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => last_status = status,
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return last_status,
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
}
```

with:

```rust
    let mut shell = Shell::new();

    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line, &mut shell) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return shell.last_status(),
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
}
```

**Edit 3 — process_line signature and executor call.** Replace:

```rust
/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str) -> ExecOutcome {
```

with:

```rust
/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
```

And replace:

```rust
        Ok(Some(sequence)) => executor::execute(&sequence),
```

with:

```rust
        Ok(Some(sequence)) => executor::execute(&sequence, shell),
```

- [ ] **Step 4: Replace `src/executor.rs` entirely with this**

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Connector, Pipeline, Redirect, Sequence};
use crate::shell_state::Shell;

pub fn execute(seq: &Sequence, shell: &mut Shell) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first, shell);
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
            status = run_pipeline(pipeline, shell);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0], shell)
    } else {
        run_multi_stage(&pipeline.commands, shell)
    }
}

// ----- redirect file handling -----------------------------------------------

struct StageFiles {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

fn open_stage_files(cmd: &Command) -> Result<StageFiles, ()> {
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
        Some(redirect) => match open_output(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", redirect_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(redirect) => match open_output(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", redirect_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    Ok(StageFiles { stdin, stdout, stderr })
}

fn open_output(redirect: &Redirect) -> io::Result<File> {
    match redirect {
        Redirect::Truncate(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path),
        Redirect::Append(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(path),
    }
}

fn redirect_path(redirect: &Redirect) -> &str {
    match redirect {
        Redirect::Truncate(path) | Redirect::Append(path) => path,
    }
}

fn status_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1))
}

// ----- single command -------------------------------------------------------

fn run_single(cmd: &Command, shell: &mut Shell) -> ExecOutcome {
    let files = match open_stage_files(cmd) {
        Ok(files) => files,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&cmd.program) {
        match files.stdout {
            Some(mut file) => builtins::run_builtin(&cmd.program, &cmd.args, &mut file, shell),
            None => {
                let mut out = io::stdout();
                builtins::run_builtin(&cmd.program, &cmd.args, &mut out, shell)
            }
        }
    } else {
        run_subprocess(cmd, files, shell)
    }
}

fn run_subprocess(cmd: &Command, files: StageFiles, shell: &Shell) -> ExecOutcome {
    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());
    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    match process.status() {
        Ok(status) => ExecOutcome::Continue(status_code(&status)),
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

fn run_multi_stage(commands: &[Command], shell: &mut Shell) -> ExecOutcome {
    let mut all_files: Vec<StageFiles> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match open_stage_files(cmd) {
            Ok(files) => all_files.push(files),
            Err(()) => return ExecOutcome::Continue(1),
        }
    }

    let n = commands.len();
    let mut stages: Vec<Stage> = Vec::with_capacity(n);
    let mut carry = Carry::None;

    for (i, (cmd, files)) in commands.iter().zip(all_files).enumerate() {
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

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
                        if let Err(e) = io::stdout().write_all(&buffer) {
                            eprintln!("shuck: {}: {e}", cmd.program);
                            status = 1;
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
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward {
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
```

- [ ] **Step 5: Replace `src/builtins.rs` entirely with this**

```rust
use std::env;
use std::io::Write;
use std::path::Path;

use crate::shell_state::Shell;

#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo")
}

pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                eprintln!("shuck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    match env::set_current_dir(Path::new(&target)) {
        Ok(()) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: cd: {target}: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_pwd(out: &mut dyn Write) -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            if let Err(e) = writeln!(out, "{}", path.display()) {
                eprintln!("shuck: pwd: {e}");
                return ExecOutcome::Continue(1);
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String], out: &mut dyn Write) -> ExecOutcome {
    if let Err(e) = writeln!(out, "{}", args.join(" ")) {
        eprintln!("shuck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code),
            Err(_) => {
                eprintln!("shuck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognizes_builtins() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("pwd"));
        assert!(is_builtin("echo"));
        assert!(!is_builtin("ls"));
    }

    #[test]
    fn exit_with_no_args() {
        assert!(matches!(builtin_exit(&[]), ExecOutcome::Exit(0)));
    }

    #[test]
    fn exit_with_code() {
        assert!(matches!(
            builtin_exit(&["3".to_string()]),
            ExecOutcome::Exit(3)
        ));
    }

    #[test]
    fn exit_with_bad_code_continues() {
        assert!(matches!(
            builtin_exit(&["abc".to_string()]),
            ExecOutcome::Continue(_)
        ));
    }

    #[test]
    fn echo_writes_args_joined_by_spaces() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_echo(&["hello".to_string(), "world".to_string()], &mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(out, b"hello world\n");
    }

    #[test]
    fn echo_with_no_args_writes_a_blank_line() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[], &mut out);
        assert_eq!(out, b"\n");
    }

    #[test]
    fn pwd_writes_the_current_directory() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_pwd(&mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let written = String::from_utf8(out).unwrap();
        let expected = env::current_dir().unwrap();
        assert_eq!(written.trim_end(), expected.to_str().unwrap());
    }
}
```

- [ ] **Step 6: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 82 tests pass (74 existing + 8 new in `shell_state`).

- [ ] **Step 7: Commit**

```bash
git add src/shell_state.rs src/main.rs src/shell.rs src/executor.rs src/builtins.rs
git commit -m "feat: introduce Shell state struct and thread through executor"
```

---

## Task 2: Word types, expand module, command-type migration

This is the big interface migration. After this task: `Token::Word` carries a structured `Word` (the lexer wraps every existing string as a single `Literal` part — no new expansion lexing yet); `command.rs` exposes `ExecCommand` and `SimpleCommand::{Assign, Exec}` (parser produces only `Exec`); a new `expand` module turns a `Word` into 0+ `String`s; the executor expands per command. Every existing v3 behavior is preserved — `cargo test` still passes the same scenarios.

**Files:**
- Modify: `src/main.rs` (add `mod expand;`)
- Create: `src/expand.rs`
- Modify: `src/lexer.rs` (full replacement)
- Modify: `src/command.rs` (full replacement)
- Modify: `src/executor.rs` (full replacement)
- Modify: `src/shell.rs` (lex_error_message stays correct — only new `LexError` variants in Task 3 expand it; no edit needed here)

- [ ] **Step 1: Update `src/main.rs`** to add the new module. Replace the file entirely with:

```rust
mod builtins;
mod command;
mod executor;
mod expand;
mod lexer;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
```

- [ ] **Step 2: Create `src/expand.rs`** with the full algorithm. Even though Task 2's lexer only emits `Literal` parts, this module already handles every variant — Task 3 just exercises the others.

```rust
use crate::lexer::{Word, WordPart};
use crate::shell_state::Shell;

/// Expands a `Word` against the current `Shell` state into 0 or more
/// argument strings. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
pub fn expand(word: &Word, shell: &Shell) -> Vec<String> {
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
                let value = shell.get(name).unwrap_or("");
                emit_split(value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::LastStatus { quoted: false } => {
                let value = shell.last_status().to_string();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
        }
    }

    if has_emitted {
        result.push(current);
    }
    result
}

/// Splits `value` on ASCII whitespace and integrates the fields into the
/// caller's accumulator state, following the standard word-splitting rule.
fn emit_split(
    value: &str,
    current: &mut String,
    result: &mut Vec<String>,
    has_emitted: &mut bool,
) {
    let fields: Vec<&str> = value.split_ascii_whitespace().collect();
    match fields.len() {
        0 => {
            // No fields — the unquoted empty expansion contributes nothing.
        }
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

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal(s.to_string())])
    }

    #[test]
    fn expand_literal_word() {
        let shell = Shell::new();
        assert_eq!(expand(&lit("hello"), &shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let shell = Shell::new();
        assert_eq!(expand(&lit(""), &shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal("foo".to_string()),
            WordPart::Literal("bar".to_string()),
        ]);
        assert_eq!(expand(&word, &shell), vec!["foobar".to_string()]);
    }
}
```

- [ ] **Step 3: Replace `src/lexer.rs` entirely with this**

(Existing operator/quoting/escape rules unchanged. The lexer now emits `Word(vec![Literal(s)])` instead of `String(s)`. `Word`/`WordPart` types are defined; the state machine only constructs `Literal` parts. Task 3 will add the `$`/`~` recognition.)

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
    And,            // &&
    Or,             // ||
    Semi,           // ;
}

#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
    Op(Operator),
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                tokens.push(Token::Word(Word(vec![WordPart::Literal(
                    std::mem::take(&mut current),
                )])));
                has_token = false;
            }
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\')) => current.push(esc),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                has_token = true;
                match chars.next() {
                    Some(ch) => current.push(ch),
                    None => current.push('\\'),
                }
            }
            '|' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                }
            }
            '&' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    return Err(LexError::BareAmpersand);
                }
            }
            ';' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
            }
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        tokens.push(Token::Word(Word(vec![WordPart::Literal(current)])));
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a Token that holds a single-Literal Word.
    fn w(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal(s.to_string())]))
    }

    /// Builds a Vec<Token> of all-Literal words.
    fn words(parts: &[&str]) -> Vec<Token> {
        parts.iter().map(|s| w(s)).collect()
    }

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), words(&["ls", "-la"]));
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_single_quotes() {
        assert_eq!(
            tokenize("echo 'hello world'").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quotes() {
        assert_eq!(
            tokenize("echo \"hello world\"").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), words(&["echo", "a\"b"]));
    }

    #[test]
    fn tokenize_backslash_escape_outside_quotes() {
        assert_eq!(tokenize(r"echo a\ b").unwrap(), words(&["echo", "a b"]));
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize(r"echo a\").unwrap(), words(&["echo", r"a\"]));
    }

    #[test]
    fn tokenize_adjacent_runs_concatenate() {
        assert_eq!(tokenize(r#"foo"bar baz""#).unwrap(), words(&["foobar baz"]));
    }

    #[test]
    fn tokenize_single_quotes_preserve_backslash() {
        assert_eq!(tokenize(r"echo 'a\b'").unwrap(), words(&["echo", r"a\b"]));
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        assert_eq!(tokenize("''").unwrap(), words(&[""]));
    }

    #[test]
    fn tokenize_unterminated_single_quote() {
        assert_eq!(
            tokenize("echo 'oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_unterminated_double_quote() {
        assert_eq!(
            tokenize("echo \"oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_pipe_with_spaces() {
        assert_eq!(
            tokenize("a | b").unwrap(),
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![w("cat"), Token::Op(Operator::RedirIn), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErr), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![w("x2"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_two_not_followed_by_redirect_is_a_word() {
        assert_eq!(tokenize("2 foo").unwrap(), words(&["2", "foo"]));
    }

    #[test]
    fn tokenize_quoted_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "|" ">""#).unwrap(),
            words(&["echo", "|", ">"])
        );
    }

    #[test]
    fn tokenize_escaped_operators_stay_words() {
        assert_eq!(tokenize(r"echo \| \>").unwrap(), words(&["echo", "|", ">"]));
    }

    #[test]
    fn tokenize_pipeline_with_redirects() {
        assert_eq!(
            tokenize("a < in | b > out").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::RedirIn),
                w("in"),
                Token::Op(Operator::Pipe),
                w("b"),
                Token::Op(Operator::RedirOut),
                w("out"),
            ]
        );
    }

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_is_error() {
        assert_eq!(tokenize("a & b").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_error() {
        assert_eq!(tokenize("a &").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_semicolon_with_spaces() {
        assert_eq!(
            tokenize("a ; b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_quoted_sequencing_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "&&" "||" ";""#).unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_escaped_sequencing_operators_stay_words() {
        assert_eq!(
            tokenize(r"echo \&\& \|\| \;").unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_combined_sequencing_operators() {
        assert_eq!(
            tokenize("a && b || c ; d").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::And),
                w("b"),
                Token::Op(Operator::Or),
                w("c"),
                Token::Op(Operator::Semi),
                w("d"),
            ]
        );
    }
}
```

- [ ] **Step 4: Replace `src/command.rs` entirely with this**

```rust
use crate::lexer::{Operator, Token, Word};

#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    Truncate(Word),
    Append(Word),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExecCommand {
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SimpleCommand {
    Assign { name: String, value: Word },
    Exec(ExecCommand),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi,
    And,
    Or,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut iter = tokens.into_iter().peekable();
    let first = parse_pipeline(&mut iter)?;
    let mut rest = Vec::new();

    while let Some(token) = iter.next() {
        let connector = match token {
            Token::Op(Operator::Semi) => Connector::Semi,
            Token::Op(Operator::And) => Connector::And,
            Token::Op(Operator::Or) => Connector::Or,
            _ => unreachable!(
                "parse_pipeline leaves only sequencing ops in the iterator; \
                 anything else it consumes itself"
            ),
        };
        if matches!(connector, Connector::Semi) && iter.peek().is_none() {
            break;
        }
        let pipeline = parse_pipeline(&mut iter)?;
        rest.push((connector, pipeline));
    }

    Ok(Some(Sequence { first, rest }))
}

fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<SimpleCommand> = Vec::new();

    let mut program: Option<Word> = None;
    let mut args: Vec<Word> = Vec::new();
    let mut stdin: Option<Word> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    while let Some(token) = iter.peek() {
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or)
        ) {
            break;
        }
        let token = iter.next().unwrap();
        match token {
            Token::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(SimpleCommand::Exec(ExecCommand {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                }));
            }
            Token::Op(op) => {
                let target = match iter.next() {
                    Some(Token::Word(word)) => word,
                    Some(Token::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    None => return Err(ParseError::MissingRedirectTarget),
                };
                match op {
                    Operator::RedirIn => stdin = Some(target),
                    Operator::RedirOut => stdout = Some(Redirect::Truncate(target)),
                    Operator::RedirAppend => stdout = Some(Redirect::Append(target)),
                    Operator::RedirErr => stderr = Some(Redirect::Truncate(target)),
                    Operator::RedirErrAppend => stderr = Some(Redirect::Append(target)),
                    Operator::Pipe | Operator::And | Operator::Or | Operator::Semi => {
                        unreachable!("handled in the outer arms");
                    }
                }
            }
        }
    }

    let prog = program.ok_or(ParseError::MissingCommand)?;
    commands.push(SimpleCommand::Exec(ExecCommand {
        program: prog,
        args,
        stdin,
        stdout,
        stderr,
    }));

    Ok(Pipeline { commands })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::WordPart;

    fn w_tok(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal(s.to_string())]))
    }

    fn ww(s: &str) -> Word {
        Word(vec![WordPart::Literal(s.to_string())])
    }

    /// Builds a SimpleCommand::Exec with no redirections, all-Literal Words.
    fn plain(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            program: ww(program),
            args: args.iter().map(|a| ww(a)).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
        Sequence {
            first: Pipeline { commands },
            rest: vec![],
        }
    }

    fn exec_stdout(seq: &Sequence) -> &Option<Redirect> {
        match &seq.first.commands[0] {
            SimpleCommand::Exec(e) => &e.stdout,
            _ => panic!("expected Exec"),
        }
    }

    fn exec_stdin(seq: &Sequence) -> &Option<Word> {
        match &seq.first.commands[0] {
            SimpleCommand::Exec(e) => &e.stdin,
            _ => panic!("expected Exec"),
        }
    }

    fn exec_stderr(seq: &Sequence) -> &Option<Redirect> {
        match &seq.first.commands[0] {
            SimpleCommand::Exec(e) => &e.stderr,
            _ => panic!("expected Exec"),
        }
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w_tok("ls")]),
            Ok(Some(one_pipeline(vec![plain("ls", &[])])))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w_tok("ls"), w_tok("-la"), w_tok("/tmp")]),
            Ok(Some(one_pipeline(vec![plain("ls", &["-la", "/tmp"])])))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Truncate(ww("f"))));
    }

    #[test]
    fn parse_redirect_append() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirAppend), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Append(ww("f"))));
    }

    #[test]
    fn parse_redirect_in() {
        let seq = parse(vec![w_tok("cat"), Token::Op(Operator::RedirIn), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdin(&seq), &Some(ww("f")));
    }

    #[test]
    fn parse_redirect_stderr() {
        let seq = parse(vec![w_tok("cmd"), Token::Op(Operator::RedirErr), w_tok("e")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Truncate(ww("e"))));
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let seq = parse(vec![
            w_tok("cmd"),
            Token::Op(Operator::RedirErrAppend),
            w_tok("e"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Append(ww("e"))));
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Pipe), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[]), plain("b", &[])]);
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let seq = parse(vec![
            w_tok("a"),
            Token::Op(Operator::Pipe),
            w_tok("b"),
            Token::Op(Operator::Pipe),
            w_tok("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands.len(), 3);
    }

    #[test]
    fn parse_leading_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Pipe), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::Pipe)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::Pipe),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_program_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::RedirOut), w_tok("f")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_target_is_error() {
        assert_eq!(
            parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut)]),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_is_operator_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }

    #[test]
    fn parse_semicolon_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
    }

    #[test]
    fn parse_and_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::And), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_or_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Or), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::Or);
    }

    #[test]
    fn parse_trailing_semicolon_is_allowed() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi)])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_trailing_and_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::And)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_leading_semicolon_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Semi), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_sequencing_op_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::And),
                Token::Op(Operator::And),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }
}
```

(This is a curated subset of the v3 parser tests, migrated to the new types. The redirect-stderr-append and several other v3 cases follow the same pattern; this set covers all distinct parser behaviors so it's representative.)

- [ ] **Step 5: Replace `src/executor.rs` entirely with this**

The executor now expands each Word per command (calling `expand::expand`), validates redirect words to single strings, and dispatches to builtin or subprocess. `SimpleCommand::Assign` arms are `unreachable!` in this task because the parser does not yet produce them; Task 4 implements them. The existing v3 pipeline and sequencing logic is preserved.

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{
    Connector, ExecCommand, Pipeline, Redirect, Sequence, SimpleCommand,
};
use crate::expand::expand;
use crate::shell_state::Shell;

pub fn execute(seq: &Sequence, shell: &mut Shell) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first, shell);
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
            status = run_pipeline(pipeline, shell);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0], shell)
    } else {
        run_multi_stage(&pipeline.commands, shell)
    }
}

// ----- resolved command (post-expansion) ------------------------------------

/// A command with every `Word` already expanded against the live `Shell`.
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

/// Expands a `Word` to exactly one string, or prints an `ambiguous redirect`
/// error and returns `Err(())`.
fn expand_single(word: &crate::lexer::Word, shell: &Shell) -> Result<String, ()> {
    let fields = expand(word, shell);
    if fields.len() == 1 {
        Ok(fields.into_iter().next().unwrap())
    } else {
        eprintln!("shuck: ambiguous redirect");
        Err(())
    }
}

/// Expands every Word in an ExecCommand. Returns `Err(status)` on failure
/// (empty program → 127, ambiguous redirect → 1).
fn resolve(cmd: &ExecCommand, shell: &Shell) -> Result<ResolvedCommand, i32> {
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

fn run_single(cmd: &SimpleCommand, shell: &mut Shell) -> ExecOutcome {
    match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell),
        SimpleCommand::Assign { .. } => {
            unreachable!("parser does not yet produce SimpleCommand::Assign")
        }
    }
}

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell) -> ExecOutcome {
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
            None => {
                let mut out = io::stdout();
                builtins::run_builtin(&resolved.program, &resolved.args, &mut out, shell)
            }
        }
    } else {
        run_subprocess(&resolved, files, shell)
    }
}

fn run_subprocess(cmd: &ResolvedCommand, files: StageFiles, shell: &Shell) -> ExecOutcome {
    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());
    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    match process.status() {
        Ok(status) => ExecOutcome::Continue(status_code(&status)),
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

fn run_multi_stage(commands: &[SimpleCommand], shell: &mut Shell) -> ExecOutcome {
    // Pre-resolve every stage. Any failure aborts and runs nothing.
    let mut resolved_stages: Vec<Option<ResolvedCommand>> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                // Task 4 makes Assign a no-op stage; the parser doesn't
                // produce it in Task 2 so this branch is unreachable here.
                unreachable!("parser does not yet produce SimpleCommand::Assign");
            }
            SimpleCommand::Exec(exec) => match resolve(exec, shell) {
                Ok(r) => resolved_stages.push(Some(r)),
                Err(code) => return ExecOutcome::Continue(code),
            },
        }
    }
    let mut all_files: Vec<StageFiles> = Vec::with_capacity(resolved_stages.len());
    for r in &resolved_stages {
        let r = r.as_ref().unwrap();
        match open_stage_files(r) {
            Ok(f) => all_files.push(f),
            Err(()) => return ExecOutcome::Continue(1),
        }
    }

    let n = resolved_stages.len();
    let mut stages: Vec<Stage> = Vec::with_capacity(n);
    let mut carry = Carry::None;

    for (i, (resolved, files)) in resolved_stages.iter().zip(all_files).enumerate() {
        let cmd = resolved.as_ref().unwrap();
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

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
            match &files.stdout {
                Some(_) => {
                    // The redirected file was already opened in all_files;
                    // re-grab it via files.stdout by taking ownership.
                }
                None => {}
            }
            // (The above match is structural; the write actually happens here:)
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
                        if let Err(e) = io::stdout().write_all(&buffer) {
                            eprintln!("shuck: {}: {e}", cmd.program);
                            status = 1;
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
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward {
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
```

Notes on the rewrite:
- Two empty/no-op match arms remain on `files.stdout` in the builtin pipeline-stage path because the previous block restructured how `files.stdout` is consumed. (The duplicated `match files.stdout` may look odd — please reproduce verbatim. The first match's arms intentionally do nothing; the second match consumes `files.stdout` and does the write.)

Actually, simplify by collapsing that. Use this corrected builtin-in-pipeline branch instead:

Replace this snippet inside `run_multi_stage` (the entire builtin echo/pwd handling block, from `let mut buffer: Vec<u8> = Vec::new();` through the `stages.push(Stage::Done(status)); continue;` ):

```rust
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
                        if let Err(e) = io::stdout().write_all(&buffer) {
                            eprintln!("shuck: {}: {e}", cmd.program);
                            status = 1;
                        }
                    } else {
                        carry = Carry::Buffer(buffer);
                    }
                }
            }
            stages.push(Stage::Done(status));
            continue;
```

(That is the only `match files.stdout` block in this branch — the earlier dead `match &files.stdout` in the pasted code is a stray and should be removed.)

- [ ] **Step 6: Verify the crate builds and all tests pass**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, approximately 85 tests pass (Task 1's 82 + 3 new expand tests). Test totals here are approximate; the gate is **0 failed** and **0 warnings**.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/expand.rs src/lexer.rs src/command.rs src/executor.rs
git commit -m "feat: Word/SimpleCommand types and expand-time execution"
```

---

## Task 3: Lexer recognizes `$VAR` / `${VAR}` / `$?` / `~`

Turn on the lexer-side recognition: `$NAME` / `${NAME}` produce `Var` parts; `$?` produces `LastStatus`; `~` at word start produces `Tilde`. Add `LexError::InvalidVarName` and `LexError::UnterminatedBrace`. Update `shell.rs`'s `lex_error_message` for the new variants. After this task, `echo $HOME`, `echo "$HOME"`, `echo $?`, `echo ~/foo` all work.

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/shell.rs`

- [ ] **Step 1: Update `src/lexer.rs`**

Replace the `pub enum LexError` block with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
}
```

Replace the function `tokenize` body with the version below. The structure is the same as Task 2's tokenize, with two additions: (a) inside the existing `'"'` (double-quote) arm, the `Some(other)` literal-add path is replaced by a sub-state-machine that recognizes `$` and adds `Var`/`LastStatus` parts (Tilde is **not** recognized inside double quotes); (b) outside any quote, new `'$'` and `'~'` arms recognize the same things in `quoted: false` mode. Both contexts share two helpers (`read_var_name` and `read_braced_var_name`) defined immediately below `tokenize`.

```rust
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut parts: Vec<WordPart> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current);
                tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                has_token = false;
            }
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\')) => current.push(esc),
                            Some('$') => current.push('$'), // `\$` -> literal $
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            flush_literal(&mut parts, &mut current);
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                has_token = true;
                match chars.next() {
                    Some(ch) => current.push(ch),
                    None => current.push('\\'),
                }
            }
            '$' => {
                // Expansion outside any quotes (quoted: false).
                has_token = true;
                flush_literal(&mut parts, &mut current);
                read_dollar_expansion(&mut chars, &mut parts, false)?;
            }
            '~' if !has_token && tilde_at_word_start(&chars) => {
                has_token = true;
                parts.push(WordPart::Tilde);
            }
            '|' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                }
            }
            '&' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    return Err(LexError::BareAmpersand);
                }
            }
            ';' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
            }
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        flush_literal(&mut parts, &mut current);
        tokens.push(Token::Word(Word(parts)));
    }
    Ok(tokens)
}

fn flush_literal(parts: &mut Vec<WordPart>, current: &mut String) {
    if !current.is_empty() {
        parts.push(WordPart::Literal(std::mem::take(current)));
    } else if parts.is_empty() {
        // The token exists (e.g. from `""`) but no literal text has accumulated.
        // Push an empty Literal so expansion's `has_emitted` fires.
        parts.push(WordPart::Literal(String::new()));
    }
}

/// Reads what follows a `$`. Pushes the resulting WordPart onto `parts` or
/// (for an unrecognized form) pushes a literal `$` and lets the caller
/// continue tokenizing.
fn read_dollar_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('{') => {
            chars.next(); // consume '{'
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
            // Unrecognized $ — emit a literal `$` and continue.
            parts.push(WordPart::Literal("$".to_string()));
        }
    }
    Ok(())
}

fn read_var_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if is_name_cont(c) {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    name
}

fn read_braced_var_name(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut name = String::new();
    let first = chars.next().ok_or(LexError::UnterminatedBrace)?;
    if !is_name_start(first) {
        // Continue consuming until the closing brace so the error is recoverable
        // (the REPL prints a single syntax error and re-prompts).
        loop {
            match chars.next() {
                Some('}') => break,
                Some(_) => continue,
                None => return Err(LexError::UnterminatedBrace),
            }
        }
        return Err(LexError::InvalidVarName);
    }
    name.push(first);
    loop {
        match chars.next() {
            Some('}') => return Ok(name),
            Some(c) if is_name_cont(c) => name.push(c),
            Some(_) => {
                // Drain to closing brace then error.
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(_) => continue,
                        None => return Err(LexError::UnterminatedBrace),
                    }
                }
                return Err(LexError::InvalidVarName);
            }
            None => return Err(LexError::UnterminatedBrace),
        }
    }
}

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_name_cont(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// True iff a `~` would expand here: next char is `/`, whitespace, an
/// operator metachar (`|`, `<`, `>`, `&`, `;`), or end of input.
fn tilde_at_word_start(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    match chars.clone().peek() {
        None => true,
        Some(&c) => {
            c == '/'
                || c.is_whitespace()
                || matches!(c, '|' | '<' | '>' | '&' | ';')
        }
    }
}
```

Add these tests at the bottom of the `mod tests` block in `src/lexer.rs`:

```rust
    fn vword_unquoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: false,
        }]))
    }

    fn vword_quoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: true,
        }]))
    }

    #[test]
    fn tokenize_dollar_var_unquoted() {
        assert_eq!(tokenize("$FOO").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_braced() {
        assert_eq!(tokenize("${FOO}").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_double_quotes_is_quoted() {
        assert_eq!(tokenize("\"$FOO\"").unwrap(), vec![vword_quoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_single_quotes_is_literal() {
        assert_eq!(tokenize("'$FOO'").unwrap(), words(&["$FOO"]));
    }

    #[test]
    fn tokenize_last_status() {
        assert_eq!(
            tokenize("$?").unwrap(),
            vec![Token::Word(Word(vec![WordPart::LastStatus {
                quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_then_digit_is_literal_dollar() {
        // `$5` -> literal `$` + word continues with `5`
        assert_eq!(
            tokenize("$5").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("$".to_string()),
                WordPart::Literal("5".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_double_dollar_is_two_literal_dollars() {
        // `$$` -> two literal `$`s
        assert_eq!(
            tokenize("$$").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("$".to_string()),
                WordPart::Literal("$".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_alone() {
        assert_eq!(
            tokenize("~").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde]))]
        );
    }

    #[test]
    fn tokenize_tilde_slash_path() {
        assert_eq!(
            tokenize("~/foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde,
                WordPart::Literal("/foo".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_mid_word_is_literal() {
        assert_eq!(tokenize("a~b").unwrap(), words(&["a~b"]));
    }

    #[test]
    fn tokenize_tilde_followed_by_name_is_literal() {
        assert_eq!(tokenize("~foo").unwrap(), words(&["~foo"]));
    }

    #[test]
    fn tokenize_tilde_in_quotes_is_literal() {
        assert_eq!(tokenize("\"~\"").unwrap(), words(&["~"]));
    }

    #[test]
    fn tokenize_braced_var_invalid_name() {
        assert_eq!(tokenize("${1foo}").unwrap_err(), LexError::InvalidVarName);
    }

    #[test]
    fn tokenize_braced_var_empty_name() {
        assert_eq!(tokenize("${}").unwrap_err(), LexError::InvalidVarName);
    }

    #[test]
    fn tokenize_unterminated_brace() {
        assert_eq!(tokenize("${FOO").unwrap_err(), LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_var_concatenates_with_literal() {
        assert_eq!(
            tokenize("a$FOOb").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("a".to_string()),
                WordPart::Var { name: "FOOb".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_braced_var_separates_from_following_word() {
        assert_eq!(
            tokenize("${FOO}bar").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Literal("bar".to_string()),
            ]))]
        );
    }
```

- [ ] **Step 2: Update `src/shell.rs`'s `lex_error_message`**

Replace:

```rust
fn lex_error_message(error: LexError) -> &'static str {
    match error {
        LexError::UnterminatedQuote => "unterminated quote",
        LexError::BareAmpersand => "unexpected '&'",
    }
}
```

with:

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

- [ ] **Step 3: Add expand tests** for the new WordPart variants. Add these at the bottom of the `mod tests` block in `src/expand.rs`:

```rust
    fn var_unq(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: false }])
    }
    fn var_q(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: true }])
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let shell = Shell::new();
        assert!(expand(&var_unq("DEFINITELY_NOT_SET_XYZ"), &shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let shell = Shell::new();
        assert_eq!(
            expand(&var_q("DEFINITELY_NOT_SET_XYZ"), &shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(expand(&var_q("SHUCK_T"), &shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(
            expand(&var_unq("SHUCK_T"), &shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_unquoted_var_with_literal_prefix_merges_first_field() {
        // `a$SHUCK_T` where SHUCK_T = "x y" -> ["ax", "y"]
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "x y".to_string());
        let word = Word(vec![
            WordPart::Literal("a".to_string()),
            WordPart::Var { name: "SHUCK_T".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand(&word, &shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand(&word, &shell), vec!["42".to_string()]);
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
            expand(&word, &shell),
            vec!["/tmp/shuck_test/foo".to_string()]
        );
    }
```

- [ ] **Step 4: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, all tests green. Test count grows by the new lexer and expand tests (roughly +25); the gate is **0 failed** and **0 warnings**.

- [ ] **Step 5: Smoke test**

Quick spot-check the new behavior works end-to-end:

```bash
cargo build -q
printf '%s\n' 'echo $HOME' 'echo "$HOME"' 'echo $?' 'echo ~' 'echo ~/foo' 'echo $UNDEFINED_XYZ done' 'exit 0' \
  | ./target/debug/shuck
```

Expected: the user's actual `$HOME` (twice, same value), `0`, the user's `$HOME` again, `$HOME/foo`, then a single line `done` (the unquoted `$UNDEFINED_XYZ` contributes zero args, so `echo` just gets `done`).

- [ ] **Step 6: Commit**

```bash
git add src/lexer.rs src/shell.rs src/expand.rs
git commit -m "feat: lexer recognizes \$VAR, \${VAR}, \$?, and ~"
```

---

## Task 4: Parser detects assignment; executor handles `Assign`

The parser detects `NAME=value` as an assignment when the stage is structurally pure (one program word, no args, no redirects) and the program word's first part is a `Literal` matching `^[A-Za-z_][A-Za-z0-9_]*=`. The executor's `Assign` arms become real: standalone (in `run_single`) sets the variable; in a multi-stage pipeline, it's a no-op (consistent with v2/v3 treatment of `cd`/`exit`).

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs`

- [ ] **Step 1: Update `src/command.rs`** to add assignment detection.

Add this helper function at the top of `command.rs` (after the `use` statement, before the type definitions):

```rust
/// If `word` looks like `NAME=value` (a leading `Literal` whose text begins
/// with a valid identifier followed by `=`), returns `Some((name, value))`
/// where `value` is a `Word` containing the rest of the prefix Literal
/// followed by the remaining original parts. Otherwise `None`.
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
        // Clone each remaining part. WordPart isn't Clone by default; replicate manually.
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

Then update the two places in `parse_pipeline` that push a `SimpleCommand::Exec`. Find this block (it appears twice — at the Pipe arm and at the end-of-loop finalize):

```rust
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(SimpleCommand::Exec(ExecCommand {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                }));
```

Replace **both occurrences** with this finalize helper. First, add the helper as a free function at the bottom of `command.rs` (outside any other function):

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

Then replace the **Pipe arm** (inside the `match token` block):

```rust
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(finalize_stage(
                    prog,
                    std::mem::take(&mut args),
                    stdin.take(),
                    stdout.take(),
                    stderr.take(),
                ));
            }
```

And the **end-of-loop finalize** (just before `Ok(Pipeline { commands })`):

```rust
    let prog = program.ok_or(ParseError::MissingCommand)?;
    commands.push(finalize_stage(prog, args, stdin, stdout, stderr));

    Ok(Pipeline { commands })
}
```

Add these parser tests at the bottom of the `mod tests` block in `command.rs`:

```rust
    fn assignment(name: &str, value: Word) -> SimpleCommand {
        SimpleCommand::Assign { name: name.to_string(), value }
    }

    #[test]
    fn parse_simple_assignment() {
        let seq = parse(vec![w_tok("FOO=bar")]).unwrap().unwrap();
        assert_eq!(seq.first.commands, vec![assignment("FOO", ww("bar"))]);
    }

    #[test]
    fn parse_empty_value_assignment() {
        let seq = parse(vec![w_tok("FOO=")]).unwrap().unwrap();
        assert_eq!(seq.first.commands, vec![assignment("FOO", ww(""))]);
    }

    #[test]
    fn parse_assignment_with_expansion_in_value() {
        // `FOO=$BAR` produces Assign with value Word([Literal(""), Var{BAR, unquoted}])
        let var_part = WordPart::Var { name: "BAR".to_string(), quoted: false };
        let prog = Token::Word(Word(vec![
            WordPart::Literal("FOO=".to_string()),
            var_part,
        ]));
        let seq = parse(vec![prog]).unwrap().unwrap();
        let expected_value = Word(vec![
            WordPart::Literal("".to_string()),
            WordPart::Var { name: "BAR".to_string(), quoted: false },
        ]);
        assert_eq!(seq.first.commands, vec![assignment("FOO", expected_value)]);
    }

    #[test]
    fn parse_assignment_invalid_name_is_exec() {
        // `1FOO=bar` — leading digit, not a valid name
        let seq = parse(vec![w_tok("1FOO=bar")]).unwrap().unwrap();
        assert_eq!(seq.first.commands, vec![plain("1FOO=bar", &[])]);
    }

    #[test]
    fn parse_assignment_with_arg_is_exec() {
        let seq = parse(vec![w_tok("FOO=bar"), w_tok("baz")]).unwrap().unwrap();
        assert_eq!(seq.first.commands, vec![plain("FOO=bar", &["baz"])]);
    }

    #[test]
    fn parse_assignment_with_redirect_is_exec() {
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::RedirOut),
            w_tok("f"),
        ])
        .unwrap()
        .unwrap();
        match &seq.first.commands[0] {
            SimpleCommand::Exec(e) => {
                assert_eq!(e.program, ww("FOO=bar"));
                assert_eq!(e.stdout, Some(Redirect::Truncate(ww("f"))));
            }
            _ => panic!("expected Exec"),
        }
    }

    #[test]
    fn parse_assignment_in_pipeline_stage() {
        // `FOO=bar | cat` — first stage is Assign, second is Exec
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::Pipe),
            w_tok("cat"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands.len(), 2);
        assert_eq!(seq.first.commands[0], assignment("FOO", ww("bar")));
        assert_eq!(seq.first.commands[1], plain("cat", &[]));
    }
```

- [ ] **Step 2: Update `src/executor.rs`** to handle `Assign`.

Replace this branch in `run_single`:

```rust
fn run_single(cmd: &SimpleCommand, shell: &mut Shell) -> ExecOutcome {
    match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell),
        SimpleCommand::Assign { .. } => {
            unreachable!("parser does not yet produce SimpleCommand::Assign")
        }
    }
}
```

with:

```rust
fn run_single(cmd: &SimpleCommand, shell: &mut Shell) -> ExecOutcome {
    match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell),
        SimpleCommand::Assign { name, value } => {
            let fields = expand(value, shell);
            let joined = fields.join(" ");
            shell.set(name, joined);
            ExecOutcome::Continue(0)
        }
    }
}
```

And replace the `Assign` branch in `run_multi_stage`'s pre-resolve loop:

```rust
            SimpleCommand::Assign { .. } => {
                // Task 4 makes Assign a no-op stage; the parser doesn't
                // produce it in Task 2 so this branch is unreachable here.
                unreachable!("parser does not yet produce SimpleCommand::Assign");
            }
```

with:

```rust
            SimpleCommand::Assign { .. } => {
                // Inside a multi-stage pipeline, an assignment is a no-op
                // (its shell-state effect would only apply to a subshell,
                // which we don't fork). We record a placeholder None here
                // so the index alignment with `all_files` and the stage
                // loop stays correct.
                resolved_stages.push(None);
            }
```

Then, in `run_multi_stage`'s subsequent loops, account for `None` resolved stages. Replace this snippet (the loop that opens stage files):

```rust
    let mut all_files: Vec<StageFiles> = Vec::with_capacity(resolved_stages.len());
    for r in &resolved_stages {
        let r = r.as_ref().unwrap();
        match open_stage_files(r) {
            Ok(f) => all_files.push(f),
            Err(()) => return ExecOutcome::Continue(1),
        }
    }
```

with:

```rust
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
```

And update the stage-execution loop's header. Replace:

```rust
    for (i, (resolved, files)) in resolved_stages.iter().zip(all_files).enumerate() {
        let cmd = resolved.as_ref().unwrap();
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

        if builtins::is_builtin(&cmd.program) {
```

with:

```rust
    for (i, (resolved, files)) in resolved_stages.iter().zip(all_files).enumerate() {
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

        let cmd = match resolved {
            Some(r) => r,
            None => {
                // Assign stage in a pipeline: no-op, hand the next stage
                // an empty input so it doesn't see the terminal stdin.
                drop(incoming);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                drop(files); // unused for Assign stages
                continue;
            }
        };

        if builtins::is_builtin(&cmd.program) {
```

Wait — `files` here is `Option<StageFiles>`; after the rename it's `Option<StageFiles>`, but the rest of the existing body expects `files` to be a `StageFiles`. Unwrap it once we've consumed the Assign case. Add this line right after the `match resolved { ... }` block:

```rust
        let files = files.expect("non-Assign stage must have StageFiles");
```

(The `Assign` branch above already `continue`s, so by this point `resolved` was `Some` and so was `files`.)

- [ ] **Step 3: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, all tests green. New parser tests for assignment all pass.

- [ ] **Step 4: Smoke test**

```bash
cargo build -q
printf '%s\n' \
  'FOO=bar' \
  'echo $FOO' \
  'FOO="a b"' \
  'echo $FOO' \
  'echo "$FOO"' \
  'echo done' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output (lines):
```
bar
a b
a b
done
```
(`echo $FOO` unquoted with FOO="a b" splits to two args, but `echo` joins them with a single space and prints `a b` — same visible output as quoted. The distinction matters when piped to a word-counting command.)

- [ ] **Step 5: Commit**

```bash
git add src/command.rs src/executor.rs
git commit -m "feat: parser detects assignment; executor sets shell vars"
```

---

## Task 5: `export` and `unset` builtins

Add the two new builtins. `is_builtin` recognizes them. `run_builtin` dispatches them. Each gets unit tests against a synthetic `Shell`.

**Files:**
- Modify: `src/builtins.rs`

- [ ] **Step 1: Update `src/builtins.rs`**

In `is_builtin`, replace:

```rust
pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo")
}
```

with:

```rust
pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo" | "export" | "unset")
}
```

In `run_builtin`, replace its `match` body to add the two new arms:

```rust
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}
```

Add these two builtin function definitions (anywhere below `builtin_exit`, above the test module):

```rust
fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false; };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn builtin_export(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut entries: Vec<(String, String)> = shell
            .exported_env()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        entries.sort();
        for (name, value) in entries {
            if let Err(e) = writeln!(out, "export {name}={value}") {
                eprintln!("shuck: export: {e}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_error = false;
    for arg in args {
        match arg.find('=') {
            Some(idx) => {
                let name = &arg[..idx];
                let value = &arg[idx + 1..];
                if !is_valid_name(name) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export_set(name, value.to_string());
            }
            None => {
                if !is_valid_name(arg) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export(arg);
            }
        }
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

fn builtin_unset(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut any_error = false;
    for arg in args {
        if !is_valid_name(arg) {
            eprintln!("shuck: unset: '{arg}': not a valid identifier");
            any_error = true;
            continue;
        }
        shell.unset(arg);
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}
```

Add these tests to the `mod tests` block (the new tests construct a `Shell` directly):

```rust
    #[test]
    fn export_marks_existing() {
        let mut shell = Shell::new();
        shell.set("SHUCK_EXP", "v".to_string());
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP");
        assert!(in_exported);
    }

    #[test]
    fn export_sets_and_exports() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP2=hello".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_EXP2"), Some("hello"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP2");
        assert!(in_exported);
    }

    #[test]
    fn export_invalid_name_continues_with_error() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(
            &["1BAD=x".to_string(), "GOOD=y".to_string()],
            &mut out,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.get("1BAD"), None);
        assert_eq!(shell.get("GOOD"), Some("y"));
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("SHUCK_RM", "v".to_string());
        let outcome = builtin_unset(&["SHUCK_RM".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_RM"), None);
    }

    #[test]
    fn unset_invalid_name_is_error() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["1BAD".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unset_unknown_name_is_silent_ok() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["NEVER_SET_SHUCK_XYZ".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
```

- [ ] **Step 2: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, all tests green.

- [ ] **Step 3: Smoke test**

```bash
cargo build -q
printf '%s\n' \
  'export SHUCK_X=hello' \
  'env | grep ^SHUCK_X=' \
  'unset SHUCK_X' \
  'env | grep ^SHUCK_X= || echo gone' \
  'export' \
  'exit 0' \
  | ./target/debug/shuck | grep -E '^(SHUCK_X=|gone|export SHUCK_)' | head -5
```

Expected lines include `SHUCK_X=hello` and `gone`. The `export` (no args) listing shows the user's exported vars in alphabetical order, prefixed `export NAME=value`.

- [ ] **Step 4: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add export and unset builtins"
```

---

## Task 6: Full smoke test

End-to-end verification of the whole feature combined with v1–v3. This task is verification only — no commit.

**Files:** none

- [ ] **Step 1: Run the combined smoke script**

```bash
cargo build -q
W=$(mktemp -d)
printf '%s\n' \
  'FOO=hello' \
  'echo $FOO' \
  'FOO="a b"' \
  'echo $FOO' \
  'echo "$FOO"' \
  'false' \
  'echo $?' \
  'true && echo ok' \
  'false || echo also-ok' \
  'echo a > '"$W"'/out' \
  "F=$W"'/out' \
  'cat < $F' \
  'echo $UNDEFINED end' \
  'echo "$UNDEFINED" end' \
  'export SHUCK_E=visible' \
  'env | grep ^SHUCK_E=' \
  'unset SHUCK_E' \
  'env | grep ^SHUCK_E= || echo unset-worked' \
  'echo ~ | head -c 1' \
  'echo done' \
  'exit 0' \
  | ./target/debug/shuck
rm -rf "$W"
```

Expected lines (in order; the `env | grep` ones are subprocess output):
- `hello`
- `a b`
- `a b`
- `1` (status of `false`)
- `ok`
- `also-ok`
- (no output from `echo a > $W/out`)
- `a` (from `cat < $F`)
- `end` (the unquoted `$UNDEFINED` contributed 0 args; `echo end` prints `end`)
- ` end` (the quoted `"$UNDEFINED"` is one empty arg; `echo` prints empty + space + `end`)
- `SHUCK_E=visible`
- `unset-worked`
- `/` (first char of $HOME on Linux)
- `done`

- [ ] **Step 2: Verify ambiguous-redirect and syntax errors**

```bash
printf 'echo hi > $UNDEFINED_XYZ\n'              | ./target/debug/shuck
printf 'FOO="a b"; echo hi > $FOO\n'             | ./target/debug/shuck
printf 'echo ${}\n'                              | ./target/debug/shuck
printf 'echo ${FOO\n'                            | ./target/debug/shuck
```

Expected:
- `shuck: ambiguous redirect`
- `shuck: ambiguous redirect`
- `shuck: syntax error: invalid variable name in '${...}'`
- `shuck: syntax error: unterminated '${...}'`

- [ ] **Step 3: Confirm**

All output matches. If any line differs, stop and fix the relevant module before completing the plan.

---

## Self-Review Notes

- **Spec coverage:**
  - Shell state (`Shell`, exported/unexported, last_status, exported_env): Task 1.
  - `Token::Word(Word)` with `WordPart::{Literal, Var, LastStatus, Tilde}`: types in Task 2, `Literal` only; recognition turned on in Task 3.
  - `expand(&Word, &Shell) -> Vec<String>` with word splitting: Task 2 (full algorithm; only Literal exercised), Task 3 adds tests for the rest.
  - `ExecCommand`, `SimpleCommand::{Assign, Exec}`, parser produces only `Exec`: Task 2; assignment detection: Task 4.
  - Lexer `$VAR`/`${VAR}`/`$?`/`~` recognition and new `LexError` variants: Task 3.
  - Executor `&mut Shell`, per-command expansion, ambiguous-redirect check, `env_clear`+exported envs: Tasks 1 (threading + env), 2 (expansion), 4 (Assign).
  - `cd` reads `HOME` via `shell.get`: Task 1.
  - `export`/`unset` builtins + invalid-name handling: Task 5.
  - Shell wiring with `lex_error_message` for new variants: Task 3.
  - Empty quoted token still yields one empty arg (`flush_literal` pushes empty Literal if no parts): Task 3 lexer helper.
  - Assignment in pipeline = no-op: Task 4 (`resolved_stages` records `None`).
- **Type consistency:** `Word`/`WordPart` defined in `lexer.rs` (Task 2) and consumed by `command.rs`, `expand.rs`, `executor.rs`. `SimpleCommand::{Assign, Exec}` defined in `command.rs` (Task 2) and matched by `executor.rs`. `Shell::{get, set, export, export_set, unset, last_status, set_last_status, exported_env}` defined in `shell_state.rs` (Task 1) and called from `expand.rs`, `executor.rs`, `builtins.rs`. The two finalize sites in `parse_pipeline` (Pipe arm and end-of-loop) both route through `finalize_stage` (Task 4) so assignment detection is uniform.
- **Interim states are explicit:** Task 2's executor `Assign` arms are `unreachable!`; Task 4 replaces them. Each task's verification gate is `cargo build` warning-free + `cargo test` all-pass.
- **No placeholders:** every code step is a complete file, a complete located edit, or both; every run step has an exact command and an expected outcome.
