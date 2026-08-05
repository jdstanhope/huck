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
use crate::shell_state::Shell;

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
    cwd: Option<PathBuf>,
    restricted: bool,
    timeout: Option<Duration>,
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

    /// Enable restricted mode for this call by selecting `Policy::Sandbox`
    /// (`policy.rs`) — huck's embedding policy, distinct from bash's `rbash`
    /// (`Policy::Rbash`, reachable via `-r` / `set -r` / `argv[0] == "rbash"`).
    /// `Sandbox` denies `cd`, `exec`, command names containing `/`, `source`
    /// of paths containing `/`, and `set +r`, same as `Rbash`. It differs from
    /// `Rbash` in exactly one place: a file-target write-redirect is denied
    /// only when the target **escapes the working directory** (an absolute
    /// path, or one with a `..` component) — a relative write like `> out.txt`
    /// stays permitted, so a hosted script can still do local work, while
    /// `Rbash` denies every file-target redirect regardless of path shape.
    /// Refused operations emit a diagnostic via the active stderr sink and
    /// return a non-zero exit; the script keeps running unless `set -e`
    /// propagates the failure. See
    /// `docs/superpowers/specs/2026-07-20-restricted-policy-design.md` for the
    /// full design and the bash-vs-huck rationale.
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

    /// Run the script; fd 1 and fd 2 inherit (or merged-to-fd1 if `merge_stderr`).
    ///
    /// fd 1/2 inherit directly — no pipe interposition, no capture.
    ///
    /// Under `merge_stderr` the merge is a REAL `dup2(1, 2)` at the fd level
    /// (saved + restored around the run), not the software `Merged` sink — so
    /// `run()` is single-model (#197 Stage 3). Both streams reach the embedder's
    /// fd 1 in program order, exactly as `bash 2>&1` would.
    pub fn run(self) -> i32 {
        use std::io::Write;
        if !self.merge {
            return self.run_with_sinks();
        }
        // merge: flush, save fd 2, point it at fd 1, run, restore.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        let saved2 = unsafe { libc::dup(2) };
        if saved2 < 0 {
            // dup failed — fall back to running unmerged rather than aborting.
            return self.run_with_sinks();
        }
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        // The owned save of fd 2: Drop restores it, then the OwnedFd closes it —
        // no manual close (#197 Class-A).
        struct Fd2Restore(OwnedFd);
        impl Drop for Fd2Restore {
            fn drop(&mut self) {
                unsafe {
                    libc::dup2(self.0.as_raw_fd(), 2);
                }
            }
        }
        let _restore = Fd2Restore(unsafe { OwnedFd::from_raw_fd(saved2) });
        unsafe {
            libc::dup2(1, 2);
        }
        let code = self.run_with_sinks();
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        code
    }

    /// Run the script; capture fd 1 and fd 2 into `Output`.
    ///
    /// Sink-free (Stage 2, #197): rather than routing output through in-memory
    /// `Capture` sinks, we redirect the process's *real* fd 1 (and fd 2) to a
    /// temp file, run the script with `Terminal` sinks, restore fd 1/2, and read
    /// the file(s) back. This makes ALL output — builtins, externals, and forked
    /// command-substitution children — land in the capture at the real-fd level,
    /// in program order, exactly as a `.run()` to a terminal would produce it.
    ///
    /// Under `merge_stderr`, fd 2 is dup2'd onto the same temp file as fd 1, so
    /// both streams interleave in the one file; `Output.stderr` is then empty.
    ///
    /// The lib test build uses the in-memory `capture()` below (parallel-safe);
    /// production uses this temp-file path. Stage 3 (#197) migrates those tests +
    /// deletes the `Capture`/`Merged` sinks. The temp-file path is exercised by
    /// the `capture_tempfile_serial` integration binary (which builds the
    /// non-test lib).
    #[cfg(not(test))]
    pub fn capture(self) -> Output {
        use std::io::{Read, Seek, Write};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        // Read a NamedTempFile from the start into a lossy String.
        fn read_back(mut f: tempfile::NamedTempFile) -> String {
            let file = f.as_file_mut();
            if file.rewind().is_err() {
                return String::new();
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_err() {
                return String::new();
            }
            String::from_utf8_lossy(&buf).into_owned()
        }

        let empty = || Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
        };

        let merge = self.merge;

        // No lock needed: production `Engine` use is single-threaded (the
        // exec_guard invariant), so no two captures redirect fd 1/2 at once.

        // 1. Create the temp file(s): one under merge, else one per stream.
        let outfile = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(_) => return empty(),
        };
        let errfile = if merge {
            None
        } else {
            match tempfile::NamedTempFile::new() {
                Ok(f) => Some(f),
                Err(_) => return empty(),
            }
        };
        let outfd = outfile.as_raw_fd();
        let errfd = errfile.as_ref().map(|f| f.as_raw_fd());

        // 2. Flush any Rust-buffered bytes on the process streams BEFORE we
        // redirect, so prior output is not diverted into the capture.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();

        // 3. Save the real fd 1 and fd 2.
        let saved1 = unsafe { libc::dup(1) };
        let saved2 = unsafe { libc::dup(2) };
        if saved1 < 0 || saved2 < 0 {
            // Own whichever dup succeeded so its drop closes it (#197 Class-A);
            // a -1 (failed dup) is not owned and is left alone.
            if saved1 >= 0 {
                drop(unsafe { OwnedFd::from_raw_fd(saved1) });
            }
            if saved2 >= 0 {
                drop(unsafe { OwnedFd::from_raw_fd(saved2) });
            }
            return empty();
        }
        // Own both saves: their Drop closes them exactly once (#197 Class-A).
        let saved1 = unsafe { OwnedFd::from_raw_fd(saved1) };
        let saved2 = unsafe { OwnedFd::from_raw_fd(saved2) };

        // RAII guard: restore fd 1/2 from the saved dups on EVERY exit path
        // (normal return or panic unwind); the owned saves then close themselves.
        struct FdRestore {
            saved1: OwnedFd,
            saved2: OwnedFd,
        }
        impl Drop for FdRestore {
            fn drop(&mut self) {
                unsafe {
                    libc::dup2(self.saved1.as_raw_fd(), 1);
                    libc::dup2(self.saved2.as_raw_fd(), 2);
                }
            }
        }
        let _restore = FdRestore { saved1, saved2 };

        // 4. Redirect fd 1 (and fd 2) onto the temp file(s). Under merge both
        // fds share one open file description, so writes interleave in order.
        unsafe {
            libc::dup2(outfd, 1);
            match errfd {
                Some(fd) => {
                    libc::dup2(fd, 2);
                }
                None => {
                    // merge: fd 2 -> the same temp file as fd 1.
                    libc::dup2(outfd, 2);
                }
            }
        }

        // 5. Run the script with Terminal sinks — output flows to real fd 1/2,
        // now pointing at the temp file(s).
        let ExecBuilder {
            engine,
            src,
            stdin,
            merge: _,
            cwd,
            restricted,
            timeout,
        } = self;
        let cell = engine.shell_cell().clone();
        let exit_code = run_core(&cell, &src, stdin, cwd, restricted, timeout);

        // 6. Flush any Rust-buffered stdout/stderr into the redirected fds, then
        // restore fd 1/2 (dropping the guard) before reading the file(s) back.
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        drop(_restore);

        // 7. Read the temp file(s) back. Under merge, all bytes are in outfile
        // and stderr is empty (matching the prior merge behavior).
        let stdout = read_back(outfile);
        let stderr = match errfile {
            Some(f) => read_back(f),
            None => String::new(),
        };
        Output {
            stdout,
            stderr,
            exit_code,
        }
    }

    /// Lib unit-test build: the multithreaded `--lib` test binary runs many
    /// tests concurrently, so the production temp-file `capture()` — which
    /// redirects the *process-global* fd 1/2 — would race libtest's own reporter
    /// (its `test … ok` writes land on the redirected descriptor and corrupt the
    /// capture; see `crates/huck-engine/tests/streaming_fd_serial.rs`). The
    /// in-memory `Capture`/`Merged` sinks never touch global fd 1/2, so they are
    /// parallel-safe. This is behavior-identical to the temp-file path for the
    /// output it produces, and the temp-file path is exercised end-to-end by the
    /// `capture_tempfile_serial` integration binary (built non-test). Stage 3
    /// (#197) migrates these tests to that binary and deletes the sinks.
    #[cfg(test)]
    pub fn capture(self) -> Output {
        let merge = self.merge;
        // Stage 3 (#197): run with `Terminal` sinks under a `capture_test_hook`
        // thread-local capture, which intercepts in-process (builtin) stdout/stderr
        // at the writer chokepoints. `merge` folds captured stderr into stdout
        // (leaving `Output.stderr` empty); otherwise the two streams are captured
        // separately. Externals (forked children) are NOT captured here — those
        // tests live in `capture_tempfile_serial` / `comsub_merge_stderr_diff_check`.
        let (buf_out, buf_err, exit_code) =
            crate::capture_test_hook::with_capture(merge, true, || self.run_with_sinks());
        Output {
            stdout: String::from_utf8_lossy(&buf_out).into_owned(),
            stderr: String::from_utf8_lossy(&buf_err).into_owned(),
            exit_code,
        }
    }

    fn run_with_sinks(self) -> i32 {
        let ExecBuilder {
            engine,
            src,
            stdin,
            merge: _,
            cwd,
            restricted,
            timeout,
        } = self;
        let cell = engine.shell_cell().clone();
        run_core(&cell, &src, stdin, cwd, restricted, timeout)
    }
}

