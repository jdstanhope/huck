//! `ExecBuilder` — per-call builder for [`Engine::exec`].
//!
//! Holds the script source + optional stdin bytes + merge flag, and runs them
//! through the engine's sink-aware path on `.run()` / `.capture()`.
//!
//! [`Engine::exec`]: crate::engine::Engine::exec

use crate::engine::{Engine, Output};
use crate::executor::{StderrSink, StdoutSink};

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
}

impl<'a> ExecBuilder<'a> {
    pub(crate) fn new(engine: &'a mut Engine, src: String) -> Self {
        ExecBuilder { engine, src, stdin: None, merge: false }
    }

    /// Feed these bytes as the script's stdin (fd 0). EOF arrives immediately
    /// after the bytes are consumed.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Route the script's fd 2 to fd 1 (bash `2>&1`). Under `.capture()` the
    /// merged bytes land in `Output.stdout` and `Output.stderr` is empty.
    pub fn merge_stderr(mut self) -> Self {
        self.merge = true;
        self
    }

    /// Run the script; fd 1 and fd 2 inherit (or merged-to-fd1 if `merge_stderr`).
    pub fn run(self) -> i32 {
        let mut out = StdoutSink::Terminal;
        let mut err = if self.merge { StderrSink::Merged } else { StderrSink::Terminal };
        self.run_with_sinks(&mut out, &mut err)
    }

    /// Run the script; capture fd 1 and fd 2 into `Output`.
    pub fn capture(self) -> Output {
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let (exit_code, stderr_str) = {
            let mut out = StdoutSink::Capture(&mut buf_out);
            // Merged: stderr writes go to the active stdout writer (buf_out).
            // Non-merged: stderr writes go to a separate capture buffer.
            if self.merge {
                let mut err = StderrSink::Merged;
                let code = self.run_with_sinks(&mut out, &mut err);
                (code, String::new())
            } else {
                let mut err = StderrSink::Capture(&mut buf_err);
                let code = self.run_with_sinks(&mut out, &mut err);
                (code, String::from_utf8_lossy(&buf_err).into_owned())
            }
        };
        Output {
            stdout: String::from_utf8_lossy(&buf_out).into_owned(),
            stderr: stderr_str,
            exit_code,
        }
    }

    fn run_with_sinks(self, out: &mut StdoutSink, err: &mut StderrSink) -> i32 {
        let ExecBuilder { engine, src, stdin, .. } = self;
        let run = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 {
            let label = engine.shell_cell().borrow().shell_argv0.clone();
            let args = engine.shell_cell().borrow().positional_args.clone();
            let code = crate::shell::run_program_in_sinks(
                &src,
                None,
                args,
                &label,
                false,
                out,
                err,
                engine.shell_cell(),
            );
            engine.shell_cell().borrow_mut().set_last_status(code);
            code
        };
        match stdin {
            Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || run(out, err)),
            None => run(out, err),
        }
    }
}
