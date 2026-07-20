//! `ExecBuilder` — per-call builder for [`Engine::prepare`].
//!
//! Holds the script source + optional stdin bytes + merge flag + sandbox knobs
//! (cwd / restricted / timeout), and runs them through the engine's sink-aware
//! path on `.run()` / `.capture()`.
//!
//! [`Engine::prepare`]: crate::engine::Engine::prepare

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use crate::engine::{Engine, Output};
use crate::executor::{StderrSink, StdoutSink};
use crate::shell_state::Shell;

type LineCallback<'a> = Box<dyn FnMut(&str) + 'a>;

/// Streaming callbacks owned by the builder for the call's duration.
/// `'cb` is the builder's lifetime — closures may borrow caller state for
/// that duration.
pub(crate) struct Callbacks<'cb> {
    pub stdout: Option<LineCallback<'cb>>,
    pub stderr: Option<LineCallback<'cb>>,
    pub line_buf_out: crate::line_buf::LineBuf,
    pub line_buf_err: crate::line_buf::LineBuf,
    /// If `Some`, after dispatching each complete line to the stdout
    /// callback, re-write `line\n` to this fd (tee). Set under `.run()` when
    /// callbacks are present and stdout was meant to inherit.
    pub tee_stdout_fd: Option<std::os::fd::RawFd>,
    pub tee_stderr_fd: Option<std::os::fd::RawFd>,
}

impl<'cb> Callbacks<'cb> {
    pub fn new(stdout: Option<LineCallback<'cb>>, stderr: Option<LineCallback<'cb>>) -> Self {
        Self {
            stdout,
            stderr,
            line_buf_out: crate::line_buf::LineBuf::new(),
            line_buf_err: crate::line_buf::LineBuf::new(),
            tee_stdout_fd: None,
            tee_stderr_fd: None,
        }
    }

    pub fn any_set(&self) -> bool {
        self.stdout.is_some()
            || self.stderr.is_some()
            || self.tee_stdout_fd.is_some()
            || self.tee_stderr_fd.is_some()
    }

    /// Push raw stdout bytes; dispatch any complete lines via the stdout
    /// callback, then re-write `line\n` to the tee fd (if set).
    pub fn push_stdout(&mut self, bytes: &[u8]) {
        if self.stdout.is_none() && self.tee_stdout_fd.is_none() {
            return;
        }
        self.line_buf_out.push(bytes);
        while let Some(line) = self.line_buf_out.next_line() {
            if let Some(cb) = &mut self.stdout {
                cb(&line);
            }
            if let Some(fd) = self.tee_stdout_fd {
                let bytes = line.as_bytes();
                unsafe {
                    libc::write(fd, bytes.as_ptr() as *const _, bytes.len());
                    libc::write(fd, b"\n".as_ptr() as *const _, 1);
                }
            }
        }
    }

    /// Push raw stderr bytes; dispatch any complete lines via the stderr
    /// callback, then re-write `line\n` to the tee fd (if set).
    pub fn push_stderr(&mut self, bytes: &[u8]) {
        if self.stderr.is_none() && self.tee_stderr_fd.is_none() {
            return;
        }
        self.line_buf_err.push(bytes);
        while let Some(line) = self.line_buf_err.next_line() {
            if let Some(cb) = &mut self.stderr {
                cb(&line);
            }
            if let Some(fd) = self.tee_stderr_fd {
                let bytes = line.as_bytes();
                unsafe {
                    libc::write(fd, bytes.as_ptr() as *const _, bytes.len());
                    libc::write(fd, b"\n".as_ptr() as *const _, 1);
                }
            }
        }
    }

