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
        eprintln!("shuck: command not found");
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
        SimpleCommand::Assign { name, value } => {
            let fields = expand(value, shell);
            let joined = fields.join(" ");
            shell.set(name, joined);
            ExecOutcome::Continue(0)
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
    let mut resolved_stages: Vec<Option<ResolvedCommand>> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                // Inside a multi-stage pipeline, an assignment is a no-op
                // (its shell-state effect would only apply to a subshell,
                // which we don't fork). Record a placeholder None so the
                // index alignment with `all_files` and the stage loop stays
                // correct.
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
                // Assign stage in a pipeline: no-op. Hand the next stage an
                // empty input so it doesn't see the terminal stdin.
                drop(incoming);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                let _ = files; // Option<StageFiles>, always None here
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
