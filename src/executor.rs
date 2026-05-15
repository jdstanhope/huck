use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline, Redirect, Sequence};

pub fn execute(seq: &Sequence) -> ExecOutcome {
    // INTERIM (made real in Task 3): only the first pipeline runs; any rest
    // is reported as "not yet implemented".
    if !seq.rest.is_empty() {
        eprintln!("shuck: sequencing not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_pipeline(&seq.first)
}

fn run_pipeline(pipeline: &Pipeline) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0])
    } else {
        run_multi_stage(&pipeline.commands)
    }
}

// ----- redirect file handling -----------------------------------------------

/// The redirect files a command needs, already opened.
struct StageFiles {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

/// Opens every redirect file a command needs. On the first failure, prints the
/// error and returns `Err(())` — the caller must then run nothing.
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

/// Maps a finished child's status to an exit code, using the POSIX
/// `128 + signal` convention for signal-killed children.
fn status_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1))
}

// ----- single command -------------------------------------------------------

fn run_single(cmd: &Command) -> ExecOutcome {
    let files = match open_stage_files(cmd) {
        Ok(files) => files,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&cmd.program) {
        match files.stdout {
            Some(mut file) => builtins::run_builtin(&cmd.program, &cmd.args, &mut file),
            None => {
                let mut out = io::stdout();
                builtins::run_builtin(&cmd.program, &cmd.args, &mut out)
            }
        }
    } else {
        run_subprocess(cmd, files)
    }
}

fn run_subprocess(cmd: &Command, files: StageFiles) -> ExecOutcome {
    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
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

/// What a stage hands to the next stage's stdin.
enum Carry {
    None,
    ChildStdout(ChildStdout),
    Buffer(Vec<u8>),
}

/// A pipeline stage awaiting its final status.
enum Stage {
    Done(i32),
    Process(Child),
}

fn run_multi_stage(commands: &[Command]) -> ExecOutcome {
    // Pre-flight: open every redirect file first. If any fails, run nothing.
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
            let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer);
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