    /// Flush partial-at-EOF lines for both streams. No trailing `\n` is added
    /// to the tee fd for the partial-at-EOF case (the original bytes had none).
    pub fn flush_partials(&mut self) {
        if let Some(line) = self.line_buf_out.drain_final() {
            if let Some(cb) = self.stdout.as_mut() {
                cb(&line);
            }
            if let Some(fd) = self.tee_stdout_fd {
                let bytes = line.as_bytes();
                unsafe {
                    libc::write(fd, bytes.as_ptr() as *const _, bytes.len());
                }
            }
        }
        if let Some(line) = self.line_buf_err.drain_final() {
            if let Some(cb) = self.stderr.as_mut() {
                cb(&line);
            }
            if let Some(fd) = self.tee_stderr_fd {
                let bytes = line.as_bytes();
                unsafe {
                    libc::write(fd, bytes.as_ptr() as *const _, bytes.len());
                }
            }
        }
    }
}

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
    cwd: Option<PathBuf>,
    restricted: bool,
    timeout: Option<Duration>,
    on_stdout_line: Option<LineCallback<'a>>,
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

    /// Enable restricted mode for this call (bash `rbash` subset: refuses
    /// `cd`, `exec`, command-names containing `/`, `source` of paths
    /// containing `/`, write-redirects to absolute or `..` paths, and
    /// `set +r`). Refused operations emit a diagnostic via the active stderr
    /// sink and return a non-zero exit; the script keeps running unless
    /// `set -e` propagates the failure.
    ///
    /// # The protected variables are marked readonly, and that OUTLIVES the call
    ///
    /// Entering a restricted policy marks `SHELL`, `PATH`, `HISTFILE`, `ENV`
    /// and `BASH_ENV` readonly, so every write path (plain assignment,
    /// `export`, `read`, `declare`, `unset`, `+=`) reports through ordinary
    /// readonly machinery as `<name>: readonly variable` rather than a
    /// restriction-specific message.
    ///
    /// Those readonly marks are **deliberately not undone** when the call
    /// ends — matching bash, where restricted mode is one-way and cannot be
    /// unset from within the shell. A later, *unrestricted* call on the same
    /// `Engine` therefore still sees them:
    ///
    /// ```text
    /// e.prepare("echo hi").restricted().capture();   // marks PATH readonly
    /// e.prepare("PATH=/usr/bin").capture();          // PATH: readonly variable
    /// ```
    ///
    /// If an embedder needs an unrestricted shell afterwards, construct a
    /// fresh `Engine` rather than reusing this one.
    pub fn restricted(mut self) -> Self {
        self.restricted = true;
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
    ///
    /// When no line callbacks are set, this takes the fast path: fd 1/2
    /// inherit directly, no pipe interposition. When a callback is set, we
    /// save real fd 1 (and fd 2 if `.on_stderr_line` is set) via `dup()`,
    /// route the script's output through Capture sinks so we can line-buffer
    /// for dispatch, then tee each complete line back to the saved real fds
    /// after the callback returns — so the embedder still sees the script's
    /// output on their terminal AND the callback fires per line.
    pub fn run(self) -> i32 {
        let any_cb = self.on_stdout_line.is_some() || self.on_stderr_line.is_some();
        if !any_cb {
            // FAST PATH: no callbacks; fd 1/2 inherit, no pipe interposition.
            let mut out = StdoutSink::Terminal;
            let mut err = if self.merge {
                StderrSink::Merged
            } else {
                StderrSink::Terminal
            };
            return self.run_with_sinks(&mut out, &mut err);
        }

        // TEE PATH: save real fd 1 (always) and fd 2 (if non-merge), route
        // through Capture sinks, ask Callbacks to re-write each line to the
        // saved fd after dispatch.
        let saved_stdout_fd = unsafe { libc::dup(1) };
        let saved_stderr_fd = unsafe { libc::dup(2) };
        if saved_stdout_fd < 0 || saved_stderr_fd < 0 {
            // Dup failure: fall back to the fast path (no tee, no callback).
            // The script still runs; the embedder still sees output.
            if saved_stdout_fd >= 0 {
                unsafe {
                    libc::close(saved_stdout_fd);
                }
            }
            if saved_stderr_fd >= 0 {
                unsafe {
                    libc::close(saved_stderr_fd);
                }
            }
            let mut out = StdoutSink::Terminal;
            let mut err = if self.merge {
                StderrSink::Merged
            } else {
                StderrSink::Terminal
            };
            return self.run_with_sinks(&mut out, &mut err);
        }

        let merge = self.merge;
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let code = {
            let mut out = StdoutSink::Capture(&mut buf_out);
            let tee_out = Some(saved_stdout_fd);
            // Under merge, fd 2 is dup2'd onto fd 1 at the executor level; the
            // tee for stderr would double-write — skip it. (Tee for stdout
            // still fires; that's where merged output lands.)
            let tee_err = if merge { None } else { Some(saved_stderr_fd) };
            if merge {
                let mut err = StderrSink::Merged;
                self.run_with_sinks_tee(&mut out, &mut err, tee_out, tee_err)
            } else {
                let mut err = StderrSink::Capture(&mut buf_err);
                self.run_with_sinks_tee(&mut out, &mut err, tee_out, tee_err)
            }
        };

        unsafe {
            libc::close(saved_stdout_fd);
            libc::close(saved_stderr_fd);
        }
        code
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
        self.run_with_sinks_inner(out, err, None, None)
    }

    fn run_with_sinks_tee(
        self,
        out: &mut StdoutSink,
        err: &mut StderrSink,
        tee_stdout_fd: Option<std::os::fd::RawFd>,
        tee_stderr_fd: Option<std::os::fd::RawFd>,
    ) -> i32 {
        self.run_with_sinks_inner(out, err, tee_stdout_fd, tee_stderr_fd)
    }

    fn run_with_sinks_inner(
        self,
        out: &mut StdoutSink,
        err: &mut StderrSink,
        tee_stdout_fd: Option<std::os::fd::RawFd>,
        tee_stderr_fd: Option<std::os::fd::RawFd>,
    ) -> i32 {
        let ExecBuilder {
            engine,
            src,
            stdin,
            merge: _,
            cwd,
            restricted,
            timeout,
            on_stdout_line,
            on_stderr_line,
        } = self;
        let cell = engine.shell_cell().clone();

        // Build callbacks; install thread-local pointer for the duration of
        // the run if any callback or tee fd was set. (See `callbacks_thread_local`.)
        let mut callbacks = Callbacks::new(on_stdout_line, on_stderr_line);
        callbacks.tee_stdout_fd = tee_stdout_fd;
        callbacks.tee_stderr_fd = tee_stderr_fd;
        let any_callbacks = callbacks.any_set();

        let code = {
            // SAFETY: `callbacks` lives until the end of run_with_sinks; the
            // guard's Drop runs before this scope exits, clearing the pointer.
            let _cb_guard = if any_callbacks {
                Some(unsafe { crate::callbacks_thread_local::install(&mut callbacks) })
            } else {
                None
            };

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
                Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, &cell, || {
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
                124
            } else {
                code
            }
        };

        // After the run (and after the guard's Drop has cleared the
        // thread-local), flush any partial-at-EOF lines through the
        // user-supplied callbacks.
        if any_callbacks {
            callbacks.flush_partials();
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

/// Snapshot+set `Shell.policy`, run the inner script, restore on exit (RAII).
///
/// The restore puts back the previous POLICY only. It deliberately does NOT
/// unmark the variables `apply_restricted_readonly` made readonly: restriction
/// is one-way for the variables it protects, so a shell that has once been
/// restricted never regains writability of SHELL/PATH/HISTFILE/ENV/BASH_ENV.
/// bash behaves the same way (its `set -r` marks are permanent for the shell's
/// life). This is intentional, not a leak.
fn run_restricted_then_inner(
    cell: &Rc<RefCell<Shell>>,
    restricted: bool,
    src: &str,
    out: &mut StdoutSink,
    err: &mut StderrSink,
) -> i32 {
    let prev_policy = cell.borrow().policy;
    if restricted {
        let mut sh = cell.borrow_mut();
        sh.policy = crate::policy::Policy::Sandbox;
        sh.apply_restricted_readonly();
    }
    struct R<'c> {
        cell: &'c Rc<RefCell<Shell>>,
        prev: crate::policy::Policy,
    }
    impl Drop for R<'_> {
        fn drop(&mut self) {
            // Policy only — see the fn doc on why the readonly marks stay.
            self.cell.borrow_mut().policy = self.prev;
        }
    }
    let _r = R {
        cell,
        prev: prev_policy,
    };

    let label = cell.borrow().shell_argv0.clone();
    let args = cell.borrow().positional_args.clone();
    let code = crate::shell::run_program_in_sinks(src, None, args, &label, false, out, err, cell);
    cell.borrow_mut().set_last_status(code);
    code
}
