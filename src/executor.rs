use std::io::ErrorKind;
use std::process::Command as ProcessCommand;

use crate::builtins::{self, ExecOutcome};
use crate::command::Command;

pub fn execute(cmd: &Command) -> ExecOutcome {
    if builtins::is_builtin(&cmd.program) {
        return builtins::run_builtin(&cmd.program, &cmd.args);
    }

    match ProcessCommand::new(&cmd.program).args(&cmd.args).status() {
        Ok(status) => ExecOutcome::Continue(status.code().unwrap_or(1)),
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
