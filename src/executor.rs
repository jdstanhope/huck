use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline, Redirect};

pub fn execute(pipeline: &Pipeline) -> ExecOutcome {
    // INTERIM: multi-command pipelines are wired up in Task 5.
    if pipeline.commands.len() > 1 {
        eprintln!("shuck: pipelines not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_single(&pipeline.commands[0])
}

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

fn run_single(cmd: &Command) -> ExecOutcome {
    let files = match open_stage_files(cmd) {
        Ok(files) => files,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&cmd.program) {
        // Builtins do not read stdin; an opened `<` or `2>` file is ignored
        // (it is still opened above, so a bad path is still reported).
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