/// Core run composition shared by `capture()` and `run_with_sinks`:
/// spawn the timeout timer (if any), install the stdin pipe (if any), apply the
/// cwd/restricted guards, run the script into the given sinks, then convert a
/// fired timeout into exit 124.
fn run_core(
    cell: &Rc<RefCell<Shell>>,
    src: &str,
    stdin: Option<Vec<u8>>,
    cwd: Option<PathBuf>,
    restricted: bool,
    timeout: Option<Duration>,
) -> i32 {
    // 1. Spawn timer (if requested). Defend against a prior call leaving the
    // timeout_flag set.
    let timer = timeout.map(|dur| {
        let flag = cell.borrow().timeout_flag.clone();
        let pids = cell.borrow().live_external_children.clone();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::timeout::spawn_timer(dur, flag, pids)
    });

    // 2. Compose stdin -> cwd -> restricted+run via nested matches.
    let code = match stdin {
        Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, cell, || {
            run_cwd_then_inner(cell, cwd.as_deref(), restricted, src)
        }),
        None => run_cwd_then_inner(cell, cwd.as_deref(), restricted, src),
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
        // #442: an `exit N` performed by a trap action decides the status. The
        // EXIT trap has already fired inside the run above, so any override it
        // requested is the one recorded here. A timeout still outranks it.
        cell.borrow_mut().take_exit().unwrap_or(code)
    }
}

