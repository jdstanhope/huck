//! `ExecBuilder` — per-call builder for [`Engine::exec`].
//!
//! Holds the script source + optional stdin bytes + merge flag + sandbox knobs
//! (cwd / restricted / timeout), and runs them through the engine's sink-aware
//! path on `.run()` / `.capture()`.
//!
//! [`Engine::exec`]: crate::engine::Engine::exec

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::engine::{Engine, Output};
use crate::executor::{StderrSink, StdoutSink};
use crate::shell_state::Shell;

type LineCallback<'a> = Box<dyn FnMut(&str) + 'a>;

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
    cwd: Option<PathBuf>,
    restricted: bool,
    timeout: Option<Duration>,
    #[allow(dead_code)]
    on_stdout_line: Option<LineCallback<'a>>,
    #[allow(dead_code)]
    on_stderr_line: Option<LineCallback<'a>>,
}

impl<'a> ExecBuilder<'a> {
    pub(crate) fn new(engine: &'a mut Engine, src: String) -> Self {
        ExecBuilder {
            engine,
            src,
            stdin: None,
            merge: false,
            cwd: None,
            restricted: false,
            timeout: None,
            on_stdout_line: None,
            on_stderr_line: None,
        }
    }

    /// Feed these bytes as the script's stdin (fd 0). EOF arrives immediately
    /// after the bytes are consumed.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Route the script's fd 2 to fd 1 (bash `2>&1`). Under `.capture()` the
    /// merged bytes land in `Output.stdout` and `Output.stderr` is empty.
    ///
    /// For multi-stage pipelines, each non-last stage's fd 2 is aliased to its
    /// inter-stage pipe (matching bash `2>&1 |` semantics) — so an intermediate
    /// stage's stderr flows into the next stage's stdin, not directly into the
    /// captured buffer.
    pub fn merge_stderr(mut self) -> Self {
        self.merge = true;
        self
    }

    /// Run the script with CWD = `path` for the duration of the call. The
    /// process's prior cwd plus `Shell.vars["PWD"]` / `["OLDPWD"]` are
    /// snapshot-and-restored on exit (including panic unwind). On chdir
    /// failure the script still runs (best-effort), with `huck: cwd: <path>:
    /// <err>` emitted to real fd 2.
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Enable restricted mode for this call only (bash `rbash` subset:
    /// refuses `cd`, `exec`, command-names containing `/`, `source` of paths
    /// containing `/`, write-redirects to absolute or `..` paths, assignment
    /// to `SHELL`/`PATH`/`ENV`/`BASH_ENV`, and `set +r`). Refused operations
    /// emit `huck: restricted: <op>` via the active stderr sink and return
    /// exit 1; the script keeps running unless `set -e` propagates the
    /// failure.
    pub fn restricted(mut self, on: bool) -> Self {
        self.restricted = on;
        self
    }

    /// Abort the script if it hasn't finished within `dur`. Returns exit
    /// 124 on timeout (matches GNU `timeout(1)`). In-flight external
    /// children receive SIGTERM; builtins finish their current command and
    /// then the next command-boundary check aborts.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }

    /// Invoke `f(line)` for each complete line written to stdout. Trailing
    /// `\n` stripped. Final partial line (if no trailing newline at EOF) fires
    /// once at stream close. Callback runs on the caller's thread.
    pub fn on_stdout_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
        self.on_stdout_line = Some(Box::new(f));
        self
    }

    /// Same for stderr. Under `.merge_stderr()`, stderr is dup2'd onto stdout
    /// at the fd level — this callback never fires; all output flows through
    /// `on_stdout_line`.
    pub fn on_stderr_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
        self.on_stderr_line = Some(Box::new(f));
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
        let ExecBuilder {
            engine,
            src,
            stdin,
            merge: _,
            cwd,
            restricted,
            timeout,
            on_stdout_line: _,
            on_stderr_line: _,
        } = self;
        let cell = engine.shell_cell().clone();

        // 1. Spawn timer (if requested). Defend against a prior call leaving
        // the timeout_flag set.
        let timer = timeout.map(|dur| {
            let flag = cell.borrow().timeout_flag.clone();
            let pids = cell.borrow().live_external_children.clone();
            flag.store(false, std::sync::atomic::Ordering::Relaxed);
            crate::timeout::spawn_timer(dur, flag, pids)
        });

