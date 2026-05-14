use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::Command as ProcessCommand;

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline};

pub fn execute(pipeline: &Pipeline) -> ExecOutcome {
    // INTERIM: redirection (Task 4) and multi-command pipelines (Task 5) are
    // wired up in the following tasks. For now only a single command with no
    // redirections runs.
    if pipeline.commands.len() > 1 {
        eprintln!("shuck: pipelines not yet implemented");
        return ExecOutcome::Continue(1);
    }
    let cmd = &pipeline.commands[0];
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        eprintln!("shuck: redirection not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_simple(cmd)
}

fn run_simple(cmd: &Command) -> ExecOutcome {
    if builtins::is_builtin(&cmd.program) {
        let mut out = io::stdout();
        return builtins::run_builtin(&cmd.program, &cmd.args, &mut out);
    }

    match ProcessCommand::new(&cmd.program).args(&cmd.args).status() {
        Ok(status) => {
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