/// Apply the cwd guard (if set), then run the restricted+inner core.
fn run_cwd_then_inner(
    cell: &Rc<RefCell<Shell>>,
    cwd: Option<&std::path::Path>,
    restricted: bool,
    src: &str,
) -> i32 {
    match cwd {
        Some(p) => run_cwd_inner(cell, p, restricted, src),
        None => run_restricted_then_inner(cell, restricted, src),
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
        run_restricted_then_inner(cell, restricted, src)
    })
}

/// Snapshot+set `Shell.policy`, run the inner script, restore on exit (RAII).
///
/// Restriction is ONE-WAY, and the restore honors that in two ways:
///
/// * It does NOT unmark the variables `apply_restricted_readonly` made
///   readonly: a shell that has once been restricted never regains
///   writability of SHELL/PATH/HISTFILE/ENV/BASH_ENV. bash behaves the same
///   way (its `set -r` marks are permanent for the shell's life).
/// * It puts the previous policy back only when the policy is still the one
///   this guard installed. If the INNER script raised it — `set -r` — that
///   elevation survives the guard, exactly as the readonly marks do.
///   Reverting it would leave a half-restricted shell (`cd` permitted again
///   while PATH stays readonly) that bash never produces.
///
/// This is intentional, not a leak.
fn run_restricted_then_inner(cell: &Rc<RefCell<Shell>>, restricted: bool, src: &str) -> i32 {
    let prev_policy = cell.borrow().policy;
    let prev_startup = cell.borrow().restricted_at_startup;
    if restricted {
        let mut sh = cell.borrow_mut();
        sh.policy = crate::policy::Policy::Sandbox;
        // An invocation-time choice by the embedder, analogous to `-r` rather
        // than to `set -r` — so `shopt restricted_shell` reports `on`.
        sh.restricted_at_startup = true;
        sh.apply_restricted_readonly();
    }
    // The policy in effect once the prologue is done. If the inner script has
    // moved away from it by the time we unwind, it raised the policy itself
    // (`set -r`) and that elevation must survive — see the fn doc.
    let installed_policy = cell.borrow().policy;
    struct R<'c> {
        cell: &'c Rc<RefCell<Shell>>,
        installed: crate::policy::Policy,
        prev: crate::policy::Policy,
        prev_startup: bool,
    }
    impl Drop for R<'_> {
        fn drop(&mut self) {
            // Policy + provenance only — see the fn doc on why the readonly
            // marks stay, and why an inner-script elevation is left alone.
            let mut sh = self.cell.borrow_mut();
            if sh.policy == self.installed {
                sh.policy = self.prev;
                sh.restricted_at_startup = self.prev_startup;
            }
        }
    }
    let _r = R {
        cell,
        installed: installed_policy,
        prev: prev_policy,
        prev_startup,
    };

    let label = cell.borrow().shell_argv0.clone();
    let args = cell.borrow().positional_args.clone();
    let code = crate::shell::run_program_in_sinks(src, None, args, &label, false, cell);
    cell.borrow_mut().set_last_status(code);
    code
}