        // 2. Compose stdin -> cwd -> restricted+run via nested matches.
        let code = match stdin {
            Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || {
                run_cwd_then_inner(&cell, cwd.as_deref(), restricted, &src, out, err)
            }),
            None => run_cwd_then_inner(&cell, cwd.as_deref(), restricted, &src, out, err),
        };

        // 3. Cancel timer (joins the thread).
        if let Some(t) = timer {
            t.cancel();
        }

        // 4. If the timeout flag is set, override the natural exit code to 124.
        if cell
            .borrow()
            .timeout_flag
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return 124;
        }
        code
    }
}

/// Apply the cwd guard (if set), then run the restricted+inner core.
fn run_cwd_then_inner(
    cell: &Rc<RefCell<Shell>>,
    cwd: Option<&std::path::Path>,
    restricted: bool,
    src: &str,
    out: &mut StdoutSink,
    err: &mut StderrSink,
) -> i32 {
    match cwd {
        Some(p) => run_cwd_inner(cell, p, restricted, src, out, err),
        None => run_restricted_then_inner(cell, restricted, src, out, err),
    }
}

/// Acquire the `with_cwd` RAII guard, then run the inner script. We must drop
/// the outer `RefMut<Shell>` before `with_cwd` calls its closure `f()` —
/// otherwise the inner `run_restricted_then_inner` would panic on
/// `cell.borrow_mut()` (RefCell runtime check).
///
/// Strategy: cast the `RefMut`'s `&mut Shell` to a raw pointer, then drop the
/// `RefMut`. The raw pointer remains valid because the `Rc<RefCell<Shell>>`
/// is still alive (we have `&cell`). `with_cwd`'s prologue uses the `&mut
/// Shell` immediately and synchronously, then calls `f()`; we never use the
/// pointer again from the outer scope. `with_cwd`'s own `Restore` Drop guard
/// stashes its own raw pointer to the same `Shell` and writes through it
/// after `f` returns — by then no `RefMut` is outstanding (the inner code's
/// borrows have all been released on its `run_restricted_then_inner` return),
/// so the write is sound.
fn run_cwd_inner(
    cell: &Rc<RefCell<Shell>>,
    path: &std::path::Path,
    restricted: bool,
    src: &str,
    out: &mut StdoutSink,
    err: &mut StderrSink,
) -> i32 {
    let shell_ptr: *mut Shell = {
        let mut refmut = cell.borrow_mut();
        // SAFETY: refmut yields a &mut Shell pointing into the RefCell's
        // contents; that memory remains valid for as long as `cell` is alive.
        let ptr: *mut Shell = &mut *refmut;
        // Drop the RefMut so the inner code path can borrow_mut() again.
        drop(refmut);
        ptr
    };
    // SAFETY: see the function-level doc-comment. The pointer is used twice:
    //   (1) here, synchronously inside with_cwd's prologue (before f() runs);
    //   (2) inside with_cwd's Restore Drop guard, after f() has returned and
    //       all inner RefMut borrows are gone.
    // No other &mut Shell exists during either window.
    let shell_mut: &mut Shell = unsafe { &mut *shell_ptr };
    crate::cwd_scope::with_cwd(path, shell_mut, || {
        run_restricted_then_inner(cell, restricted, src, out, err)
    })
}

/// Snapshot+set `Shell.restricted`, run the inner script, restore on exit (RAII).
fn run_restricted_then_inner(
    cell: &Rc<RefCell<Shell>>,
    restricted: bool,
    src: &str,
    out: &mut StdoutSink,
    err: &mut StderrSink,
) -> i32 {
    let prev_restricted = cell.borrow().restricted;
    cell.borrow_mut().restricted = restricted || prev_restricted;
    struct R<'c> {
        cell: &'c Rc<RefCell<Shell>>,
        prev: bool,
    }
    impl Drop for R<'_> {
        fn drop(&mut self) {
            self.cell.borrow_mut().restricted = self.prev;
        }
    }
    let _r = R { cell, prev: prev_restricted };

    let label = cell.borrow().shell_argv0.clone();
    let args = cell.borrow().positional_args.clone();
    let code = crate::shell::run_program_in_sinks(
        src, None, args, &label, false, out, err, cell,
    );
    cell.borrow_mut().set_last_status(code);
    code
}
