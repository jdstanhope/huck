use std::io::ErrorKind;
use std::os::unix::process::ExitStatusExt;
use std::process::Command as ProcessCommand;

use crate::builtins::{self, ExecOutcome};
use crate::command::Command;

pub fn execute(cmd: &Command) -> ExecOutcome {
    if builtins::is_builtin(&cmd.program) {
        return builtins::run_builtin(&cmd.program, &cmd.args);
    }

    match ProcessCommand::new(&cmd.program).args(&cmd.args).status() {
        Ok(status) => {
            // A signal-killed child has no exit code; report 128 + signal,
            // the POSIX convention (e.g. 130 for SIGINT).
            let code = status
                .code()
                .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1));
            ExecOutcome::Continue(code)
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
