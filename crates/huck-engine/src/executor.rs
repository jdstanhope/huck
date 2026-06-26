use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::io::RawFd;
use std::process::{Command as ProcessCommand, Stdio};

use crate::builtins::{self, ExecOutcome, InterruptReason};
use crate::command::{
    CaseClause, CaseItem, CaseTerminator, Command, Connector, ExecCommand, FileMode, ForClause,
    IfClause, Pipeline, Redirect, RedirFd, RedirOp, Redirection, Sequence, SimpleCommand, TestBinaryOp,
    TestExpr, TestUnaryOp, WhileClause,
};
use crate::expand::{expand, expand_assignment, expand_pattern, glob_expand_fields_opts};
use crate::shell_state::Shell;

/// `pgid_target` sentinel for the fork primitives meaning "do not `setpgid` —
/// inherit the shell's process group" (job control off). `0` = become a new
/// group leader; `N > 0` = join group `N`.
const NO_PGROUP: i32 = -1;

/// Where the terminal stage of a top-level pipeline sends its stdout when
/// there's no explicit `> file` redirect.
pub enum StdoutSink<'a> {
    Terminal,
    Capture(&'a mut Vec<u8>),
}

/// Where the active "errored" output stream goes. Symmetric to `StdoutSink`,
/// except for the extra `Merged` variant which routes stderr writes through
/// the active stdout writer (the `2>&1` analog).
pub enum StderrSink<'a> {
    Terminal,
    Merged,
    Capture(&'a mut Vec<u8>),
}

/// Materialize a `Box<dyn Write>` for the active `StderrSink`, with `out_sink`
/// supplied so `Merged` can route through the active stdout writer. Allocates
/// per call site — stderr is best-effort and small, so the heap hit is fine.
/// Each call-site brace-scopes the writer to release the `err_sink` / `sink`
/// borrows before subsequent code runs (`{ let mut err = err_writer(...); e!(...) }`).
/// Restricted-mode gate for a write-style redirect target path. Returns
/// `Err(())` after emitting the diagnostic when the shell is in restricted
/// mode, the `mode` is write-style (Truncate/Append/Clobber/ReadWrite), and
/// the path is absolute or contains a `..` component. Input-only modes
/// (`ReadOnly`) are NEVER refused.
#[inline]
fn check_restricted_redirect(
    mode: &FileMode,
    path: &str,
    shell: &Shell,
    sink: &mut StdoutSink<'_>,
    err_sink: &mut StderrSink<'_>,
) -> Result<(), ()> {
    if !crate::restricted::is_restricted(shell) {
        return Ok(());
    }
    if !matches!(
        mode,
        FileMode::Truncate | FileMode::Append | FileMode::Clobber | FileMode::ReadWrite
    ) {
        return Ok(());
    }
    if let Err(msg) = crate::restricted::check_redirect_path(path) {
        let mut err = err_writer(err_sink, sink);
        e!(&mut *err, "{msg}");
        return Err(());
    }
    Ok(())
}

/// Stream identifier for `LineDispatchWriter`.
#[derive(Clone, Copy)]
pub(crate) enum LineStream {
    Stdout,
    Stderr,
}

/// Writer that wraps an inner `Vec<u8>` AND notifies the active callbacks
/// thread-local of any bytes written, so streaming line callbacks fire as
/// builtins write to their capture buffer.
pub(crate) struct LineDispatchWriter<'a> {
    pub inner: &'a mut Vec<u8>,
    pub stream: LineStream,
}

impl std::io::Write for LineDispatchWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.extend_from_slice(bytes);
        let stream = self.stream;
        crate::callbacks_thread_local::with_callbacks(|cb| {
            if let Some(cb) = cb {
                match stream {
                    LineStream::Stdout => cb.push_stdout(bytes),
                    LineStream::Stderr => cb.push_stderr(bytes),
                }
            }
        });
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn err_writer<'a>(
    err_sink: &'a mut StderrSink<'_>,
    out_sink: &'a mut StdoutSink<'_>,
) -> Box<dyn std::io::Write + 'a> {
    match err_sink {
        StderrSink::Terminal => Box::new(std::io::stderr()),
        StderrSink::Capture(buf) => Box::new(LineDispatchWriter {
            inner: buf,
            stream: LineStream::Stderr,
        }),
        StderrSink::Merged => match out_sink {
            StdoutSink::Terminal => Box::new(std::io::stdout()),
            StdoutSink::Capture(buf) => Box::new(LineDispatchWriter {
                inner: buf,
                stream: LineStream::Stdout,
            }),
        },
    }
}

/// Flush huck's buffered stdout (Rust wraps fd 1 in a `LineWriter`, so a trailing
/// partial line is held back) before handing fd 1 to another process. A fork
/// child would otherwise inherit — and possibly duplicate — the pending bytes,
/// and a spawned peer would otherwise race ahead of them. Call at every fork/spawn
/// handoff. `io::stderr()` is unbuffered, so it needs no equivalent.
fn flush_stdout() {
    let _ = io::stdout().flush();
}

/// Called after a simple command's status is set. If errexit is on, the
/// status is non-zero, and we're not in an err-suppressed context (matches
/// v36's ERR-trap gate), returns the Exit outcome to terminate the shell
/// with that status. Caller propagates the outcome with an early return.
fn maybe_errexit(shell: &Shell, status: i32) -> Option<ExecOutcome> {
    if shell.shell_options.errexit
        && shell.err_suppressed_depth == 0
        && status != 0
    {
        Some(ExecOutcome::Exit(status))
    } else {
        None
    }
}

/// RAII guard that pops a pid from the `Shell::live_external_children` registry
/// when the surrounding scope ends — including early returns and panics. Pushed
/// at every external-fork site so the timeout timer thread (see `timeout.rs`)
/// has an accurate snapshot of which children to SIGTERM if the deadline fires.
struct LiveChildGuard<'a> {
    pids: &'a std::sync::Arc<std::sync::Mutex<Vec<libc::pid_t>>>,
    pid: libc::pid_t,
}

impl Drop for LiveChildGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut g) = self.pids.lock() {
            // Remove the first match — multiple distinct pids can coexist in the
            // registry (e.g. concurrent pipeline stages), but the same pid
            // appears at most once for a given live child.
            if let Some(idx) = g.iter().position(|&p| p == self.pid) {
                g.swap_remove(idx);
            }
        }
    }
}

/// Consumes a pending SIGINT and decides whether to abort. Returns
/// `Some(ExecOutcome::Interrupted(InterruptReason::Sigint))` when an untrapped
/// SIGINT is pending; `None` when none is pending OR when a user `INT` trap
/// (handler or ignore-form) is installed — the existing trap dispatch then
/// handles it and execution continues, matching bash. (v138)
pub(crate) fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;
    if shell
        .sigint_flag
        .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        if shell.trap_sigids.contains_key(&libc::SIGINT) {
            return None;
        }
        return Some(ExecOutcome::Interrupted(InterruptReason::Sigint));
    }
    // Timeout poll. The flag stays set — the builder's epilogue does a single
    // `swap(false)` at the run boundary to override the exit code to 124.
    if shell.timeout_flag.load(Ordering::Relaxed) {
        return Some(ExecOutcome::Interrupted(InterruptReason::Timeout));
    }
    None
}

/// Runs a top-level sequence, sending the terminal pipeline-stage's stdout to
/// the given `sink`. `execute` is the Terminal-sink wrapper; command
/// substitution / `$()` capture supply a `Capture` sink so a captured `eval`
/// or `source` (via the `*_in_sink` plumbing) lands in the right buffer.
pub fn execute_with_sink(
    seq: &Sequence,
    shell: &mut Shell,
    source: &str,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // Install the sinks as the thread-local err sinks so deep call chains
    // (`expand`, `param_expansion`, `Shell` methods, `jobs`) route their
    // diagnostics through `with_err` to the active sink. The Guard inside
    // `install_err_sinks` clears the pointer on scope exit (including panic).
    // SAFETY contract: see `err_thread_local` module docs. We use the unsafe
    // raw-pointer-install variant so the executor body can keep using its own
    // `&mut sink`/`&mut err_sink` directly (the thread-local is consulted only
    // by `with_err` in tight, leaf scopes).
    let guard = unsafe { crate::err_thread_local::install_err_sinks_raw(sink, err_sink) };
    let r = execute_with_sink_inner(seq, shell, source, sink, err_sink);
    drop(guard);
    r
}

fn execute_with_sink_inner(
    seq: &Sequence,
    shell: &mut Shell,
    source: &str,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // Fast path: a trailing-`&` that backgrounds a SINGLE and-or group (no
    // `&`-separators inside the list). This preserves the real source-derived
    // job-display label for the common `cmd &` / `a && b &` / `a | b &` forms.
    // Sequences containing `Connector::Amp` (mid-list `&` separators) fall
    // through to the group-aware `execute_sequence_body`.
    let has_amp_separator = seq.rest.iter().any(|(c, _)| matches!(c, Connector::Amp));
    if seq.background && !has_amp_separator {
        if seq.rest.is_empty() {
            // Single-pipeline or subshell backgrounded — existing paths.
            if let Command::Pipeline(p) = &seq.first {
                return run_background_sequence(p, shell, sink, err_sink, source);
            }
            if let Command::Subshell { .. } = &seq.first {
                return run_background_subshell(&seq.first, shell, sink, err_sink, source);
            }
        } else if seq
            .rest
            .iter()
            .all(|(c, _)| matches!(c, Connector::And | Connector::Or))
        {
            // A trailing-`&` backgrounding a single and-or group spanning
            // multiple stages (`a && b &`, `a || b &`). Wrap the whole group
            // in a Subshell and background it (bash-correct: the whole group
            // runs in the child). Groups separated by `;` are NOT collapsed
            // here — those go through execute_sequence_body so only the LAST
            // group is backgrounded.
            let inner = Sequence {
                first: seq.first.clone(),
                rest: seq.rest.clone(),
                background: false,
            };
            let subshell = Command::Subshell { body: Box::new(inner) };
            return run_background_subshell(&subshell, shell, sink, err_sink, source);
        }
    }
    execute_sequence_body(seq, shell, sink, err_sink)
}

/// Runs a top-level sequence with stdout going to the terminal. Thin wrapper
/// over `execute_with_sink` with a Terminal sink.
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    let mut err_sink = StderrSink::Terminal;
    execute_with_sink(seq, shell, source, &mut sink, &mut err_sink)
}

/// Runs a sequence with stdout captured to a buffer. Used by command
/// substitution; the trailing `&` is ignored here because substitutions
/// must complete before their output is interpolated. Spawning real
/// background children whose pids the parent's JobTable doesn't track
/// would let them escape `wait`/`jobs` and litter the terminal.
///
/// Streaming-callback contract (v207 fixup): command substitution captures
/// inner output and interpolates it back into the parent's command line — the
/// bytes never reach the script's OUTERMOST stdout. So for the duration of
/// this call we suspend the thread-local callbacks pointer; otherwise the
/// builtin-path `LineDispatchWriter` and the external-path `with_callbacks`
/// dispatch in `stream_loop::external_capture_loop` would leak hidden bytes
/// (e.g. `$(echo hidden)`) into `on_stdout_line` callbacks. The guard's Drop
/// restores the pointer on every exit path (including panics).
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let _suspend = crate::callbacks_thread_local::suspend();
    // Command substitution must complete before its output is interpolated, so
    // ALL backgrounding is ignored here: the trailing `&` (seq.background) and
    // any mid-list `&` separators (Connector::Amp) are run synchronously.
    // Spawning real background children whose pids the parent's JobTable
    // doesn't track would let them escape `wait`/`jobs` and litter the
    // terminal. Amp → Semi so each group runs foreground in source order.
    let sanitized = if seq.background
        || seq.rest.iter().any(|(c, _)| matches!(c, Connector::Amp))
    {
        Sequence {
            first: seq.first.clone(),
            rest: seq
                .rest
                .iter()
                .map(|(c, cmd)| {
                    let c = if matches!(c, Connector::Amp) { Connector::Semi } else { *c };
                    (c, cmd.clone())
                })
                .collect(),
            background: false,
        }
    } else {
        seq.clone()
    };
    let mut buf: Vec<u8> = Vec::new();
    let outcome = {
        let mut sink = StdoutSink::Capture(&mut buf);
        // stderr inside $() still inherits the process (Terminal); capturing
        // stderr through command substitution isn't plumbed here yet.
        let mut err_sink = StderrSink::Terminal;
        execute_sequence_body(&sanitized, shell, &mut sink, &mut err_sink)
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::FunctionReturn(n) => n,
        ExecOutcome::Interrupted(reason) => {
            // The substitution body was aborted by `check_interrupt`. Re-raise
            // the originating flag on the shared `Shell` so the enclosing
            // command list observes it and aborts too — matching bash, where
            // `x=$(... kill -INT $$ ...); echo after` never runs `after`. (v138)
            //
            // SIGINT: `check_interrupt` already cleared `sigint_flag`, so we
            // re-store it. Timeout: `check_interrupt` does NOT clear
            // `timeout_flag` (the builder epilogue is the single reader at the
            // run boundary), so re-storing is logically a no-op — but doing it
            // explicitly keeps the reason-propagation symmetric and robust to
            // future changes.
            match reason {
                InterruptReason::Sigint => {
                    shell
                        .sigint_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    130
                }
                InterruptReason::Timeout => {
                    shell
                        .timeout_flag
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    124
                }
            }
        }
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

/// Runs ONE and-or group foreground: a `first` command plus a `rest` of
/// `(And|Or, &Command)` (NO `Semi`/`Amp` — those are group boundaries handled
/// by `execute_sequence_body`). Carries the existing `&&`/`||` short-circuit
/// plus `$?` propagation, pending-trap dispatch, ERR-trap firing, and errexit
/// handling. Returns the group's final `ExecOutcome` (which may be
/// `Exit`/`LoopBreak`/`LoopContinue`/`FunctionReturn` to propagate upward).
fn run_andor_group(
    first: &Command,
    rest: &[(Connector, &Command)],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let mut status = run_command(first, shell, sink, err_sink);
    if let Some(o) = check_interrupt(shell) {
        return o;
    }
    if matches!(
        status,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted(_)
    ) {
        return status;
    }
    // B-11: propagate `$?` across sequence connectors. The top-level loop
    // in shell.rs only refreshes `shell.last_status` after `process_line`
    // returns, so without this update the second command in `false; echo $?`
    // would see a stale value.
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_status.is_some() {
            return ExecOutcome::Continue(c);
        }
        crate::traps::dispatch_pending_traps(shell);
        // ERR fires for first's failure if not suppressed AND the
        // next connector (if any) is not Or.
        let next_is_or = matches!(rest.first(), Some((Connector::Or, _)));
        if c != 0
            && shell.err_suppressed_depth == 0
            && !next_is_or
            && !is_negated_pipeline(first)
        {
            crate::traps::fire_err_trap(shell);
            if let Some(out) = maybe_errexit(shell, c) {
                return out;
            }
        }
    }
    for i in 0..rest.len() {
        let (connector, command) = &rest[i];
        let should_run = match connector {
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
            // Semi/Amp are group boundaries; they never appear inside a group.
            Connector::Semi | Connector::Amp => true,
        };
        if should_run {
            status = run_command(command, shell, sink, err_sink);
            if let Some(o) = check_interrupt(shell) {
                return o;
            }
            if matches!(
                status,
                ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted(_)
            ) {
                return status;
            }
            if let ExecOutcome::Continue(c) = status {
                shell.set_last_status(c);
                if shell.pending_fatal_status.is_some() {
                    return ExecOutcome::Continue(c);
                }
                crate::traps::dispatch_pending_traps(shell);
                // ERR fires if this command failed AND we're not in a
                // suppression context AND the NEXT connector is not Or
                // (i.e. the failure isn't "handled" by a following || clause).
                let next_is_or = matches!(rest.get(i + 1), Some((Connector::Or, _)));
                if c != 0
                    && shell.err_suppressed_depth == 0
                    && !next_is_or
                    && !is_negated_pipeline(command)
                {
                    crate::traps::fire_err_trap(shell);
                    if let Some(out) = maybe_errexit(shell, c) {
                        return out;
                    }
                }
            }
        }
    }
    status
}

/// A single and-or group carved out of a flat `Sequence`. `backgrounded` is
/// true when the separator that *terminates* this group is `&` (or, for the
/// final group, the sequence's trailing-`&` `background` flag).
struct AndOrGroup<'a> {
    first: &'a Command,
    rest: Vec<(Connector, &'a Command)>,
    backgrounded: bool,
}

/// Partitions a flat `Sequence` into and-or groups at `Semi`/`Amp` boundaries.
/// Each group's `backgrounded` reflects the separator that closes it (`Amp` →
/// true, `Semi` → false); the LAST group inherits `seq.background`.
fn partition_into_groups(seq: &Sequence) -> Vec<AndOrGroup<'_>> {
    let mut groups: Vec<AndOrGroup<'_>> = Vec::new();
    let mut cur_first: &Command = &seq.first;
    let mut cur_rest: Vec<(Connector, &Command)> = Vec::new();
    for (connector, command) in &seq.rest {
        match connector {
            Connector::Semi | Connector::Amp => {
                // This connector closes the current group.
                groups.push(AndOrGroup {
                    first: cur_first,
                    rest: std::mem::take(&mut cur_rest),
                    backgrounded: matches!(connector, Connector::Amp),
                });
                cur_first = command;
            }
            Connector::And | Connector::Or => {
                cur_rest.push((*connector, command));
            }
        }
    }
    // Final group inherits the sequence's trailing-`&` flag.
    groups.push(AndOrGroup {
        first: cur_first,
        rest: cur_rest,
        backgrounded: seq.background,
    });
    groups
}

fn execute_sequence_body(
    seq: &Sequence,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let groups = partition_into_groups(seq);
    // The status of the most recent FOREGROUND group; a list that ends with a
    // backgrounded group reports the launch status 0.
    let mut last_status = ExecOutcome::Continue(0);
    for group in &groups {
        if group.backgrounded {
            // Background the group: wrap its commands into a synthetic
            // foreground `Sequence`, then a `Subshell`, and reuse the existing
            // background-subshell path (forks, registers a job, sets `$!`, no
            // wait). A backgrounded group does NOT affect the foreground `$?`
            // or trigger parent `set -e` (it runs in a child).
            let inner = Sequence {
                first: group.first.clone(),
                rest: group
                    .rest
                    .iter()
                    .map(|(c, cmd)| (*c, (*cmd).clone()))
                    .collect(),
                background: false,
            };
            // Best-effort job-display label for the backgrounded group. The
            // original source text isn't threaded down to this point, so derive
            // a label from the group's first command's static program name when
            // possible (good enough for `jobs` listings — not byte-diffed).
            let source = group_display_label(group.first);
            let subshell = Command::Subshell { body: Box::new(inner) };
            // Launch; ignore the Continue(0) it returns — the foreground status
            // is unchanged by a background launch.
            run_background_subshell(&subshell, shell, sink, err_sink, &source);
        } else {
            last_status = run_andor_group(group.first, &group.rest, shell, sink, err_sink);
            // Propagate control-flow outcomes immediately.
            if matches!(
                last_status,
                ExecOutcome::Exit(_)
                    | ExecOutcome::LoopBreak(_, _)
                    | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_)
                    | ExecOutcome::Interrupted(_)
            ) {
                return last_status;
            }
        }
    }
    last_status
}

/// Dispatches a single sequence element.
fn run_command(
    cmd: &Command,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // `set -n` / `-n` (noexec): read and parse but do not execute. Per-command
    // and non-interactive only (bash ignores -n interactively). Parsing already
    // happened (the reader caught any syntax error) — we simply skip running.
    // Once on, this also skips a later `set +n`, so noexec cannot be turned back
    // off mid-script — matching bash.
    if shell.shell_options.noexec && !shell.is_interactive {
        return ExecOutcome::Continue(0);
    }
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink, err_sink),
        Command::Simple(s) => run_single(s, shell, sink, err_sink),
        Command::If(clause) => run_if(clause, shell, sink, err_sink),
        Command::While(clause) => run_while(clause, shell, sink, err_sink),
        Command::For(clause) => run_for(clause, shell, sink, err_sink),
        Command::Case(clause) => run_case(clause, shell, sink, err_sink),
        Command::BraceGroup(seq) => execute_sequence_body(seq, shell, sink, err_sink),
        Command::Subshell { .. } => {
            let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);
            // Determine stdout fd for the child.  For Terminal (the common
            // case) we pass STDOUT_FILENO directly.  For Capture we create a
            // pipe so the parent can read the child's output back into the
            // capture buffer after the child exits.
            let (stdout_fd, capture_read_fd): (RawFd, Option<RawFd>) = match sink {
                StdoutSink::Terminal => (libc::STDOUT_FILENO, None),
                StdoutSink::Capture(_) => match make_pipe() {
                    Ok((r, w)) => (w, Some(r)),
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        return ExecOutcome::Continue(1);
                    }
                },
            };

            // Mirror the stdout fd construction for stderr:
            //   Terminal → STDERR_FILENO (inherit).
            //   Merged   → stdout_fd (kernel-level 2>&1: both streams hit the
            //              same write-end of whatever fd 1 is — pipe or terminal).
            //   Capture  → fresh pipe; read end drained in parent post-fork.
            let (stderr_fd, capture_err_read_fd): (RawFd, Option<RawFd>) = match err_sink {
                StderrSink::Terminal => (libc::STDERR_FILENO, None),
                StderrSink::Merged => (stdout_fd, None),
                StderrSink::Capture(_) => match make_pipe() {
                    Ok((r, w)) => (w, Some(r)),
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        if let Some(r) = capture_read_fd { unsafe { libc::close(r); } }
                        if stdout_fd != libc::STDOUT_FILENO { unsafe { libc::close(stdout_fd); } }
                        return ExecOutcome::Continue(1);
                    }
                },
            };

            let pid = match fork_and_run_in_subshell(
                cmd,
                shell,
                libc::STDIN_FILENO,
                stdout_fd,
                stderr_fd,
                if interactive { 0 } else { NO_PGROUP },
                &[],
                None, // no Dup redirect at this call site
                None,
            ) {
                Ok(p) => p,
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: fork: {e}"); }
                    if let Some(r) = capture_read_fd {
                        unsafe { libc::close(r); }
                    }
                    if let Some(r) = capture_err_read_fd {
                        unsafe { libc::close(r); }
                    }
                    if stdout_fd != libc::STDOUT_FILENO {
                        unsafe { libc::close(stdout_fd); }
                    }
                    if stderr_fd != libc::STDERR_FILENO && stderr_fd != stdout_fd {
                        unsafe { libc::close(stderr_fd); }
                    }
                    return ExecOutcome::Continue(1);
                }
            };

            // Register the subshell child in the live-children registry so the
            // timeout timer thread can SIGTERM it if the deadline fires. The
            // guard pops on every exit path (including early returns / panics).
            let live_pids = shell.live_external_children.clone();
            live_pids.lock().unwrap().push(pid as libc::pid_t);
            let _pid_guard = LiveChildGuard { pids: &live_pids, pid: pid as libc::pid_t };

            // Close the write-end in the parent so the child's write-end is
            // the only writer; once the child exits, the read-end sees EOF.
            if stdout_fd != libc::STDOUT_FILENO {
                unsafe { libc::close(stdout_fd); }
            }
            // Same for the dedicated stderr pipe (skip if it's the merged-stdout
            // alias; that fd has already been closed above).
            if matches!(err_sink, StderrSink::Capture(_))
                && stderr_fd != libc::STDERR_FILENO
                && stderr_fd != stdout_fd
            {
                unsafe { libc::close(stderr_fd); }
            }

            if interactive {
                // Interactive subshell path keeps the dual-drain +
                // wait_with_untraced shape. Real-time streaming callbacks are
                // moot here (sink == Terminal => no capture pipes are open
                // anyway), and we must keep job-control / stopped-job semantics
                // intact for the REPL.

                // Drain the stderr pipe in a background thread (concurrent with
                // the foreground stdout drain) to avoid PIPE_BUF deadlock when
                // the child writes more than ~64 KiB to either stream.
                let err_drain = if let Some(r) = capture_err_read_fd {
                    use std::os::fd::FromRawFd;
                    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
                    let handle = std::thread::spawn(move || {
                        let mut f = unsafe { File::from_raw_fd(r) };
                        let mut local = Vec::new();
                        let _ = io::copy(&mut f, &mut local);
                        let _ = tx.send(local);
                    });
                    Some((handle, rx))
                } else {
                    None
                };

                // Drain stdout capture pipe before waitpid to avoid deadlock.
                if let (Some(r), StdoutSink::Capture(buf)) = (capture_read_fd, &mut *sink) {
                    use std::os::fd::FromRawFd;
                    let mut f = unsafe { File::from_raw_fd(r) };
                    let _ = io::copy(&mut f, *buf);
                    // f is dropped here, closing r.
                }

                // Now join the stderr drainer and fold bytes into err_sink.
                if let Some((handle, rx)) = err_drain {
                    let _ = handle.join();
                    if let Ok(bytes) = rx.recv()
                        && let StderrSink::Capture(buf) = err_sink
                    {
                        buf.extend_from_slice(&bytes);
                    }
                }

                // Foreground subshell: make it a job that owns the terminal,
                // mirroring the single-command/pipeline dance. Without this the
                // subshell runs in a background pgroup and deadlocks on tty I/O.
                // (fork_and_run_in_subshell already race-closes setpgid in the
                // parent; this is belt-and-suspenders to guarantee the pgrp
                // exists before give_terminal_to, mirroring the pipeline path.)
                unsafe {
                    if libc::setpgid(pid, pid) != 0 {
                        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        debug_assert!(
                            errno == libc::ESRCH || errno == libc::EACCES,
                            "setpgid({pid}, {pid}) failed with unexpected errno {errno}"
                        );
                    }
                }
                give_terminal_to(pid);
                let outcome = match wait_with_untraced(pid) {
                    Ok((raw_status, true)) => {
                        let sig = libc::WSTOPSIG(raw_status);
                        let job_id = shell.jobs.add(pid, vec![pid], "( subshell )".to_string());
                        for job in shell.jobs.jobs_mut() {
                            if job.id == job_id {
                                job.state = crate::jobs::JobState::Stopped(sig);
                                job.notified = true;
                                break;
                            }
                        }
                        let line = shell.jobs.iter()
                            .find(|j| j.id == job_id)
                            .map(|j| crate::jobs::notification_line(j, '+'))
                            .unwrap_or_default();
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "\n{line}"); }
                        128 + sig
                    }
                    Ok((raw_status, false)) => {
                        if libc::WIFEXITED(raw_status) {
                            libc::WEXITSTATUS(raw_status)
                        } else if libc::WIFSIGNALED(raw_status) {
                            let sig = libc::WTERMSIG(raw_status);
                            if sig == libc::SIGINT {
                                shell
                                    .sigint_flag
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            128 + sig
                        } else {
                            1
                        }
                    }
                    Err(()) => 1,
                };
                give_terminal_to(shell.shell_pgid);
                shell.set_pipestatus(&[outcome]);
                ExecOutcome::Continue(outcome)
            } else {
                // Non-interactive (script), capture (`$( ( … ) )`), nested
                // subshell, or completion: poll-based wait via
                // stream_loop::external_capture_loop. Runs on the embedder's
                // thread; streaming callbacks fire in real time.
                let pipe_out: RawFd = capture_read_fd.unwrap_or(-1);
                let pipe_err: RawFd = capture_err_read_fd.unwrap_or(-1);
                let mut stderr_capture: Vec<u8> = Vec::new();
                let stdout_sink_buf: Option<&mut Vec<u8>> = match &mut *sink {
                    StdoutSink::Capture(buf) => Some(*buf),
                    StdoutSink::Terminal => None,
                };
                let stderr_sink_buf: Option<&mut Vec<u8>> = if matches!(err_sink, StderrSink::Capture(_)) {
                    Some(&mut stderr_capture)
                } else {
                    None
                };
                let sinks = crate::stream_loop::CaptureSinks {
                    stdout: stdout_sink_buf,
                    stderr: stderr_sink_buf,
                };
                let loop_result = crate::stream_loop::external_capture_loop(
                    pid as libc::pid_t,
                    pipe_out,
                    pipe_err,
                    sinks,
                    || None,
                );
                // Close pipe read-ends we owned.
                if pipe_out >= 0 {
                    unsafe { libc::close(pipe_out); }
                }
                if pipe_err >= 0 {
                    unsafe { libc::close(pipe_err); }
                }
                let raw_status = match loop_result {
                    Ok(s) => s,
                    Err(_) => return ExecOutcome::Continue(1),
                };
                // Fold captured stderr bytes into err_sink now that &mut sink
                // is released.
                if let StderrSink::Capture(buf) = err_sink {
                    buf.extend_from_slice(&stderr_capture);
                }
                let code = if libc::WIFEXITED(raw_status) {
                    libc::WEXITSTATUS(raw_status)
                } else if libc::WIFSIGNALED(raw_status) {
                    let sig = libc::WTERMSIG(raw_status);
                    if sig == libc::SIGINT {
                        shell
                            .sigint_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    128 + sig
                } else {
                    1
                };
                shell.set_pipestatus(&[code]);
                ExecOutcome::Continue(code)
            }
        }
        Command::FunctionDef { name, body } => {
            // POSIX: a function may not be named after a special builtin; a
            // non-interactive posix shell errors and exits (default mode allows it).
            if shell.shell_options.posix && builtins::is_special_builtin(name) {
                { let mut err = err_writer(err_sink, sink);
                  e!(&mut *err, "{}{name}: is a special builtin", shell.error_prefix(None)); }
                shell.posix_fatal(2);
                return ExecOutcome::Continue(2);
            }
            shell.define_function(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
        Command::DoubleBracket { expr, inline_assignments } => {
            run_double_bracket(expr, inline_assignments, shell, sink, err_sink)
        }
        Command::ArithFor(clause) => run_arith_for(clause, shell, sink, err_sink),
        Command::Arith(expr) => run_arith(expr, shell, sink, err_sink),
        Command::Select(clause) => run_select(clause, shell, sink, err_sink),
        Command::Redirected { inner, redirects } => {
            run_redirected(inner, redirects, shell, sink, err_sink)
        }
        Command::Coproc { name, body } => run_coproc(name, body, shell, sink, err_sink),
        _ => {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: unsupported command variant"); }
            ExecOutcome::Continue(1)
        }
    }
}

/// RAII guard that records fds saved (via `dup`) before they were replaced by
/// `dup2`, and restores each on Drop. `saved` holds `(target_fd, saved_dup_fd)`
/// pairs; restoration runs in reverse to undo overlapping swaps cleanly.
///
/// v156: generalized into the ordered in-process redirect applier. `apply`
/// walks one `Redirection` at a time over the real fds (open/dup2/close),
/// honoring source order so e.g. `2>&1 >file` differs from `>file 2>&1`. A
/// failure mid-list returns `Err(outcome)`; Drop then rolls back the entries
/// already applied (atomic). Heredoc/here-string writer pids spawned during
/// `apply` are tracked in `heredoc_writers` and reaped by `reap_heredoc_writers`
/// after the body has run.
struct RedirectScope {
    saved: Vec<(RawFd, RawFd)>,
    heredoc_writers: Vec<libc::pid_t>,
}

impl RedirectScope {
    fn new() -> Self {
        RedirectScope { saved: Vec::new(), heredoc_writers: Vec::new() }
    }

    /// Replace `target_fd` with a dup of `new_fd`, saving the original so Drop
    /// can restore it. `new_fd` is NOT consumed (caller closes it). If
    /// `target_fd` is not currently open, the saved slot is recorded as `-1`
    /// (Drop closes it back to unopened) — bash leaves a fresh high fd open
    /// only for the command's duration.
    fn redirect(
        &mut self,
        new_fd: RawFd,
        target_fd: RawFd,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ()> {
        unsafe {
            // `dup` fails with EBADF when target_fd is not open (e.g. a fresh
            // fd>2 like `>&3` when fd 3 was never opened). That is fine — record
            // -1 so Drop closes target_fd back to its unopened state.
            let saved = libc::dup(target_fd);
            if libc::dup2(new_fd, target_fd) < 0 {
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: dup2: {}", io::Error::last_os_error()); }
                if saved >= 0 {
                    libc::close(saved);
                }
                return Err(());
            }
            self.saved.push((target_fd, saved));
        }
        Ok(())
    }

    /// `N>&-` / `N<&-`: close `target_fd`, saving its prior state so Drop can
    /// restore it. EBADF (already closed) is lenient per bash.
    fn close_target(&mut self, target_fd: RawFd) {
        unsafe {
            let saved = libc::dup(target_fd);
            // Record the swap so Drop restores (saved == -1 if it was unopened).
            self.saved.push((target_fd, saved));
            libc::close(target_fd);
        }
    }

    /// Apply one redirection to the real fds, saving the prior target for
    /// restore. Returns `Err(outcome)` on failure (diagnostic already printed).
    fn apply(
        &mut self,
        redir: &Redirection,
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        use std::os::unix::io::IntoRawFd;
        if let RedirFd::Var(name) = &redir.fd {
            return self.apply_var(name, redir, shell, sink, err_sink);
        }
        let Some(target) = redir.target_fd() else {
            // RedirFd::Var is handled above; any other None is unexpected.
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ambiguous redirect"); }
            return Err(ExecOutcome::Continue(1));
        };
        let target = target as RawFd;
        match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => return Err(ExecOutcome::Continue(1)),
                };
                if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
                let new_fd: RawFd = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                            return Err(ExecOutcome::Continue(1));
                        }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            // Truncate honors noclobber (`set -C`).
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", resolved_path(&resolved)); }
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        // `<>`: O_RDWR|O_CREAT — open in place, do NOT truncate
                        // (bash keeps existing content for read-write access).
                        match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                };
                if new_fd == target {
                    // The kernel placed the opened file directly at the target fd,
                    // which means target was previously free/closed (lowest-free
                    // fd == target). Leave the file in place and record a
                    // "was-closed" restore (-1) so Drop closes target back when
                    // the scope ends. Do NOT dup2 and do NOT close new_fd (it IS
                    // the target now).
                    self.saved.push((target, -1));
                } else {
                    // Normal case: save the prior target state (or -1 if it was
                    // closed), dup2 the opened file onto the target, then close
                    // the temp fd. `redirect()` already records saved=-1 when
                    // dup(target) returns EBADF (target was free but not lowest).
                    if self.redirect(new_fd, target, sink, err_sink).is_err() {
                        unsafe { libc::close(new_fd) };
                        return Err(ExecOutcome::Continue(1));
                    }
                    unsafe { libc::close(new_fd) };
                }
                Ok(())
            }
            RedirOp::Dup { source, .. } => {
                // `>&w` / `<&w`: duplicate the source fd onto target. Resolved
                // AFTER earlier swaps so e.g. `>file 2>&1` makes stderr follow
                // the already-redirected stdout.
                let src = match resolve_fd_target(source, shell) {
                    Ok(fd) => fd,
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                        return Err(ExecOutcome::Continue(1));
                    }
                };
                // Validate the source fd is open before dup2 (bash: bad fd error).
                if unsafe { libc::fcntl(src, libc::F_GETFD) } < 0 {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {src}: Bad file descriptor"); }
                    return Err(ExecOutcome::Continue(1));
                }
                if self.redirect(src, target, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
                Ok(())
            }
            RedirOp::Close => {
                self.close_target(target);
                Ok(())
            }
            RedirOp::Heredoc { body, .. } => {
                // The lexer stores a `<<'EOF'` (non-expanding) body as a single
                // literal Word, so `expand_assignment` is a no-op there; an
                // expanding `<<EOF` body undergoes parameter/command expansion.
                // Mirrors the pre-v156 with_redirect_scope stdin block.
                let bytes = expand_assignment(body, shell).into_bytes();
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        if self.redirect(rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                        Err(ExecOutcome::Continue(1))
                    }
                }
            }
            RedirOp::HereString(w) => {
                let mut bytes = expand_assignment(w, shell).into_bytes();
                bytes.push(b'\n');
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        if self.redirect(rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                        Err(ExecOutcome::Continue(1))
                    }
                }
            }
        }
    }

    /// Apply a `{var}` named-fd redirection in-process. Allocates a free fd >= 10
    /// (non-CLOEXEC, so an exec'd child would inherit it), wires the redirect onto
    /// it, and assigns the number to `$name` (the var PERSISTS after the command).
    /// The allocated fd is NOT registered in `saved` — bash keeps it open in the
    /// shell process until an explicit `{var}>&-` (Close) or shell exit, so Drop
    /// must NOT close it. Explicit close via `{var}>&-` is handled in the
    /// `RedirOp::Close` arm above.
    fn apply_var(
        &mut self,
        name: &str,
        redir: &Redirection,
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        use std::os::unix::io::IntoRawFd;
        // `{var}>&-` / `{var}<&-`: close the fd currently named by $var.
        if matches!(&redir.op, RedirOp::Close) {
            let cur = shell.lookup_var(name).unwrap_or_default();
            let fd: RawFd = match cur.trim().parse::<i32>() {
                Ok(n) if n >= 0 => n,
                _ => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: ambiguous redirect"); }
                    return Err(ExecOutcome::Continue(1));
                }
            };
            // Save prior state so Drop restores (saved == -1 if it was unopened),
            // then close. EBADF (already closed) is lenient per bash.
            self.close_target(fd);
            return Ok(());
        }
        // Compute the source fd to dup from. `owns_src` is true when WE opened it
        // (File / heredoc / here-string read end) and must close it after duping;
        // a Dup source belongs to the shell and is left alone.
        let (src, owns_src): (RawFd, bool) = match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => return Err(ExecOutcome::Continue(1)),
                };
                if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
                let fd: RawFd = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                            return Err(ExecOutcome::Continue(1));
                        }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", resolved_path(&resolved)); }
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                };
                (fd, true)
            }
            RedirOp::Dup { source, .. } => {
                let src = match resolve_fd_target(source, shell) {
                    Ok(fd) => fd,
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                        return Err(ExecOutcome::Continue(1));
                    }
                };
                if unsafe { libc::fcntl(src, libc::F_GETFD) } < 0 {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {src}: Bad file descriptor"); }
                    return Err(ExecOutcome::Continue(1));
                }
                (src, false)
            }
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        (rfd, true)
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                        return Err(ExecOutcome::Continue(1));
                    }
                }
            }
            RedirOp::HereString(w) => {
                let mut bytes = expand_assignment(w, shell).into_bytes();
                bytes.push(b'\n');
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        (rfd, true)
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                        return Err(ExecOutcome::Continue(1));
                    }
                }
            }
            RedirOp::Close => unreachable!("Close handled above"),
        };
        // Allocate a free high fd duped from `src` (non-CLOEXEC). The high fd
        // ITSELF is the live descriptor the command sees — do NOT dup2 onto a
        // lower fd.
        let high = match alloc_high_fd(src) {
            Ok(h) => h,
            Err(e) => {
                if owns_src {
                    unsafe { libc::close(src) };
                }
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: {e}"); }
                return Err(ExecOutcome::Continue(1));
            }
        };
        if owns_src {
            // The opened file / heredoc read-end was only a temp to dup from.
            unsafe { libc::close(src) };
        }
        // Assign $var and leave `high` OPEN — bash keeps the allocated fd alive
        // in the shell process until an explicit `{var}>&-` or shell exit.
        // Do NOT register `high` in `self.saved`; Drop must NOT close it.
        shell.set(name, high.to_string());
        Ok(())
    }

    /// Reap any forked heredoc/here-string writers spawned during `apply`.
    /// Call after the body has run (its consumer has drained + closed the read
    /// end, so the writer has finished). ECHILD or any error is fine.
    fn reap_heredoc_writers(&mut self) {
        for pid in self.heredoc_writers.drain(..) {
            let mut st = 0;
            unsafe { libc::waitpid(pid, &mut st, 0); }
        }
    }
}

impl Drop for RedirectScope {
    fn drop(&mut self) {
        // Flush any buffered stdout written through the redirected fd before we
        // swap the original fd back, so it lands in the redirect target.
        let _ = io::stdout().flush();
        // Restore in reverse application order.
        while let Some((target_fd, saved)) = self.saved.pop() {
            unsafe {
                if saved >= 0 {
                    libc::dup2(saved, target_fd);
                    libc::close(saved);
                } else {
                    // target_fd was not open before we redirected it — close it
                    // back to its unopened state.
                    libc::close(target_fd);
                }
            }
        }
    }
}

/// True if `cmd` carries any explicit redirect — the gate for wrapping a
/// body-running command (function / eval / source) in `with_redirect_scope`.
fn has_any_redirect(cmd: &ExecCommand) -> bool {
    !cmd.redirects.is_empty()
}

/// True if any redirection in `redirs` re-targets fd 1 (stdout) — the gate for
/// forcing a `Terminal` inner sink so the redirect wins over an outer capture.
/// Any output File, a Dup (`>&N`), OR a Close (`>&-`) on fd 1 qualifies: in all
/// three cases the command's real fd 1 is redirected (to a file, another fd, or
/// closed), so an in-process builtin must write through `io::stdout()` (= fd 1 =
/// the redirect target) rather than into the capture buffer — otherwise `>&-`'s
/// discard / `>&N`'s dup would be silently ignored by the buffer. A stdin-only
/// redirect does not force Terminal. `RedirFd::Var` (target_fd None) is ignored.
fn redirs_write_stdout(redirs: &[Redirection]) -> bool {
    redirs.iter().any(|r| {
        r.target_fd() == Some(1)
            && matches!(
                &r.op,
                RedirOp::File {
                    mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber | FileMode::ReadWrite,
                    ..
                } | RedirOp::Dup { .. }
                  | RedirOp::Close
            )
    })
}

/// The final effective destination of a fd after applying a redirect list
/// left-to-right (source order). Used by `final_dests_for_1_2` to decide
/// whether the builtin-redirect path's in-memory `>&2` / `2>&1` software
/// routing applies — it must fire ONLY when no earlier file/pipe redirect
/// already intercepted the source fd.
enum RedirectDest {
    /// No redirect on this fd: inherits whatever the active sink provides.
    Sink,
    /// Any non-`Dup` redirect on this fd (file, here-doc, here-string, close,
    /// etc.). The real-fd scope owns this destination; software routing must
    /// NOT short-circuit it.
    External,
    /// `>&N` / `<&N`: this fd follows whatever fd `N` points to at apply time.
    /// Carries the literal source fd resolved from the redirect word.
    Follows(u32),
}

/// Walk `redirs` in source order and return the FINAL effective destination of
/// fd 1 and fd 2 — i.e. what the real-fd scope will actually leave fd 1 and
/// fd 2 pointing at AFTER all redirects are applied. Used by
/// `run_builtin_with_redirects` to decide whether software in-memory routing
/// can stand in for a `>&2` / `2>&1` dup, or whether an earlier file/pipe
/// redirect on the same fd means the real-fd dup must run as-is.
///
/// Source words on `Dup` are resolved via `resolve_fd_target`; an unresolvable
/// source falls back to `External` (conservative: the real-fd scope will
/// report the error and we want to skip software routing).
fn final_dests_for_1_2(
    redirs: &[Redirection],
    shell: &mut Shell,
) -> (RedirectDest, RedirectDest) {
    let mut fd1 = RedirectDest::Sink;
    let mut fd2 = RedirectDest::Sink;
    for r in redirs {
        let Some(fd) = r.target_fd() else { continue };
        if fd != 1 && fd != 2 { continue; }
        let dest = match &r.op {
            RedirOp::Dup { source: src_word, output: true } => {
                match resolve_fd_target(src_word, shell) {
                    Ok(n) if n >= 0 => RedirectDest::Follows(n as u32),
                    _ => RedirectDest::External,
                }
            }
            // Any other op (File, Close, Heredoc, HereString, Dup{output:false})
            // hands the fd to the real-fd scope.
            _ => RedirectDest::External,
        };
        match fd {
            1 => fd1 = dest,
            2 => fd2 = dest,
            _ => {}
        }
    }
    (fd1, fd2)
}

/// Returns the redirections NOT consumed by the pipeline-stage 0/1/2 slot
/// fast-path (`slots_for_simple_path`): fd>2, `<&` dup-in, `N>&-` close, `<>`
/// ReadWrite, and the cross-direction combos the fast-path drops. The fast-path
/// consumes:
///   fd 0: File{ReadOnly}, Heredoc, HereString
///   fd 1/2: File{Truncate|Append|Clobber}, Dup{output:true}
/// A redirection is "extra" iff it is NOT one the fast-path consumes. ALL
/// fast-path-consumed entries are excluded — including earlier same-fd entries
/// the fast-path shadowed (they would be a no-op or, worse, double-applied, so we
/// drop every fast-path-eligible entry regardless of position).
/// Used ONLY by the pipeline-external additive path (`build_child_extra_ops`);
/// the single-command builtin/external paths now apply the full ordered list.
fn stage_extra_redirects(redirs: &[Redirection]) -> Vec<Redirection> {
    redirs
        .iter()
        .filter(|r| !slot_consumes(r))
        .cloned()
        .collect()
}

/// True if the pipeline-stage 0/1/2 slot fast-path consumes this redirection into
/// a slot (so the additive extra-op list must skip it to avoid double-applying).
fn slot_consumes(r: &Redirection) -> bool {
    match r.target_fd() {
        Some(0) => matches!(
            &r.op,
            RedirOp::File { mode: FileMode::ReadOnly, .. }
                | RedirOp::Heredoc { .. }
                | RedirOp::HereString(_)
        ),
        Some(1) | Some(2) => matches!(
            &r.op,
            RedirOp::File {
                mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber,
                ..
            } | RedirOp::Dup { output: true, .. }
        ),
        _ => false,
    }
}

/// Applies redirects at the real-fd level (saved/restored via `RedirectScope`),
/// forcing a `Terminal` inner sink when a stdout redirect is present so the
/// redirect wins over an outer capture, then runs `run_inner(shell, inner_sink)`
/// and returns its status. Redirects are applied in source order. A
/// redirect-open failure prints `huck: <target>: <err>` and returns
/// `Continue(1)` WITHOUT running `run_inner`. Shared by `run_redirected` for
/// compound commands and by the function / eval / source call branches.
fn with_redirect_scope<F>(
    redirs: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    run_inner: F,
) -> ExecOutcome
where
    F: FnOnce(&mut Shell, &mut StdoutSink, &mut StderrSink) -> ExecOutcome,
{
    // Snapshot the procsub stack BEFORE expanding any redirect-target words.
    // Any process substitutions realized while expanding redirect words (e.g.
    // `cmd < <(inner)`) are recorded in [procsub_base..]. We drain that slice
    // on every exit path. The run_exec_single layer inside run_inner handles
    // its own argument procsubs (those go in [run_exec_base..] which is a
    // subset captured later), so the two layers cover disjoint ranges.
    let procsub_base = shell.procsub_pending.len();

    // Flush buffered terminal/builtin output BEFORE swapping fds so prior
    // output is not diverted into the redirect target.
    let _ = io::stdout().flush();

    let mut scope = RedirectScope::new();

    // Apply every redirection IN SOURCE ORDER, so `2>&1 >file` differs from
    // `>file 2>&1`. A failure mid-list returns early; the scope's Drop rolls
    // back the entries already applied (atomic, matching pre-v156 behavior).
    let force_terminal = redirs_write_stdout(redirs);
    for r in redirs {
        if let Err(outcome) = scope.apply(r, shell, sink, err_sink) {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return outcome;
        }
    }

    // Run the inner compound with the now-redirected fds. Its (possibly
    // buffered) terminal output is flushed by the scope's Drop before restore.
    //
    // If a stdout redirect (`>`/`>>`/`>&`/`&>`) is present, fd 1 now points at
    // the redirect target. In capture mode (`$(...)`) the outer `sink` would
    // otherwise steer the inner command's stdout into the capture buf/pipe,
    // ignoring the redirect entirely. Force `Terminal` so builtins write via
    // `io::stdout()` (= fd 1 = the target) and externals inherit the redirected
    // fd 1 — the capture then correctly receives nothing for the diverted
    // stream. This is a no-op when the outer sink is already `Terminal`. A
    // compound with only a stdin/stderr redirect keeps the outer sink so its
    // stdout is still captured.
    let mut terminal_sink = StdoutSink::Terminal;
    let inner_sink: &mut StdoutSink = if force_terminal {
        &mut terminal_sink
    } else {
        sink
    };
    let outcome = run_inner(shell, inner_sink, err_sink);
    let _ = io::stdout().flush();
    // Reap the forked heredoc/herestring writers now that the inner body has run
    // (the consumer has drained and closed its read end, so the writers have
    // finished). ECHILD or any error is fine — they are transient helpers.
    scope.reap_heredoc_writers();
    // Restore the real fds BEFORE draining redirect-target process substitutions.
    // For an OUTPUT procsub (`cmd > >(consumer)`), the redirect dup'd the procsub's
    // write end onto fd 1; `drain_procsubs` blocks waiting for the inner consumer
    // (e.g. `cat`), which only sees EOF once that write end is closed — i.e. when the
    // scope's Drop restores fd 1. Draining first causes a deadlock: the consumer
    // never sees EOF while the scope still holds fd 1 open.
    // (This mirrors the ordering in `run_builtin_with_redirects`, which was fixed
    // for the same reason. Drop-then-drain is also safe for INPUT procsubs: their
    // inner producer already wrote and the body has finished reading, so closing
    // the pipe-read copy just lets cleanup reap normally.)
    drop(scope);
    drain_procsubs(shell, procsub_base);
    debug_assert_eq!(
        shell.procsub_pending.len(), procsub_base,
        "process-substitution leak: a return path in with_redirect_scope skipped drain_procsubs"
    );
    outcome
}

/// Runs an in-process builtin (regular or declaration) with ALL of `redirs`
/// applied in SOURCE ORDER to the real fds via one `RedirectScope`, then
/// restored on return. Replaces the old additive bridge path (legacy 0/1/2
/// slots + a disjoint extra scope), so `echo x 2>&1 >file` is now source-ordered
/// for bare builtins exactly like compounds/functions/externals (L-08 fully
/// fixed). A redirect-open failure prints its own diagnostic and returns
/// `Continue(1)` WITHOUT running the builtin (the scope's Drop rolls back any
/// partially-applied redirects).
///
/// Sink handling mirrors `with_redirect_scope`: when any redirect writes to
/// stdout (fd 1), force a `Terminal` sink so the builtin writes through
/// `io::stdout()` (= fd 1 = the redirect target) and an outer capture correctly
/// receives nothing for the diverted stream. Otherwise the enclosing `sink` is
/// kept, so `r=$(builtin)` still captures the builtin's stdout into the buffer.
///
/// **In-memory `>&2` / `2>&1` routing (v205):** A `>&2` (fd 1 → fd 2) under a
/// `StderrSink::Capture` or `StderrSink::Merged` sink — and the symmetric
/// `2>&1` (fd 2 → fd 1) under a `StdoutSink::Capture` sink — would, when applied
/// at the real-fd level, dup to the embedder's terminal fd, missing the
/// in-memory buffer. To hit the buffer we detect a TRAILING `>&2` / `2>&1` (no
/// later override of the target fd) and route the builtin's writer to the other
/// sink IN SOFTWARE; the redirect is still applied at the real-fd level (cheap
/// no-op for the builtin's writer choice — external children below would not
/// see the swap and we are an in-process builtin). Resolves L-25.
///
/// `read`'s stdin (`<`, `<<`, `<<<`) lands on fd 0 via the scope, so the builtin
/// reads from the redirected descriptor. Heredoc/here-string writer pids spawned
/// during apply are reaped after the call.
fn run_builtin_with_redirects(
    resolved: &ResolvedCommand,
    redirs: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let procsub_base = shell.procsub_pending.len();
    let _ = io::stdout().flush();

    // Detect in-memory dup re-routing BEFORE applying the real-fd scope.
    // - `route_out_to_err`: final `>&2` (fd 1 follows fd 2) where fd 2's final
    //   destination is the sink (no earlier file/pipe on fd 2 to intercept it)
    //   AND stderr is in-memory.
    // - `route_err_to_out`: final `2>&1` (fd 2 follows fd 1) where fd 1's final
    //   destination is the sink AND stdout is in-memory.
    //
    // The walk over `redirs` computes each fd's FINAL effective destination so
    // we don't over-fire on `>file 2>&1` (where fd 1's earlier `>file` makes the
    // real-fd dup chain the right path: software routing would steal the bytes
    // from the file).
    let (final_1, final_2) = final_dests_for_1_2(redirs, shell);
    let route_out_to_err = matches!(err_sink, StderrSink::Capture(_) | StderrSink::Merged)
        && matches!(final_2, RedirectDest::Sink)
        && matches!(final_1, RedirectDest::Follows(2));
    let route_err_to_out = matches!(sink, StdoutSink::Capture(_))
        && matches!(final_1, RedirectDest::Sink)
        && matches!(final_2, RedirectDest::Follows(1));

    let mut scope = RedirectScope::new();
    for r in redirs {
        if let Err(outcome) = scope.apply(r, shell, sink, err_sink) {
            scope.reap_heredoc_writers();
            drop(scope);
            drain_procsubs(shell, procsub_base);
            return outcome;
        }
    }

    // When a stdout redirect is present, fd 1 now points at the target; force a
    // Terminal sink so the builtin writes there (= fd 1 = the target) instead of
    // into an outer capture buf. A capture sink with NO stdout redirect keeps
    // writing to the buf so `r=$(builtin)` still captures.
    //
    // EXCEPT when `route_out_to_err` is set: the `>&2` Dup would normally make
    // `redirs_write_stdout` true and force fd-1 writes, but we want to route
    // the builtin's stdout to the in-memory stderr sink instead. The `if
    // route_out_to_err` arm below handles this; suppress `write_to_fd1` here.
    let write_to_fd1 = !route_out_to_err
        && (redirs_write_stdout(redirs) || matches!(sink, StdoutSink::Terminal));
    let run = |out: &mut dyn std::io::Write, err: &mut dyn std::io::Write, shell: &mut Shell| {
        if let Some(da) = resolved.decl_args.as_deref() {
            builtins::run_declaration_builtin(&resolved.program, da, out, err, shell)
        } else {
            builtins::run_builtin(&resolved.program, &resolved.args, out, err, shell)
        }
    };
    // Materialize the stderr writer from the err_sink. In the capture-stdout
    // arm below we MUST split the `sink` and `err_sink` borrows manually because
    // `*buf` (used as `out`) is already a mutable borrow of `sink`; the helper
    // `err_writer` (which takes both sinks) would conflict. So in the capture
    // arm we hand-roll the err writer here, mirroring `err_writer`'s logic.
    let outcome = if route_out_to_err {
        // `>&2` under captured/merged stderr: route the builtin's stdout into
        // the (effective) stderr destination. The `err` writer is io::stderr()
        // — the builtin's own direct stderr writes (e.g. an error diagnostic)
        // still land in the embedder's stderr if err_sink is Terminal, but
        // err_sink isn't Terminal here (the route_out_to_err guard requires
        // Capture or Merged), so we materialize err from those.
        match (&mut *sink, &mut *err_sink) {
            (_, StderrSink::Capture(ebuf)) => {
                // out writes go to the stderr capture buffer; err writes also
                // go to it. (Borrow ebuf only once; route both writers via a
                // side buf for the err side to avoid aliasing.) The side buf
                // is wrapped in a LineDispatchWriter tagged Stderr so streaming
                // callbacks fire for the builtin's direct stderr writes too.
                let mut side_err_buf: Vec<u8> = Vec::new();
                let outcome = {
                    let mut out_w = LineDispatchWriter {
                        inner: ebuf,
                        stream: LineStream::Stderr,
                    };
                    let mut side_err_w = LineDispatchWriter {
                        inner: &mut side_err_buf,
                        stream: LineStream::Stderr,
                    };
                    run(&mut out_w, &mut side_err_w, shell)
                };
                ebuf.extend_from_slice(&side_err_buf);
                outcome
            }
            (StdoutSink::Capture(obuf), StderrSink::Merged) => {
                // Merged means stderr is routed to the active stdout sink (here:
                // the capture buf). So out writes (via `>&2` → merged → buf) AND
                // err writes both go to obuf. Tag the side buf Stdout so the
                // embedder sees these bytes as stdout-stream events (Merged
                // means stderr converges on stdout from the embedder's view).
                let mut side_err_buf: Vec<u8> = Vec::new();
                let outcome = {
                    let mut out_w = LineDispatchWriter {
                        inner: obuf,
                        stream: LineStream::Stdout,
                    };
                    let mut side_err_w = LineDispatchWriter {
                        inner: &mut side_err_buf,
                        stream: LineStream::Stdout,
                    };
                    run(&mut out_w, &mut side_err_w, shell)
                };
                obuf.extend_from_slice(&side_err_buf);
                outcome
            }
            (StdoutSink::Terminal, StderrSink::Merged) => {
                // Merged + terminal stdout: writes go to real fd 1 (which the
                // redirect dup'd from real fd 2, so → real fd 2). This matches
                // the non-routed path, so just fall back to the standard write.
                let mut out = io::stdout();
                let mut err = err_writer(err_sink, sink);
                run(&mut out, &mut *err, shell)
            }
            (_, StderrSink::Terminal) => unreachable!("route_out_to_err requires non-Terminal err_sink"),
        }
    } else if route_err_to_out {
        // `2>&1` under captured stdout: route the builtin's stderr into the
        // stdout capture buf. (L-25 resolution.)
        match sink {
            StdoutSink::Capture(obuf) => {
                // out → obuf (the standard capture path), err → obuf (via the
                // `2>&1` swap). Aliasing: borrow obuf once for out; use a side
                // buf for err and append. Tag the side buf Stdout — the script
                // redirected fd 2 to fd 1, so the embedder sees these bytes as
                // stdout-stream events.
                let mut side_err_buf: Vec<u8> = Vec::new();
                let outcome = {
                    let mut out_w = LineDispatchWriter {
                        inner: obuf,
                        stream: LineStream::Stdout,
                    };
                    let mut side_err_w = LineDispatchWriter {
                        inner: &mut side_err_buf,
                        stream: LineStream::Stdout,
                    };
                    run(&mut out_w, &mut side_err_w, shell)
                };
                obuf.extend_from_slice(&side_err_buf);
                outcome
            }
            StdoutSink::Terminal => unreachable!("route_err_to_out requires Capture stdout"),
        }
    } else if write_to_fd1 {
        let mut out = io::stdout();
        let mut err = err_writer(err_sink, sink);
        run(&mut out, &mut *err, shell)
    } else {
        // Capture stdout sink with no fd-1 redirect. Mirror `err_writer` inline
        // so the `*buf` borrow for `out` doesn't fight the err_sink construction.
        match sink {
            StdoutSink::Terminal => unreachable!("Terminal handled by write_to_fd1"),
            StdoutSink::Capture(buf) => match err_sink {
                StderrSink::Terminal => {
                    let mut err = io::stderr();
                    let mut out_w = LineDispatchWriter {
                        inner: buf,
                        stream: LineStream::Stdout,
                    };
                    run(&mut out_w, &mut err, shell)
                }
                StderrSink::Capture(ebuf) => {
                    let mut out_w = LineDispatchWriter {
                        inner: buf,
                        stream: LineStream::Stdout,
                    };
                    let mut err_w = LineDispatchWriter {
                        inner: ebuf,
                        stream: LineStream::Stderr,
                    };
                    run(&mut out_w, &mut err_w, shell)
                }
                StderrSink::Merged => {
                    // Both stdout and stderr converge on the same capture buf.
                    // Rust's aliasing rules forbid handing `&mut *buf` to both
                    // `out` and `err`; route them through a thread-local-style
                    // side buffer for stderr then append after the call. Order
                    // is preserved as out-then-err (builtins use fd 1 then fd 2
                    // in series in practice); not byte-strict-interleaved but
                    // matches a single-writer line discipline well enough. Tag
                    // the side buf Stdout — Merged means stderr converges on
                    // stdout from the embedder's view.
                    let mut side: Vec<u8> = Vec::new();
                    let outcome = {
                        let mut out_w = LineDispatchWriter {
                            inner: buf,
                            stream: LineStream::Stdout,
                        };
                        let mut side_w = LineDispatchWriter {
                            inner: &mut side,
                            stream: LineStream::Stdout,
                        };
                        run(&mut out_w, &mut side_w, shell)
                    };
                    buf.extend_from_slice(&side);
                    outcome
                }
            },
        }
    };
    let _ = io::stdout().flush();
    let _ = std::io::Write::flush(&mut std::io::stderr());
    scope.reap_heredoc_writers();
    // Restore the real fds BEFORE draining redirect-target process substitutions.
    // For an OUTPUT procsub (`builtin > >(cat)`), the redirect dup'd the procsub's
    // write end onto fd 1; `drain_procsubs` blocks waiting for the inner consumer
    // (`cat`), which only sees EOF once that write end is closed — i.e. when the
    // scope's Drop restores fd 1. (The old bridge path closed the builtin's
    // stdout `File` before draining for the same reason.) Dropping first is safe
    // for INPUT procsubs too: their inner producer already wrote and the builtin
    // has finished reading, so closing fd 0 just lets cleanup reap.
    drop(scope);
    drain_procsubs(shell, procsub_base);
    outcome
}

/// Runs a compound command with trailing redirections applied at the real fd
/// level: each present redirect is `dup2`'d onto fd 0/1/2 (originals saved and
/// restored on scope exit), `inner` runs through the existing `sink`, and its
/// status is returned. A redirect-open failure prints `huck: <target>: <err>`
/// and returns `Continue(1)` WITHOUT running `inner`.
fn run_redirected(
    inner: &Command,
    redirects: &[crate::command::Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    with_redirect_scope(redirects, shell, sink, err_sink, |shell, inner_sink, inner_err_sink| {
        run_command(inner, shell, inner_sink, inner_err_sink)
    })
}

/// Runs a `while`/`until` loop. The body runs while the condition's
/// exit status satisfies the loop's polarity. `break` ends the loop;
/// `continue` jumps to the next condition test; `exit` propagates; a
/// pending SIGINT (Ctrl-C) ends the loop with status 130.
fn run_while(
    clause: &WhileClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_while_inner(clause, shell, sink, err_sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_while_inner(
    clause: &WhileClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let mut last = ExecOutcome::Continue(0);
    loop {
        if let Some(o) = check_interrupt(shell) {
            return o;
        }
        shell.err_suppressed_depth += 1;
        let cond = execute_sequence_body(&clause.condition, shell, sink, err_sink);
        shell.err_suppressed_depth -= 1;
        let keep_going = match cond {
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted(_) => {
                return cond;
            }
            ExecOutcome::Continue(c) => {
                if clause.until { c != 0 } else { c == 0 }
            }
        };
        if !keep_going {
            break;
        }
        match execute_sequence_body(&clause.body, shell, sink, err_sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak(1, st) => {
                last = ExecOutcome::Continue(st);
                break;
            }
            ExecOutcome::LoopBreak(n, st) => {
                return ExecOutcome::LoopBreak(n - 1, st);
            }
            ExecOutcome::LoopContinue(1) => {
                last = ExecOutcome::Continue(0);
                // fall through — the loop re-tests the condition
            }
            ExecOutcome::LoopContinue(n) => {
                return ExecOutcome::LoopContinue(n - 1);
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }
    }
    last
}

/// Runs a `for` loop. The word list is expanded once, up front; the
/// body then runs with the loop variable set to each value in turn.
/// `break` ends the loop, `continue` advances to the next value,
/// `exit` propagates, and a pending SIGINT (Ctrl-C) ends the loop
/// with status 130.
fn run_for(
    clause: &ForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_for_inner(clause, shell, sink, err_sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_for_inner(
    clause: &ForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // bash accepts any word as the loop variable at parse time but requires a
    // valid identifier at runtime; a bad name is a NON-FATAL error (status 1,
    // body not run, the surrounding list continues). Reserved words like `if`
    // are valid identifiers and fall through to run normally.
    if !crate::builtins::is_valid_name(&clause.var) {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: `{}': not a valid identifier", clause.var); }
        return ExecOutcome::Continue(1);
    }

    // Expand the word list once — the same path command arguments take.
    // The no-`in` form (`has_in == false`) iterates the positional
    // parameters ("$@"); an explicit empty `in` (`has_in == true`, empty
    // `words`) iterates nothing (M-24a, matching bash).
    let mut values: Vec<String> = Vec::new();
    if clause.has_in {
        for word in &clause.words {
            match glob_expand_word(word, shell, &mut *err_writer(err_sink, sink)) {
                Ok(v) => values.extend(v),
                Err(()) => return ExecOutcome::Continue(1),
            }
        }
    } else {
        values = shell.positional_args.clone();
    }

    let mut last = ExecOutcome::Continue(0);
    for value in values {
        if shell.shell_options.xtrace {
            let words = clause
                .words
                .iter()
                .map(crate::expand::reconstruct_word_source)
                .collect::<Vec<_>>()
                .join(" ");
            let body = if clause.has_in {
                format!("for {} in {}", clause.var, words)
            } else {
                format!("for {}", clause.var)
            };
            xtrace_compound(shell, &body);
        }
        if let Some(o) = check_interrupt(shell) {
            return o;
        }
        if shell.try_set(&clause.var, value).is_err() {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: readonly variable", clause.var); }
            shell.posix_fatal(127);
            return ExecOutcome::Continue(1);
        }
        match execute_sequence_body(&clause.body, shell, sink, err_sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak(1, st) => {
                last = ExecOutcome::Continue(st);
                break;
            }
            ExecOutcome::LoopBreak(n, st) => {
                return ExecOutcome::LoopBreak(n - 1, st);
            }
            ExecOutcome::LoopContinue(1) => {
                last = ExecOutcome::Continue(0);
                // fall through — advance to the next value
            }
            ExecOutcome::LoopContinue(n) => {
                return ExecOutcome::LoopContinue(n - 1);
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }
    }
    last
}

/// Default screen width when $COLUMNS is unset/invalid (bash uses 80).
const SELECT_DEFAULT_COLS: usize = 80;
const SELECT_TABSIZE: usize = 8;

/// Decimal digit count of `n` (bash NUMBER_LEN). n>=1 in practice.
fn number_len(n: usize) -> usize {
    let mut len = 1;
    let mut v = n;
    while v >= 10 {
        v /= 10;
        len += 1;
    }
    len
}

/// Display width of a menu item. ASCII-exact (codepoint count); wide-char
/// width is a documented sub-divergence (see spec).
fn select_displen(s: &str) -> usize {
    s.chars().count()
}

/// Pad column position `from` up to `to` exactly as bash's `indent()`:
/// emit a tab when crossing an 8-column tab stop, else a space.
fn select_indent(out: &mut String, mut from: usize, to: usize) {
    while from < to {
        if to / SELECT_TABSIZE > from / SELECT_TABSIZE {
            out.push('\t');
            from += SELECT_TABSIZE - (from % SELECT_TABSIZE);
        } else {
            out.push(' ');
            from += 1;
        }
    }
}

/// Render the numbered `select` menu byte-for-byte like bash 5.2's
/// `print_select_list`. `cols_width` is the screen width (COLS). The returned
/// string (with a trailing newline per row) is written to stderr by the caller.
fn format_select_menu(items: &[String], cols_width: usize) -> String {
    let mut out = String::new();
    let list_len = items.len();
    if list_len == 0 {
        // bash print_select_list: `if (list == 0) { putc('\n', stderr); return; }` — emit one newline.
        // (In practice run_select guards empty lists before calling this.)
        out.push('\n');
        return out;
    }
    let indices_len = number_len(list_len);
    let max_item = items.iter().map(|s| select_displen(s)).max().unwrap_or(0);
    // RP_SPACE_LEN (") ") = 2, plus bash's extra +2 gap.
    let max_elem_len = max_item + indices_len + 2 + 2;

    // max_elem_len >= 4 (indices_len >= 1, + RP_SPACE_LEN 2, + gap 2); safe to divide.
    let mut cols = cols_width / max_elem_len;
    if cols == 0 {
        cols = 1;
    }
    let mut rows = list_len.div_ceil(cols);
    cols = list_len.div_ceil(rows);
    if rows == 1 {
        rows = cols;
        // After the flip, rows == item count and each row holds one item. cols is
        // intentionally not set to 1: the inner loop advances ind by rows (now the
        // full count), so ind >= list_len after the first item in every row.
    }
    let first_col_iw = number_len(rows);
    let other_iw = indices_len;

    for row in 0..rows {
        let mut ind = row;
        let mut pos = 0usize;
        loop {
            let iw = if pos == 0 { first_col_iw } else { other_iw };
            let item = &items[ind];
            // bash print_index_and_element: "%*d" + ") " + item
            out.push_str(&format!("{:>width$}) {}", ind + 1, item, width = iw));
            let elem_len = select_displen(item) + iw + 2;
            ind += rows;
            if ind >= list_len {
                break;
            }
            select_indent(&mut out, pos + elem_len, pos + max_elem_len);
            pos += max_elem_len;
        }
        out.push('\n');
    }
    out
}

/// Runs a standalone `((expr))` arith command. Per bash semantics, the
/// command exits 0 if the expression's value is non-zero, 1 if zero;
/// arith errors emit a diagnostic to stderr and exit 1.
fn run_arith(
    body: &crate::lexer::Word,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source_inner(body)));
    let (src, res) = crate::expand::eval_arith_word_src(body, shell);
    match res {
        Ok(0) => ExecOutcome::Continue(1),
        Ok(_) => ExecOutcome::Continue(0),
        Err(e) => {
            let prefix = shell.error_prefix(Some("(("));
            { let mut err = err_writer(err_sink, sink);
              e!(&mut *err, "{prefix}{}", crate::arith::render_error_body(&src, &e)); }
            ExecOutcome::Continue(1)
        }
    }
}

/// Runs a C-style `for ((init; cond; step)) do BODY done` arith
/// for-loop. Evaluates `init` once, then loops while `cond` is non-zero
/// (None = always true). Evaluates `step` after each iteration body.
/// Mirrors `run_for`'s break/continue/return/exit/SIGINT handling.
fn run_arith_for(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_arith_for_inner(clause, shell, sink, err_sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_arith_for_inner(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {

    // 1. Eval init once (if present).
    if let Some(init) = &clause.init {
        xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source_inner(init)));
    }
    if let Some(init) = &clause.init
        && let Err(e) = crate::expand::eval_arith_word(init, shell)
    {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ((: {e}"); }
        return ExecOutcome::Continue(1);
    }

    let mut last = ExecOutcome::Continue(0);
    loop {
        // SIGINT check (mirrors run_for).
        if let Some(o) = check_interrupt(shell) {
            return o;
        }

        if let Some(c) = &clause.cond {
            xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source_inner(c)));
        }
        // 2. Eval cond. Empty cond = always true (matches bash).
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::expand::eval_arith_word(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ((: {e}"); }
                    return ExecOutcome::Continue(1);
                }
            },
        };
        if cond_value == 0 {
            break;
        }

        // 3. Execute body.
        match execute_sequence_body(&clause.body, shell, sink, err_sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak(1, st) => {
                last = ExecOutcome::Continue(st);
                break;
            }
            ExecOutcome::LoopBreak(n, st) => {
                return ExecOutcome::LoopBreak(n - 1, st);
            }
            ExecOutcome::LoopContinue(1) => {
                last = ExecOutcome::Continue(0);
                // fall through to step (LoopContinue(1) runs this loop's step)
            }
            ExecOutcome::LoopContinue(n) => {
                // Skip this loop's step — bubble to outer loop.
                return ExecOutcome::LoopContinue(n - 1);
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }

        // 4. Eval step (if present).
        if let Some(step) = &clause.step {
            xtrace_compound(shell, &format!("(( {} ))", crate::expand::reconstruct_word_source_inner(step)));
        }
        if let Some(step) = &clause.step
            && let Err(e) = crate::expand::eval_arith_word(step, shell)
        {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ((: {e}"); }
            return ExecOutcome::Continue(1);
        }
    }
    last
}

/// Reads one line from stdin into `REPLY` via the `read` builtin's
/// no-NAME path. Returns the builtin's outcome: `Continue(0)` on success
/// (REPLY set to the raw line, possibly empty), `Continue(1)` on EOF
/// (nothing read).
fn read_line_into_reply(shell: &mut Shell) -> ExecOutcome {
    let mut devnull: Vec<u8> = Vec::new();
    let mut err = io::stderr();
    crate::builtins::run_builtin("read", &[], &mut devnull, &mut err, shell)
}

/// Runs a `select NAME [in WORDS]; do BODY; done` menu loop, mirroring
/// bash 5.2's `execute_select_command`/`select_query`. The numbered menu
/// (via `format_select_menu`) and the `PS3` prompt go to stderr; one line
/// is read into `REPLY` per prompt via the `read` builtin. An empty list
/// runs the body zero times; `break`/`continue N` bubble via the v79
/// loop infrastructure. Wrapped to keep a single `loop_depth` return path.
fn run_select(
    clause: &crate::command::SelectClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_select_inner(clause, shell, sink, err_sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_select_inner(
    clause: &crate::command::SelectClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {

    // 1. Build the item list: expand `in WORDS` (Some), or "$@" (None).
    let items: Vec<String> = match &clause.words {
        Some(words) => {
            let mut v = Vec::new();
            for w in words {
                match glob_expand_word(w, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(g) => v.extend(g),
                    Err(()) => return ExecOutcome::Continue(1),
                }
            }
            v
        }
        None => shell.positional_args.clone(),
    };

    if shell.shell_options.xtrace {
        let body = match &clause.words {
            Some(words) => format!(
                "select {} in {}",
                clause.var,
                words.iter().map(crate::expand::reconstruct_word_source)
                    .collect::<Vec<_>>().join(" ")
            ),
            None => format!("select {}", clause.var),
        };
        xtrace_compound(shell, &body);
    }

    // 2. Empty list → body never runs (bash returns the loop's last status, 0).
    if items.is_empty() {
        return ExecOutcome::Continue(0);
    }

    // 3. Screen width: $COLUMNS if a positive integer, else the default (80).
    let cols_width = shell
        .lookup_var("COLUMNS")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(SELECT_DEFAULT_COLS);

    // bash returns the read failure (1) if EOF hits before any body runs;
    // otherwise the last body status.
    let mut last = ExecOutcome::Continue(1);
    let mut show_menu = true;

    loop {
        // 3a. PS3 (default "#? ").
        let ps3 = shell.lookup_var("PS3").unwrap_or_else(|| "#? ".to_string());

        // 3b. select_query: (re)print menu when show_menu, prompt, read one
        //     line. An empty line reprints the menu; EOF terminates the loop.
        let selection: String = loop {
            {
                let mut err = err_writer(err_sink, sink);
                if show_menu {
                    let _ = write!(&mut *err, "{}", format_select_menu(&items, cols_width));
                }
                let _ = write!(&mut *err, "{ps3}");
                let _ = err.flush();
            }

            let r = read_line_into_reply(shell);
            if !matches!(r, ExecOutcome::Continue(0)) {
                // EOF / read failure → write a newline to stdout (bash) and
                // terminate the loop with the last status (read failure if no
                // body ran).
                match sink {
                    StdoutSink::Terminal => {
                        let _ = writeln!(io::stdout());
                    }
                    StdoutSink::Capture(buf) => {
                        buf.push(b'\n');
                        crate::callbacks_thread_local::with_callbacks(|cb| {
                            if let Some(cb) = cb {
                                cb.push_stdout(b"\n");
                            }
                        });
                    }
                }
                return last;
            }
            let reply = shell.lookup_var("REPLY").unwrap_or_default();
            if reply.is_empty() {
                show_menu = true;
                continue; // reprint menu, re-prompt
            }
            // 1-based index; invalid / out-of-range → empty NAME (body runs).
            match reply.trim().parse::<usize>() {
                Ok(n) if n >= 1 && n <= items.len() => break items[n - 1].clone(),
                _ => break String::new(),
            }
        };

        // 3c. Bind NAME (honor readonly like the other loop runners).
        if shell.try_set(&clause.var, selection).is_err() {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: readonly variable", clause.var); }
            return ExecOutcome::Continue(1);
        }

        // 3d. SIGINT check (mirror run_for).
        if let Some(o) = check_interrupt(shell) {
            return o;
        }

        // 3e. Run the body; bubble flow with the v79 decrement-and-bubble pattern.
        match execute_sequence_body(&clause.body, shell, sink, err_sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak(1, st) => {
                last = ExecOutcome::Continue(st);
                break;
            }
            ExecOutcome::LoopBreak(n, st) => return ExecOutcome::LoopBreak(n - 1, st),
            ExecOutcome::LoopContinue(1) => {
                last = ExecOutcome::Continue(0);
                // fall through — re-prompt
            }
            ExecOutcome::LoopContinue(n) => return ExecOutcome::LoopContinue(n - 1),
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
            ExecOutcome::Continue(c) => last = ExecOutcome::Continue(c),
        }

        // 3f. Suppress the menu next iteration unless the last REPLY was empty
        //     (KSH_COMPATIBLE_SELECT, bash 5.2's default). An empty REPLY was
        //     handled inside the inner loop (it reprints there), so a body
        //     iteration always means a non-empty REPLY → suppress.
        show_menu = false;
    }
    last
}

/// Matches `subject` against a `case` clause's `|`-patterns. A clause
/// matches if any pattern matches; an unparseable glob matches nothing.
fn case_item_matches(item: &CaseItem, subject: &str, shell: &mut Shell) -> bool {
    let nocase = shell.nocasematch();
    let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
    for pattern_word in &item.patterns {
        let pattern = expand_pattern(pattern_word, shell);
        let hit = if (extglob && crate::glob_match::has_extglob(&pattern))
            || crate::glob_match::has_posix_class(&pattern)
        {
            crate::glob_match::extglob_match(&pattern, subject, nocase)
        } else {
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            glob::Pattern::new(&npat)
                .map(|p| {
                    p.matches_with(
                        subject,
                        glob::MatchOptions {
                            case_sensitive: !nocase,
                            require_literal_separator: false,
                            require_literal_leading_dot: false,
                        },
                    )
                })
                .unwrap_or(false)
        };
        if hit {
            return true;
        }
    }
    false
}

/// Runs a `case` statement. The subject is expanded once; clauses are
/// walked in order. The first matching clause's body runs, then the
/// terminator decides what happens: `;;` stops, `;&` runs the next
/// clause's body unconditionally, `;;&` resumes pattern-testing.
/// `case` is not a loop — `break`/`continue` propagate out unchanged.
fn run_case(
    clause: &CaseClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let subject = expand_assignment(&clause.subject, shell);
    xtrace_compound(
        shell,
        &format!("case {} in", crate::expand::reconstruct_word_source(&clause.subject)),
    );
    let mut last = ExecOutcome::Continue(0);
    let mut i = 0;
    let mut fall_through = false;
    while i < clause.items.len() {
        let item = &clause.items[i];
        let run_this = fall_through || case_item_matches(item, &subject, shell);
        if let Some(status) = shell.pending_fatal_status {
            return ExecOutcome::Continue(status);
        }
        if !run_this {
            i += 1;
            continue;
        }
        match &item.body {
            None => last = ExecOutcome::Continue(0),
            Some(body) => match execute_sequence_body(body, shell, sink, err_sink) {
                ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
                ExecOutcome::LoopBreak(n, st) => return ExecOutcome::LoopBreak(n, st),
                ExecOutcome::LoopContinue(n) => return ExecOutcome::LoopContinue(n),
                ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
                ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
                ExecOutcome::Continue(c) => last = ExecOutcome::Continue(c),
            },
        }
        match item.terminator {
            CaseTerminator::Break => return last,
            CaseTerminator::FallThrough => {
                fall_through = true;
                i += 1;
            }
            CaseTerminator::ContinueMatch => {
                fall_through = false;
                i += 1;
            }
        }
    }
    last
}

/// Runs an `if` clause: evaluate the condition, then run the first
/// branch whose condition succeeds (exit 0), or the `else` body, or
/// nothing (status 0). An `exit` anywhere inside propagates.
fn run_if(
    clause: &IfClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    shell.err_suppressed_depth += 1;
    let cond = execute_sequence_body(&clause.condition, shell, sink, err_sink);
    shell.err_suppressed_depth -= 1;
    if matches!(
        cond,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted(_)
    ) {
        return cond;
    }
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink, err_sink);
    }
    for elif in &clause.elif_branches {
        shell.err_suppressed_depth += 1;
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink, err_sink);
        shell.err_suppressed_depth -= 1;
        if matches!(
            elif_cond,
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted(_)
        ) {
            return elif_cond;
        }
        if matches!(elif_cond, ExecOutcome::Continue(0)) {
            return execute_sequence_body(&elif.body, shell, sink, err_sink);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell, sink, err_sink);
    }
    ExecOutcome::Continue(0)
}

// ──────────────────────────────────────────────────────────────
// v30: `[[ ]]` extended test evaluator
// ──────────────────────────────────────────────────────────────

fn run_double_bracket(
    expr: &TestExpr,
    inline_assignments: &[crate::command::Assignment],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let snap = match apply_inline_assignments(inline_assignments, shell, sink, err_sink) {
        Ok(s) => s,
        Err(s) => {
            restore_inline_assignments(s, shell);
            return ExecOutcome::Continue(1);
        }
    };
    let result = match eval_test_expr(expr, shell) {
        Ok(true)  => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg)  => {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: [[: {msg}"); }
            ExecOutcome::Continue(2)
        }
    };
    restore_inline_assignments(snap, shell);
    result
}

fn test_unary_op_str(op: crate::command::TestUnaryOp) -> &'static str {
    use crate::command::TestUnaryOp as U;
    match op {
        U::FileExists => "-e", U::IsRegFile => "-f", U::IsDir => "-d",
        U::IsReadable => "-r", U::IsWritable => "-w", U::IsExecutable => "-x",
        U::IsNonEmpty => "-s", U::IsSymlink => "-L", U::StringNonEmpty => "-n",
        U::StringEmpty => "-z", U::VarSet => "-v", U::OptEnabled => "-o",
        U::IsFifo => "-p", U::IsSocket => "-S", U::IsBlockDev => "-b",
        U::IsCharDev => "-c", U::OwnedByEuid => "-O", U::OwnedByEgid => "-G",
        U::NewerThanRead => "-N", U::IsSticky => "-k", U::IsSetuid => "-u",
        U::IsSetgid => "-g", U::IsTerminal => "-t",
    }
}

fn test_binary_op_str(op: crate::command::TestBinaryOp) -> &'static str {
    use crate::command::TestBinaryOp as B;
    match op {
        B::StringEq => "==", B::StringNe => "!=", B::StringLt => "<", B::StringGt => ">",
        B::IntEq => "-eq", B::IntNe => "-ne", B::IntLt => "-lt", B::IntGt => "-gt",
        B::IntLe => "-le", B::IntGe => "-ge", B::NewerThan => "-nt",
        B::OlderThan => "-ot", B::SameFile => "-ef",
    }
}

/// bash shows an empty `[[ ]]` operand as `''` and a non-empty one raw.
fn xtrace_operand(s: &str) -> String {
    if s.is_empty() { "''".to_string() } else { s.to_string() }
}

/// Render the `[[ … ]]` body for a single leaf (operands EXPANDED), for `set -x`.
fn render_test_leaf(expr: &TestExpr, shell: &mut Shell) -> String {
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            format!("{} {}", test_unary_op_str(*op), xtrace_operand(&s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            let r = expand_assignment(rhs, shell);
            format!("{} {} {}", xtrace_operand(&l), test_binary_op_str(*op), xtrace_operand(&r))
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            format!("{} =~ {}", xtrace_operand(&l), xtrace_operand(&p))
        }
        TestExpr::Not(_) | TestExpr::And(_, _) | TestExpr::Or(_, _) => String::new(),
    }
}

fn eval_test_expr(expr: &TestExpr, shell: &mut Shell) -> Result<bool, String> {
    eval_test_expr_traced(expr, shell, false)
}

fn eval_test_expr_traced(expr: &TestExpr, shell: &mut Shell, suppress: bool) -> Result<bool, String> {
    if !suppress
        && shell.shell_options.xtrace
        && matches!(expr, TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. })
    {
        let body = render_test_leaf(expr, shell);
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}[[ {body} ]]"));
    }
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            if matches!(op, TestUnaryOp::VarSet) {
                return Ok(shell.element_or_var_is_set(&s));
            }
            if matches!(op, TestUnaryOp::OptEnabled) {
                return Ok(crate::builtins::option_get(shell, &s).unwrap_or(false));
            }
            Ok(eval_unary(*op, &s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            eval_binary(*op, &l, rhs, shell)
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            // A QUOTED span of the operand matches literally (regex metachars
            // escaped); an unquoted span stays an active regex (bash 3.2+, L-23).
            let p = crate::expand::expand_regex_operand(pattern, shell);
            let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            match re.captures(&l) {
                Some(caps) => {
                    // BASH_REMATCH[0] = whole matched substring; [1..] = capture
                    // groups (a non-participating group is "" but still indexed).
                    let map: std::collections::BTreeMap<usize, String> = (0..caps.len())
                        .map(|i| {
                            (
                                i,
                                caps.get(i)
                                    .map(|m| m.as_str().to_string())
                                    .unwrap_or_default(),
                            )
                        })
                        .collect();
                    let _ = shell.replace_indexed("BASH_REMATCH", map);
                    Ok(true)
                }
                None => {
                    // bash clears BASH_REMATCH to an empty array on no match.
                    let _ = shell.replace_indexed("BASH_REMATCH", std::collections::BTreeMap::new());
                    Ok(false)
                }
            }
        }
        TestExpr::Not(inner) => {
            if !suppress
                && shell.shell_options.xtrace
                && matches!(**inner, TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. })
            {
                let body = render_test_leaf(inner, shell);
                let p4 = ps4(shell);
                xtrace_emit(&format!("{p4}[[ ! {body} ]]"));
                return eval_test_expr_traced(inner, shell, true).map(|b| !b);
            }
            eval_test_expr_traced(inner, shell, suppress).map(|b| !b)
        }
        TestExpr::And(a, b) => {
            if eval_test_expr_traced(a, shell, false)? {
                eval_test_expr_traced(b, shell, false)
            } else {
                Ok(false)
            }
        }
        TestExpr::Or(a, b) => {
            if eval_test_expr_traced(a, shell, false)? {
                Ok(true)
            } else {
                eval_test_expr_traced(b, shell, false)
            }
        }
    }
}

fn eval_unary(op: TestUnaryOp, s: &str) -> bool {
    use crate::test_builtin;
    match op {
        TestUnaryOp::StringNonEmpty => !s.is_empty(),
        TestUnaryOp::StringEmpty    => s.is_empty(),
        // Delegate all file tests to the shared test_builtin logic.
        TestUnaryOp::FileExists   => test_builtin::evaluate(&["-e".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsRegFile    => test_builtin::evaluate(&["-f".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsDir        => test_builtin::evaluate(&["-d".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsReadable   => test_builtin::evaluate(&["-r".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsWritable   => test_builtin::evaluate(&["-w".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsExecutable => test_builtin::evaluate(&["-x".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsNonEmpty   => test_builtin::evaluate(&["-s".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSymlink    => test_builtin::evaluate(&["-L".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsFifo       => test_builtin::evaluate(&["-p".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSocket     => test_builtin::evaluate(&["-S".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsBlockDev   => test_builtin::evaluate(&["-b".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsCharDev    => test_builtin::evaluate(&["-c".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::OwnedByEuid  => test_builtin::evaluate(&["-O".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::OwnedByEgid  => test_builtin::evaluate(&["-G".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::NewerThanRead => test_builtin::evaluate(&["-N".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSticky     => test_builtin::evaluate(&["-k".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSetuid     => test_builtin::evaluate(&["-u".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsSetgid     => test_builtin::evaluate(&["-g".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::IsTerminal   => test_builtin::evaluate(&["-t".to_string(), s.to_string()]).unwrap_or(false),
        TestUnaryOp::VarSet       => unreachable!("VarSet handled in eval_test_expr"),
        TestUnaryOp::OptEnabled   => unreachable!("OptEnabled handled in eval_test_expr"),
    }
}

/// Arithmetic-evaluate a `[[ ]]` integer-comparison operand string. An empty
/// operand is 0 (bash). Errors render the same as the arith evaluator does
/// elsewhere (the caller wraps the `Err(String)` into a `[[`-prefixed message).
fn arith_eval_operand(s: &str, shell: &mut Shell) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    crate::arith::parse(t)
        .and_then(|e| crate::arith::eval(&e, shell))
        .map_err(|e| format!("{e}"))
}

fn eval_binary(
    op: TestBinaryOp,
    lhs: &str,
    rhs_word: &crate::lexer::Word,
    shell: &mut Shell,
) -> Result<bool, String> {
    match op {
        TestBinaryOp::StringEq | TestBinaryOp::StringNe => {
            let pattern_str = expand_pattern(rhs_word, shell);
            let nocase = shell.nocasematch();
            let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
            let matched = if (extglob && crate::glob_match::has_extglob(&pattern_str))
                || crate::glob_match::has_posix_class(&pattern_str)
            {
                crate::glob_match::extglob_match(&pattern_str, lhs, nocase)
            } else {
                let npat = crate::glob_match::translate_bracket_negation(&pattern_str);
                let pat = glob::Pattern::new(&npat)
                    .map_err(|e| format!("bad pattern: {e}"))?;
                pat.matches_with(lhs, glob::MatchOptions {
                    case_sensitive: !nocase,
                    require_literal_separator: false,
                    require_literal_leading_dot: false,
                })
            };
            Ok(if matches!(op, TestBinaryOp::StringEq) { matched } else { !matched })
        }
        TestBinaryOp::StringLt | TestBinaryOp::StringGt => {
            let rhs = expand_assignment(rhs_word, shell);
            Ok(match op {
                TestBinaryOp::StringLt => lhs < rhs.as_str(),
                TestBinaryOp::StringGt => lhs > rhs.as_str(),
                _ => unreachable!(),
            })
        }
        TestBinaryOp::IntEq
        | TestBinaryOp::IntNe
        | TestBinaryOp::IntLt
        | TestBinaryOp::IntGt
        | TestBinaryOp::IntLe
        | TestBinaryOp::IntGe => {
            let rhs = expand_assignment(rhs_word, shell);
            // In `[[ ]]`, the arithmetic comparison ops evaluate BOTH operands
            // as arithmetic expressions: a bare variable name resolves to its
            // value, `2+3` -> 5, and an unset/empty operand -> 0.
            let l: i64 = arith_eval_operand(lhs, shell)?;
            let r: i64 = arith_eval_operand(&rhs, shell)?;
            Ok(match op {
                TestBinaryOp::IntEq => l == r,
                TestBinaryOp::IntNe => l != r,
                TestBinaryOp::IntLt => l < r,
                TestBinaryOp::IntGt => l > r,
                TestBinaryOp::IntLe => l <= r,
                TestBinaryOp::IntGe => l >= r,
                _ => unreachable!(),
            })
        }
        TestBinaryOp::NewerThan | TestBinaryOp::OlderThan | TestBinaryOp::SameFile => {
            let rhs = expand_assignment(rhs_word, shell);
            let op_str = match op {
                TestBinaryOp::NewerThan => "-nt",
                TestBinaryOp::OlderThan => "-ot",
                TestBinaryOp::SameFile => "-ef",
                _ => unreachable!(),
            };
            Ok(crate::test_builtin::compare_files(op_str, lhs, &rhs))
        }
    }
}

fn run_pipeline(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage pipeline: run directly in the parent shell (no fork needed).
        // This covers both Simple commands and compound commands as single stages.
        run_command(&pipeline.commands[0], shell, sink, err_sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink, err_sink)
    };
    if pipeline.negate {
        // Negate the exit status only; $PIPESTATUS (set by the stage(s) above)
        // stays raw, and control-flow outcomes propagate unchanged.
        if let ExecOutcome::Continue(s) = outcome {
            return ExecOutcome::Continue(if s == 0 { 1 } else { 0 });
        }
    }
    outcome
}

/// True if `cmd` is a `!`-negated pipeline — exempt from `set -e`/ERR (bash).
fn is_negated_pipeline(cmd: &Command) -> bool {
    matches!(cmd, Command::Pipeline(p) if p.negate)
}

// ----- background pipeline --------------------------------------------------

/// Backgrounds a `Command::Subshell` via fork + job registration.
/// Used by `execute()` when `seq.background` is set and `seq.first` is
/// `Command::Subshell`.  Does NOT waitpid — returns immediately after
/// registering the job.
fn run_background_subshell(
    cmd: &Command,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);
    let job_control = shell.job_control_active();
    // Inherit stdin from the terminal (unlike pipeline backgrounds that use
    // /dev/null) — match bash/dash behaviour for `(cmd) &`.
    match fork_and_run_in_subshell(
        cmd,
        shell,
        libc::STDIN_FILENO,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
        /*parent_fds_to_close=*/ &[],
        None, // no Dup redirect at this call site
        None,
    ) {
        Ok(pid) => {
            shell.last_bg_pid = Some(pid);
            let id = shell.jobs.add_with_pgroup(pid, vec![pid], display, job_control);
            // bash suppresses automatic job notices inside a subshell environment / completion funcs
            if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "[{id}] {pid}"); }
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: fork: {e}"); }
            ExecOutcome::Continue(1)
        }
    }
}

fn run_background_sequence(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);
    let job_control = shell.job_control_active();

    // Spawn each stage using the same per-stage fork dispatch as run_multi_stage
    // (classify_stage → External via spawn_external_with_fds, or InProcess via
    // fork_and_run_in_subshell). This handles all Command variants including
    // compound commands (if/while/for/case/brace-group), so there are no
    // unreachable! arms. After all stages are spawned, register the job and
    // return immediately (no wait) — that's what makes this "background".
    //
    // Background stdin default: /dev/null for stage 0 (no explicit redirect,
    // no previous pipe) so the job doesn't compete for the terminal.

    let n = pipeline.commands.len();
    let mut spawned_pids: Vec<i32> = Vec::with_capacity(n);
    let mut first_pid: Option<i32> = None;
    let mut prev_pipe_read: Option<RawFd> = None;
    let mut parent_held: Vec<RawFd> = Vec::new();

    // Open /dev/null once for the first stage's default stdin.
    let devnull_fd: RawFd = {
        use std::os::unix::io::IntoRawFd;
        match File::open("/dev/null") {
            Ok(f) => f.into_raw_fd(),
            Err(e) => {
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: /dev/null: {e}"); }
                return ExecOutcome::Continue(1);
            }
        }
    };
    parent_held.push(devnull_fd);

    // Snapshot the procsub stack. Word expansion in the spawn loop may realize
    // process substitutions (pushing onto shell.procsub_pending). We must drain
    // [procsub_base..] on every exit path so the parent fd is closed and the
    // inner child is reaped. For background jobs we use a non-blocking reap after
    // spawning (see drain_procsubs_nonblocking below) so we don't stall the
    // background path on a long-running inner producer.
    let procsub_base = shell.procsub_pending.len();

    for (i, stage_cmd) in pipeline.commands.iter().enumerate() {
        let is_last = i == n - 1;

        // ---- Assign-only stages: no-op ----------------------------------------
        if let Command::Simple(SimpleCommand::Assign(items, aline)) = stage_cmd {
            // Drop incoming pipe (no-op stage produces no output).
            if let Some(r) = prev_pipe_read.take() {
                parent_held.retain(|&fd| fd != r);
                unsafe { libc::close(r); }
            }
            // Run via fork so it's isolated (assignments don't affect parent).
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone(), *aline));
            let pgid_target = if job_control { first_pid.unwrap_or(0) } else { NO_PGROUP };
            let stdin_fd = devnull_fd; // stage 0 default (overridden below if not first)
            // For a no-op assign stage, stdout is irrelevant but we still need
            // to either pipe or close it for downstream stages.
            let stdout_fd = if !is_last {
                match make_pipe() {
                    Ok((r, w)) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                        parent_held.push(w);
                        w
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        drain_procsubs(shell, procsub_base);
                        cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                        for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            } else {
                libc::STDOUT_FILENO
            };
            let fds_to_close: Vec<RawFd> = parent_held.iter().copied()
                .filter(|&fd| fd != stdout_fd && fd != stdin_fd)
                .collect();
            match fork_and_run_in_subshell(&assign_cmd, shell, stdin_fd, stdout_fd, libc::STDERR_FILENO, pgid_target, &fds_to_close, None, None) {
                Ok(pid) => {
                    if stdout_fd > 2 {
                        parent_held.retain(|&fd| fd != stdout_fd);
                        unsafe { libc::close(stdout_fd); }
                    }
                    if first_pid.is_none() {
                        first_pid = Some(pid);
                        if job_control {
                            unsafe {
                                if libc::setpgid(pid, pid) != 0 {
                                    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                                    debug_assert!(errno == libc::ESRCH || errno == libc::EACCES,
                                        "setpgid({pid},{pid}) failed errno {errno}");
                                }
                            }
                        }
                    }
                    spawned_pids.push(pid);
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: fork: {e}"); }
                    if stdout_fd > 2 { unsafe { libc::close(stdout_fd); } }
                    drain_procsubs(shell, procsub_base);
                    cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
            continue;
        }

        // ---- Inline assignments (v23 scoping) ---------------------------------
        let inline_assignments: &[crate::command::Assignment] =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                &exec.inline_assignments
            } else {
                &[]
            };
        let snap = match apply_inline_assignments(inline_assignments, shell, sink, err_sink) {
            Ok(s) => s,
            Err(s) => {
                restore_inline_assignments(s, shell);
                drain_procsubs(shell, procsub_base);
                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                return ExecOutcome::Continue(1);
            }
        };

        // ---- Stdin fd ---------------------------------------------------------
        // A heredoc/herestring stdin is fed by a forked writer process (M-120);
        // the read end becomes this stage's stdin and the parent holds no write
        // end.
        let stdin_fd: RawFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.slot_stdin() {
                Some(Redirect::Read(word)) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    };
                    use std::os::unix::io::IntoRawFd;
                    match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                Some(Redirect::Heredoc { body, .. }) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    // Forked writer (M-120): the read end is this stage's stdin;
                    // the writer process is an internal helper collected by the
                    // existing SIGCHLD reaper — never a job, never $!, so it is
                    // NOT added to spawned_pids/first_pid.
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, _pid)) => r,
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                Some(Redirect::HereString(body)) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    // Here-string: expand with no split/glob + trailing newline,
                    // then feed via a forked writer (M-120; see Heredoc above).
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, _pid)) => r,
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                _ => prev_pipe_read.take().unwrap_or(devnull_fd),
            }
        } else {
            // Compound stage: use prev pipe or /dev/null for stage 0.
            prev_pipe_read.take().unwrap_or(devnull_fd)
        };

        // Remove stdin_fd from parent_held if it was tracked there.
        parent_held.retain(|&fd| fd != stdin_fd);

        // ---- Stdout redirect (ExecCommand only) ------------------------------
        let explicit_stdout_fd: Option<RawFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stdout() {
                    Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        let guard = shell.shell_options.noclobber
                            && !matches!(r, Redirect::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Stderr redirect (ExecCommand only) ------------------------------
        let explicit_stderr_fd: Option<RawFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stderr() {
                    Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        let guard = shell.shell_options.noclobber
                            && !matches!(r, Redirect::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Stdout fd -------------------------------------------------------
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            // Upstream stdout goes to the file. For a non-final stage we
            // STILL need to create an inter-stage pipe so the downstream
            // stage reads EOF instead of inheriting parent stdin (M-125).
            if !is_last {
                match make_orphan_pipe_for_eof_reader() {
                    Ok(r) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        restore_inline_assignments(snap, shell);
                        if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                        if let Some(efd) = explicit_stderr_fd { unsafe { libc::close(efd); } }
                        unsafe { libc::close(fd); } // close the open file fd we won't use
                        drain_procsubs(shell, procsub_base);
                        cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                        for pfd in parent_held.drain(..) { unsafe { libc::close(pfd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            fd
        } else if !is_last {
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            libc::STDOUT_FILENO
        };

        let stderr_fd = explicit_stderr_fd.unwrap_or(libc::STDERR_FILENO);

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if job_control { first_pid.unwrap_or(0) } else { NO_PGROUP };

        let fds_to_close_in_child: Vec<RawFd> = parent_held.iter().copied()
            .filter(|&fd| fd != stdout_fd && fd != stdin_fd && fd != stderr_fd)
            .collect();
        // (Any heredoc pipe write end lives in the forked writer process, not
        // here, so there is nothing extra to add to fds_to_close_in_child.)

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.slot_stdout() {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.slot_stderr() {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                (sdt, sedt)
            } else {
                (None, None)
            };

        let went_external;
        let spawn_result = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => {
                went_external = true;
                spawn_external_with_fds(simple, shell, sink, err_sink, stdin_fd, stdout_fd, stderr_fd, pgid_target, &fds_to_close_in_child)
            }
            StageKind::InProcess(cmd) => {
                went_external = false;
                fork_and_run_in_subshell(cmd, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &fds_to_close_in_child, stdout_dup_target, stderr_dup_target)
            }
        };

        restore_inline_assignments(snap, shell);

        let pid = match spawn_result {
            Ok(p) => p,
            Err(e) => {
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                if !went_external {
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if stdout_fd > 2 { unsafe { libc::close(stdout_fd); } }
                    if stderr_fd > 2 { unsafe { libc::close(stderr_fd); } }
                }
                for fd in [stdout_fd, stdin_fd, stderr_fd] {
                    if fd > 2 { parent_held.retain(|&x| x != fd); }
                }
                drain_procsubs(shell, procsub_base);
                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                return ExecOutcome::Continue(1);
            }
        };

        // Close parent copies of fds given to child.
        if !went_external {
            if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
            if stderr_fd > 2 { unsafe { libc::close(stderr_fd); } }
        }
        if stdout_fd > 2 {
            parent_held.retain(|&fd| fd != stdout_fd);
            if !went_external {
                unsafe { libc::close(stdout_fd); }
            }
        }

        // (A heredoc/herestring body, if any, is written by the forked writer
        // process; the parent holds no write end and nothing to record here.)

        // Track pgrp + pid.
        if first_pid.is_none() {
            first_pid = Some(pid);
            if job_control {
                unsafe {
                    if libc::setpgid(pid, pid) != 0 {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        debug_assert!(
                            errno == libc::ESRCH || errno == libc::EACCES,
                            "setpgid({pid},{pid}) failed errno {errno}"
                        );
                    }
                }
            }
        }
        spawned_pids.push(pid);
    }

    // Close all remaining parent-held fds (inter-stage pipe read-ends that
    // weren't consumed, and the /dev/null fd).
    for fd in parent_held.drain(..) {
        unsafe { libc::close(fd); }
    }

    // (Heredoc/herestring bodies are written by their forked writer processes,
    // reaped by the existing SIGCHLD reaper — they are internal helpers, not
    // part of this backgrounded job.)

    let Some(pgid) = first_pid else {
        // No actual children spawned (all-Assign pipeline). Treat as
        // synthetic Done. This shouldn't happen in practice — the parser
        // doesn't produce all-Assign backgrounded pipelines as a typical
        // user input shape, but we handle it defensively.
        shell.jobs.add_synthetic_done(display, 0);
        crate::jobs::reap_and_notify(shell);
        // Non-blocking drain: close parent fds so any inner child sees EOF.
        drain_procsubs_nonblocking(shell, procsub_base);
        return ExecOutcome::Continue(0);
    };

    let last_pid = *spawned_pids.last().unwrap();
    shell.last_bg_pid = Some(last_pid);
    let id = shell.jobs.add_with_pgroup(pgid, spawned_pids, display, job_control);
    // bash suppresses automatic job notices inside a subshell environment / completion funcs
    if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "[{id}] {last_pid}"); }
    }
    // Non-blocking drain: close parent fds and attempt WNOHANG reap of inner
    // procsub children. We don't block here because a long-running inner producer
    // (e.g. `cmd < <(long-running-gen) &`) should not make the background job
    // synchronous. Any child not yet exited is left to SIGCHLD/exit cleanup.
    drain_procsubs_nonblocking(shell, procsub_base);
    ExecOutcome::Continue(0)
}

/// Cleans up stages spawned during a background pipeline that failed to start
/// completely. Signals the whole process group (catching any double-forked
/// grandchildren), then reaps each pid via waitpid so we don't leave zombies.
fn cleanup_partial_pipeline_raw(pgid: Option<i32>, pids: &[i32]) {
    if let Some(pg) = pgid {
        unsafe {
            libc::killpg(pg, libc::SIGKILL);
        }
    }
    // Also SIGKILL each pid directly. When job control is off the stages share
    // the shell's group (no dedicated group to killpg), so the killpg above is a
    // no-op (ESRCH) and the blocking waitpid below would otherwise hang on a
    // still-running stage. The pids are our direct children, so this is safe and
    // (when a group DOES exist) merely redundant.
    for &pid in pids {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    for &pid in pids {
        let mut raw: libc::c_int = 0;
        unsafe { libc::waitpid(pid, &mut raw, 0); }
    }
}

/// Strips a trailing `&` and surrounding whitespace from the source line for
/// display in the job table.
fn display_command(source: &str) -> String {
    source
        .trim_end()
        .trim_end_matches('&')
        .trim_end()
        .to_string()
}

/// Best-effort `jobs`-listing label for a backgrounded and-or group produced by
/// a mid-list `&` separator (where the original source text isn't available at
/// the executor partition site). Uses the first command's static program name
/// when it is a plain literal; otherwise falls back to a generic label.
fn group_display_label(first: &Command) -> String {
    // Look through a single-stage pipeline wrapper (the parser wraps even a
    // lone simple command in a Pipeline at some sites).
    let simple = match first {
        Command::Simple(s) => Some(s),
        Command::Pipeline(p) if p.commands.len() == 1 => {
            if let Command::Simple(s) = &p.commands[0] { Some(s) } else { None }
        }
        _ => None,
    };
    if let Some(SimpleCommand::Exec(e)) = simple
        && let Some(name) = e.program_static_text()
    {
        return name;
    }
    "background job".to_string()
}

// ----- resolved command (post-expansion) ------------------------------------

/// A command after program/argument expansion. v156 task 7: redirections are no
/// longer pre-resolved into 0/1/2 slots here — every execution path now applies
/// the ordered `cmd.redirects` list directly (builtins via `RedirectScope`,
/// externals via `build_child_redir_plan`/`run_subprocess`). So `ResolvedCommand`
/// carries only the expanded program/args (+ declaration-arg shapes).
struct ResolvedCommand {
    program: String,
    args: Vec<String>,
    /// Populated only when `program` is a declaration command
    /// (`declare`, `typeset`, `local`, `readonly`, `export`). Carries
    /// the per-arg `DeclArg` shape so the builtin can route compound-RHS
    /// assignments through `apply_one_assignment` while still handling
    /// flags and scalar names. `args` is left empty in that case.
    decl_args: Option<Vec<crate::command::DeclArg>>,
}

enum ResolvedRedirect {
    Truncate(String),
    NoclobberTruncate(String),
    Append(String),
}

fn expand_single(
    word: &crate::lexer::Word,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<String, ()> {
    // Redirect targets do NOT undergo pathname expansion in v10 (per spec).
    // We call `expand` directly and require exactly one field, preserving the
    // ambiguous-redirect contract for word-splitting that produces 0 or >1.
    let fields = expand(word, shell);
    if fields.len() == 1 {
        Ok(fields.into_iter().next().unwrap().chars)
    } else {
        e!(err, "huck: ambiguous redirect");
        Err(())
    }
}

/// Expands `source` to a string and parses it as an fd number (e.g. "1" or "2").
/// Used for `Redirect::Dup { source }` to obtain the target fd pre-fork.
/// Errors with "bad fd: ..." if the expansion is not a valid non-negative integer.
fn resolve_fd_target(source: &crate::lexer::Word, shell: &mut Shell) -> Result<i32, io::Error> {
    let expanded = expand_assignment(source, shell);
    expanded
        .parse::<i32>()
        .map_err(|_| io::Error::other(format!("bad fd: {expanded}")))
}

/// Glob-expands one word honoring `shopt` flags. On a `failglob` no-match,
/// prints the bash-style "no match" error to stderr and returns `Err(())`,
/// signaling the caller to abort the command/loop with status 1.
fn glob_expand_word(
    word: &crate::lexer::Word,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<Vec<String>, ()> {
    // Borrow note: take the owned `Copy` opts before the mutable `expand`
    // borrow, so the immutable borrow ends first.
    let opts = shell.glob_opts();
    let fields = expand(word, shell);
    let exp = glob_expand_fields_opts(fields, opts);
    if !exp.failglob_unmatched.is_empty() {
        e!(err, "huck: no match: {}", exp.failglob_unmatched.join(" "));
        return Err(());
    }
    Ok(exp.words)
}

fn resolve(
    cmd: &ExecCommand,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<ResolvedCommand, i32> {
    let prog_fields = match glob_expand_word(&cmd.program, shell, err) {
        Ok(v) => v,
        Err(()) => return Err(1),
    };
    if let Some(status) = shell.pending_fatal_status {
        return Err(status);
    }
    if prog_fields.is_empty() {
        e!(err, "huck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();

    // Declaration commands (`declare` / `typeset` / `local` / `readonly` /
    // `export`) need a side-channel for assignment-shaped args so compound
    // RHS (`a=(x y z)`) doesn't crash through `expand()` — the lexer leaves
    // an `ArrayLiteral` WordPart that's unreachable! in expansion. For
    // those programs, split each arg Word: assignment-shaped → parsed
    // Assignment (via `try_split_assignment`); other → expanded String.
    let is_decl = builtins::is_declaration_command(&program);
    let mut decl_args: Option<Vec<crate::command::DeclArg>> = if is_decl {
        // Migrate any tail of `prog_fields` (post-split) into decl_args as
        // Plain, then drain `args` since the builtin won't read it.
        let mut v: Vec<crate::command::DeclArg> = Vec::with_capacity(args.len() + cmd.args.len());
        for s in args.drain(..) {
            v.push(crate::command::DeclArg::Plain(s));
        }
        Some(v)
    } else {
        None
    };
    for word in &cmd.args {
        if let Some(da) = decl_args.as_mut()
            && crate::command::is_assignment_word(word)
        {
            match crate::command::try_split_assignment(word.clone()) {
                Ok(a) => da.push(crate::command::DeclArg::Assign(a)),
                Err(_) => unreachable!(
                    "is_assignment_word confirmed shape but try_split_assignment refused"
                ),
            }
            continue;
        }
        let fields = match glob_expand_word(word, shell, err) {
            Ok(v) => v,
            Err(()) => return Err(1),
        };
        if let Some(status) = shell.pending_fatal_status {
            return Err(status);
        }
        if let Some(da) = decl_args.as_mut() {
            for s in fields {
                da.push(crate::command::DeclArg::Plain(s));
            }
        } else {
            args.extend(fields);
        }
    }
    // v156 task 7: redirections are NOT resolved here anymore. The builtin path
    // (run_builtin_with_redirects) and the single-external path (run_subprocess)
    // apply `cmd.redirects` in source order at execution time; the pipeline-stage
    // path reads `exec.slot_stdin/stdout/stderr()` directly off the AST. So
    // resolve() only expands the program + arguments.
    Ok(ResolvedCommand { program, args, decl_args })
}

// ----- redirect file handling -----------------------------------------------

/// Feed `bytes` (an expanded heredoc/herestring body) to a child's stdin WITHOUT
/// the parent ever blocking on a full pipe. Forks a writer process that owns the
/// pipe's write end, writes the whole body, then `_exit`s. The parent closes the
/// write end immediately, so no later in-process forked stage inherits it (a
/// writer *thread* would fail there — CLOEXEC only fires on exec, and InProcess
/// stages fork without exec). Returns the READ end (→ consumer stdin) and the
/// writer PID (reap it at the consumer's wait point; ECHILD is fine).
fn spawn_heredoc_writer(bytes: &[u8]) -> Result<(RawFd, libc::pid_t), io::Error> {
    let mut fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (r, w) = (fds[0], fds[1]);
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(r); libc::close(w); }
        return Err(e);
    }
    if pid == 0 {
        // CHILD: async-signal-safe only. Close read end; write the body; _exit.
        // v137: keep SIGPIPE ignored here (the process is otherwise SIG_DFL now)
        // so the writer retains its manual EPIPE handling and closes cleanly,
        // preserving v134 large-heredoc behavior exactly.
        unsafe { libc::close(r); libc::signal(libc::SIGPIPE, libc::SIG_IGN); }
        let mut off = 0usize;
        while off < bytes.len() {
            let n = unsafe {
                libc::write(w, bytes[off..].as_ptr() as *const libc::c_void, bytes.len() - off)
            };
            if n < 0 {
                // Read errno directly (async-signal-safe; no Rust io::Error
                // wrapper between fork and _exit). EINTR → retry; EPIPE (consumer
                // gone) or anything else → stop, body delivery is moot.
                // Symbol differs by platform: glibc/musl/Android expose
                // `__errno_location`, the BSDs and Apple expose `__error`.
                #[cfg(any(target_os = "linux", target_os = "android"))]
                let errno = unsafe { *libc::__errno_location() };
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                let errno = unsafe { *libc::__error() };
                if errno == libc::EINTR { continue; }
                break;
            }
            if n == 0 { break; }
            off += n as usize;
        }
        unsafe { libc::close(w); libc::_exit(0); }
    }
    unsafe { libc::close(w); }
    Ok((r, pid))
}

fn open_resolved(redirect: &ResolvedRedirect) -> io::Result<File> {
    match redirect {
        ResolvedRedirect::Truncate(path) => open_writable(path, false),
        ResolvedRedirect::NoclobberTruncate(path) => open_writable(path, true),
        ResolvedRedirect::Append(path) => OpenOptions::new()
            .create(true)
            .append(true)
            .open(path),
    }
}

/// Opens `path` for writing, truncating. When `guard_noclobber` is true
/// (the `noclobber` option is on and this is a plain `>`/`&>`, not `>|`),
/// refuse to overwrite an existing **regular** file — but exempt
/// non-regular files (e.g. /dev/null, FIFOs), matching bash's `set -C`.
fn open_writable(path: &str, guard_noclobber: bool) -> io::Result<File> {
    if !guard_noclobber {
        return OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path);
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            // TOCTOU: the stat-then-reopen on this exemption path has a benign
            // race (path could be swapped between metadata and open) — bash's
            // set -C has the same inherent race; no truncate is requested.
            match std::fs::metadata(path) {
                Ok(md) if !md.is_file() => OpenOptions::new().write(true).open(path),
                _ => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "cannot overwrite existing file",
                )),
            }
        }
        Err(e) => Err(e),
    }
}

fn resolved_path(redirect: &ResolvedRedirect) -> &str {
    match redirect {
        ResolvedRedirect::Truncate(p)
        | ResolvedRedirect::NoclobberTruncate(p)
        | ResolvedRedirect::Append(p) => p,
    }
}

// ----- single command -------------------------------------------------------

// $PIPESTATUS leaf-site rule (M-50, v83): ONLY `run_single`, `run_multi_stage`,
// and the foreground subshell arm write `$PIPESTATUS`. Compound runners
// (`run_if`/`run_while`/`run_for`/`run_case`/`run_select`/brace group) are
// deliberately PIPESTATUS-transparent — they never write it; their inner leaf
// commands do. This matches bash: after `if cond; then ...; fi`, `$PIPESTATUS`
// reflects the last inner pipeline (e.g. `cond`), not the `if` itself. Do NOT
// add a set_pipestatus call to a compound runner.
fn run_single(
    cmd: &SimpleCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let outcome = match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell, sink, err_sink),
        SimpleCommand::Assign(items, line) => {
            // Stamp $LINENO before expanding RHS so it reflects this assignment's line.
            if *line != 0 {
                shell.current_lineno = *line;
            }
            ExecOutcome::Continue(run_assignment_list(items, shell, sink, err_sink))
        }
    };
    // $PIPESTATUS reflects this leaf command's exit status. break/continue
    // bubble up as LoopBreak/LoopContinue (not Continue) but the builtin itself
    // succeeds, so bash sets PIPESTATUS=[0] — match that. (exit/return don't
    // reach here as themselves: `return` is normalized to Continue by
    // call_function; `exit` ends the shell.)
    match outcome {
        ExecOutcome::Continue(c) => shell.set_pipestatus(&[c]),
        ExecOutcome::LoopBreak(_, st) => shell.set_pipestatus(&[st]),
        ExecOutcome::LoopContinue(_) => shell.set_pipestatus(&[0]),
        ExecOutcome::Exit(_) | ExecOutcome::FunctionReturn(_) => {}
        ExecOutcome::Interrupted(_) => {}
    }
    outcome
}

/// True for the un-shadowable control builtins. Functions named the
/// same way are stored in `shell.functions` but unreachable — these
/// builtins always win.
fn is_control_builtin(name: &str) -> bool {
    matches!(name, "return" | "exit" | "break" | "continue")
}

/// Runs a function body in a new positional-arg frame. `args` is the
/// call's arguments *excluding* the function name — POSIX `$1` is the
/// first user arg, not the function name. Catches `FunctionReturn` and
/// converts to a normal `Continue(n)`; `Exit` propagates unchanged.
/// `shell.loop_depth` is zeroed for the function body and restored on
/// exit so that `break` inside a function called from a loop correctly
/// errors as "only meaningful in a loop" rather than escaping the
/// caller's loop (matches bash 5.2).
pub(crate) fn call_function(
    name: &str,
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // FUNCNEST enforcement + recursion backstop. Refuse a call that would exceed
    // the effective nesting limit BEFORE any frame/positional/local setup, so the
    // caller's statement simply sees rc 1 (matching bash). The backstop
    // (FUNCNEST_HARD_MAX) converts otherwise-unbounded recursion into a clean
    // error instead of a Rust stack-overflow abort.
    const FUNCNEST_HARD_MAX: usize = 2048;
    let limit = shell
        .funcnest_limit()
        .map_or(FUNCNEST_HARD_MAX, |n| n.min(FUNCNEST_HARD_MAX));
    let depth = shell
        .call_stack
        .iter()
        .filter(|f| matches!(f.kind, crate::shell_state::FrameKind::Function))
        .count();
    if depth >= limit {
        let prefix = shell.error_prefix(Some(name));
        {
            let mut err = err_writer(err_sink, sink);
            e!(&mut *err, "{prefix}maximum function nesting level exceeded ({limit})");
        }
        return ExecOutcome::Continue(1);
    }
    let saved = std::mem::take(&mut shell.positional_args);
    let saved_loop_depth = std::mem::replace(&mut shell.loop_depth, 0);
    // getopts' within-word cursor is per-call-context: save it and start the
    // function body fresh, so a function that runs getopts (e.g. with
    // `local OPTIND=1`) cannot corrupt a caller that is mid-cluster. Restored
    // below so the caller resumes its own scan, matching bash. (M-106)
    let saved_getopts_sp = std::mem::replace(&mut shell.getopts_sp, 0);
    let saved_getopts_optind_cache = std::mem::replace(&mut shell.getopts_optind_cache, 0);
    shell.positional_args = args;
    let frame = crate::shell_state::Frame {
        funcname: name.to_string(),
        source: shell.function_source.get(name).cloned().unwrap_or_else(|| "environment".to_string()),
        call_line: shell.current_lineno,
        kind: crate::shell_state::FrameKind::Function,
    };
    shell.call_stack.push(frame);
    // Keep the dynamic FUNCNAME array in lockstep with the call stack (v151).
    shell.sync_call_arrays();
    shell.local_scopes.push(std::collections::HashMap::new());

    let result = run_command(&body, shell, sink, err_sink);

    // RETURN trap fires with $? set to the function's status AND the
    // function's positional args still in scope. After the action runs,
    // restore the caller's frame.
    let status_for_trap = match &result {
        ExecOutcome::FunctionReturn(n) => *n,
        ExecOutcome::Continue(c) => *c,
        // Exit/LoopBreak/LoopContinue propagate up; keep $? as-is.
        _ => shell.last_status(),
    };
    shell.set_last_status(status_for_trap);
    crate::traps::fire_return_trap(shell);

    // Pop local scope and restore each snapshotted variable. Runs
    // AFTER the RETURN trap so the trap action still sees the
    // function's locals.
    if let Some(frame) = shell.local_scopes.pop() {
        for (var_name, snapshot) in frame {
            shell.restore_var(&var_name, snapshot);
        }
    }

    shell.call_stack.pop();
    shell.sync_call_arrays();
    shell.positional_args = saved;
    shell.loop_depth = saved_loop_depth;
    shell.getopts_sp = saved_getopts_sp;
    shell.getopts_optind_cache = saved_getopts_optind_cache;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}

/// Invokes a function by name with the given args. Looks up the body
/// from `shell.functions`. Returns `ExecOutcome::Continue(1)` if the
/// function doesn't exist. Stdout from the function goes to the real
/// stdout (matches bash's behavior where completion functions that
/// print produce visible output).
pub(crate) fn call_function_body(
    name: &str,
    args: Vec<String>,
    shell: &mut Shell,
) -> ExecOutcome {
    let body = match shell.functions.get(name) {
        Some(b) => b.clone(),
        None => return ExecOutcome::Continue(1),
    };
    let mut sink = StdoutSink::Terminal;
    let mut err_sink = StderrSink::Terminal;
    call_function(name, body, args, shell, &mut sink, &mut err_sink)
}

fn ps4(shell: &mut Shell) -> String {
    // bash expands $PS4 (prompt escapes + $VAR, via the PS1/PS2 expander), THEN
    // replicates the FIRST char of the EXPANDED value once per nesting level.
    let raw = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
    // Rendering a prompt must be transparent to $? (bash saves/restores it).
    let saved_status = shell.last_status();
    let saved_cmd_sub = shell.last_cmd_sub_status();
    // Suppress xtrace WHILE expanding PS4: bash does not trace commands run by a
    // PS4 command substitution, and tracing them here would recurse into ps4().
    let saved_xtrace = shell.shell_options.xtrace;
    shell.shell_options.xtrace = false;
    let expanded = crate::prompt::expand_prompt(&raw, shell);
    shell.shell_options.xtrace = saved_xtrace;
    shell.set_last_status(saved_status);
    shell.set_last_cmd_sub_status(saved_cmd_sub);
    let mut chars = expanded.chars();
    let Some(first) = chars.next() else { return String::new(); };
    let rest: String = chars.collect();
    let level = shell.xtrace_depth + 1;
    let mut out = String::with_capacity(level * first.len_utf8() + rest.len());
    for _ in 0..level { out.push(first); }
    out.push_str(&rest);
    out
}

/// Emit one xtrace line (the trailing newline is added) to fd 2 in a SINGLE
/// `write(2)`. Pipeline stages run in separate processes (a forked in-process
/// stage and the parent tracing an external stage) and share fd 2; a multi-write
/// `eprintln!` lets a sibling's bytes wedge between the prefix and the body
/// (`+ + echo a` / `cat`). A single write of the whole line keeps each trace
/// line intact (stages may still REORDER, which is best-effort per spec).
fn xtrace_emit(line: &str) {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let bytes = buf.as_bytes();
    // Ignore partial write / EINTR: trace lines are small (< PIPE_BUF = 4096, the
    // POSIX pipe-atomicity threshold) and best-effort; a short write at most
    // truncates one line. Single write keeps a line intact against concurrent
    // fd-2 writers (forked pipeline stages).
    unsafe {
        let _ = libc::write(2, bytes.as_ptr() as *const libc::c_void, bytes.len());
    }
}

/// Join (prefix ++ program ++ args), each xtrace-quoted, into one trace-line body.
fn xtrace_command_line(prefix: &[String], program: &str, args: &[String]) -> String {
    use crate::param_expansion::xtrace_quote;
    let mut parts: Vec<String> = prefix.iter().map(|w| xtrace_quote(w)).collect();
    parts.push(xtrace_quote(program));
    parts.extend(args.iter().map(|a| xtrace_quote(a)));
    parts.join(" ")
}

/// Emit one xtrace line for a compound-command header (gated on `set -x`).
/// Reuses the simple-command `ps4`/`xtrace_emit` path so depth/PS4/single-write
/// behavior is identical.
fn xtrace_compound(shell: &mut Shell, body: &str) {
    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}{body}"));
    }
}

/// If `w` is an array-literal RHS (`(a b c)`), return its elements for
/// best-effort xtrace rendering. `None` for ordinary scalar RHS words.
fn array_literal_elements(w: &crate::lexer::Word) -> Option<&[crate::lexer::ArrayLiteralElement]> {
    for part in &w.0 {
        if let crate::lexer::WordPart::ArrayLiteral(elems) = part {
            return Some(elems);
        }
    }
    None
}

/// Clean up (close fd + unlink FIFO + reap) every process substitution recorded
/// since `base`. Drains from the end so nested realizations are handled in reverse.
fn drain_procsubs(shell: &mut Shell, base: usize) {
    while shell.procsub_pending.len() > base {
        if let Some(ps) = shell.procsub_pending.pop() {
            crate::procsub::cleanup(ps);
        }
    }
}

/// Non-blocking variant of `drain_procsubs` for use after spawning a background
/// job. Closes the parent fd and unlinks the FIFO for each pending procsub, then
/// attempts a WNOHANG reap of the inner child. If the child has not yet exited
/// (uncommon: the inner producer is still running), we skip the blocking wait and
/// leave reaping to the shell's general SIGCHLD handler or exit cleanup. The
/// entries are always popped so they never leak into later commands' drains.
fn drain_procsubs_nonblocking(shell: &mut Shell, base: usize) {
    while shell.procsub_pending.len() > base {
        if let Some(ps) = shell.procsub_pending.pop() {
            // Close the parent end so the inner child sees EOF / SIGPIPE.
            if ps.parent_fd >= 0 {
                unsafe { libc::close(ps.parent_fd); }
            }
            // Remove the FIFO file if present.
            if let Some(ref p) = ps.fifo_path {
                let _ = std::fs::remove_file(p);
            }
            // Best-effort non-blocking reap. If the inner child is still running
            // (e.g. a long-running producer used with a background consumer),
            // skip the wait — it will be reaped by SIGCHLD handling or shell exit.
            let mut status = 0;
            unsafe { libc::waitpid(ps.pid, &mut status, libc::WNOHANG); }
        }
    }
}

/// Apply a list of assignments to the current shell (persistent), bash-style:
/// per-assignment readonly check, then `apply_one_assignment`, with the exit
/// status taken from the last RHS command substitution (or 0; a readonly/apply
/// error keeps status 1). Shared by `SimpleCommand::Assign` (a bare assignment)
/// and the assignment/redirect-only `ExecCommand` (empty program word, e.g.
/// `VAR=val 2>err`).
fn run_assignment_list(
    items: &[crate::command::Assignment],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> i32 {
    // Reset so only THESE assignments' RHS command substitutions count.
    shell.set_last_cmd_sub_status(None);
    let mut st = 0;
    for a in items {
        let name = a.target.name();
        // For namerefs, skip the early readonly check and let assign() check the
        // RESOLVED target's readonly — a readonly nameref lets you write through.
        if !shell.is_nameref(name) && shell.is_readonly(name) {
            let prefix = shell.error_prefix(None);
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "{prefix}{name}: readonly variable"); }
            shell.posix_fatal(127);
            st = 1;
            break;
        }
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
            shell.posix_fatal(127);
            st = 1;
            break;
        }
        if shell.shell_options.xtrace {
            let val = shell.lookup_var(name).unwrap_or_default();
            let p4 = ps4(shell);
            xtrace_emit(&format!("{p4}{name}={}",
                      crate::param_expansion::xtrace_quote(&val)));
        }
    }
    // bash: a bare assignment's status is the last command substitution in its
    // RHS (or 0 if none). A readonly/apply error keeps st=1.
    if st == 0 {
        st = shell.last_cmd_sub_status().unwrap_or(0);
    }
    st
}

fn run_exec_single(
    cmd: &ExecCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    run_exec_single_inner(cmd, shell, sink, err_sink, false)
}

/// `wrapped` is true when this invocation was reached via the pre-resolve
/// `command <decl>` / `builtin <decl>` rewrite (the inner program is therefore
/// command/builtin-wrapped even though `command_prefix`/`require_builtin` were
/// reset on recursion). It suppresses the bare-special-builtin posix-fatal
/// consume so `command export AA[4]=1` / `builtin export AA[4]=1` strip the
/// usage/assignment fatal, matching bash.
fn run_exec_single_inner(
    cmd: &ExecCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    wrapped: bool,
) -> ExecOutcome {
    // POSIX case #1: clear any usage-error signal a prior command may have left
    // un-consumed (e.g. a `command`-wrapped special builtin). Each special
    // builtin re-sets it at its own usage/assignment error site during dispatch.
    shell.builtin_usage_error = None;
    // Stamp $LINENO from the parse-time source line before any expansion.
    // The guard prevents synthesized line-0 commands (rewrites, builtins-via-command)
    // from clobbering a real current line.
    if cmd.line != 0 {
        shell.current_lineno = cmd.line;
    }
    // Snapshot the procsub stack. Any process substitutions realized during
    // argument expansion (resolve()) or redirect expansion are recorded in
    // shell.procsub_pending. We drain [procsub_base..] on every exit path so
    // the parent fd is closed and the inner child is reaped after the command
    // runs. The two recursive pre-resolve paths (command/builtin re-dispatch)
    // return before any expansion, so the drain is a no-op on those paths.
    let procsub_base = shell.procsub_pending.len();
    crate::traps::fire_debug_trap(shell);

    // Pre-resolve interception: `command [-p|--]… <decl-command> …` where the
    // inner program is a declaration builtin (`declare`/`export`/…). Detected
    // statically from RAW Words so a compound RHS (`arr=(x y z)`) never reaches
    // the outer `resolve` (which would expand the `command` operands and panic
    // on the parser-internal `ArrayLiteral` WordPart). When matched, rewrite to
    // an inner `ExecCommand` over the raw operand Words and recurse: the normal
    // flow then builds correct `decl_args` and dispatches `run_declaration_builtin`.
    // Declaration builtins are special builtins, so suppressing function lookup
    // is moot — recursion is equivalent and reuses all redirect/assign handling.
    if word_static_text(&cmd.program).as_deref() == Some("command")
        && let Some(k) = command_decl_operand_index(&cmd.args)
    {
        let inner = ExecCommand {
            inline_assignments: cmd.inline_assignments.clone(),
            program: cmd.args[k].clone(),
            args: cmd.args[k + 1..].to_vec(),
            redirects: cmd.redirects.clone(),
            line: 0,
        };
        // Pre-resolve recursion: no expansion yet, drain is a no-op but kept for uniformity.
        // `wrapped`: the inner decl builtin is `command`-wrapped → strip its usage fatal.
        drain_procsubs(shell, procsub_base);
        return run_exec_single_inner(&inner, shell, sink, err_sink, true);
    }

    // `builtin <decl-builtin> …` (v142): a declaration builtin reached via `builtin`
    // (e.g. `builtin local x=1`). Rewrite to the inner declaration command and
    // recurse so the normal flow builds correct decl_args + dispatches
    // run_declaration_builtin (declaration builtins can't be function-shadowed, so
    // the bypass is moot — same rationale as the `command` block above).
    if word_static_text(&cmd.program).as_deref() == Some("builtin")
        && cmd
            .args
            .first()
            .and_then(word_static_text)
            .map(|s| {
                builtins::is_declaration_command(&s) || s == "builtin" || s == "command"
            })
            .unwrap_or(false)
    {
        let inner = ExecCommand {
            inline_assignments: cmd.inline_assignments.clone(),
            program: cmd.args[0].clone(),
            args: cmd.args[1..].to_vec(),
            redirects: cmd.redirects.clone(),
            line: 0,
        };
        // Pre-resolve recursion: no expansion yet, drain is a no-op but kept for uniformity.
        // `wrapped`: the inner decl builtin is `builtin`-wrapped → strip its usage fatal.
        drain_procsubs(shell, procsub_base);
        return run_exec_single_inner(&inner, shell, sink, err_sink, true);
    }

    // Assignment/redirect-only command: no program word, just inline assignments
    // and/or redirects (e.g. `VAR=val 2>err`). bash applies the assignments to
    // the CURRENT shell (persistent, unlike the scoped prefix-assignments of a
    // real command) and performs the redirects for side effects, then exits 0
    // (1 on a readonly assignment) — it is NOT "command not found". The parser
    // emits an ExecCommand with an empty program Word for these; resolve() would
    // otherwise reject the empty program name. (A bare redirect with no
    // assignment, `>file`, is currently a parse error — tracked separately.)
    if cmd.program.0.is_empty() {
        // `$(< file)` special case: a command substitution whose body is JUST a
        // stdin read-only redirect reads the file directly as the substitution
        // output (bash). This only applies in a CAPTURE context — outside one
        // (`< file` at a terminal sink) it falls through to the normal
        // redirect-only behavior, which produces no output. Conditions: empty
        // program, no inline assignments, exactly one redirect that is a stdin
        // `File{ReadOnly}` (fd 0), and the current sink is a Capture buffer.
        if cmd.inline_assignments.is_empty()
            && cmd.redirects.len() == 1
            && matches!(sink, StdoutSink::Capture(_))
        {
            let redir = &cmd.redirects[0];
            if let RedirOp::File { mode: crate::command::FileMode::ReadOnly, target } = &redir.op
                && redir.target_fd() == Some(0)
            {
                let path = match expand_single(target, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => { drain_procsubs(shell, procsub_base); return ExecOutcome::Continue(1); }
                };
                let outcome = match std::fs::read(&path) {
                    Ok(bytes) => {
                        if let StdoutSink::Capture(buf) = sink {
                            buf.extend_from_slice(&bytes);
                            crate::callbacks_thread_local::with_callbacks(|cb| {
                                if let Some(cb) = cb {
                                    cb.push_stdout(&bytes);
                                }
                            });
                        }
                        ExecOutcome::Continue(0)
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                        ExecOutcome::Continue(1)
                    }
                };
                drain_procsubs(shell, procsub_base);
                return outcome;
            }
        }
        // bash processes the assignments with the ORIGINAL (un-redirected) fds:
        // RHS command substitutions and any readonly-variable error use the
        // inherited stderr, NOT the command's own `2>…`. It then performs the
        // redirections for their side effects only (e.g. `>f` truncation). So
        // apply the assignments first, then open the redirects with a no-op
        // body. A redirect that fails to open still leaves the assignment
        // applied but makes the status reflect the open failure (bash:
        // `x=1 </missing` → `x` is set, rc 1) — with_redirect_scope returns the
        // open failure before running the body, so its status wins.
        let st = run_assignment_list(&cmd.inline_assignments, shell, sink, err_sink);
        let outcome = with_redirect_scope(
            &cmd.redirects,
            shell,
            sink,
            err_sink,
            move |_shell, _sink, _err_sink| ExecOutcome::Continue(st),
        );
        drain_procsubs(shell, procsub_base);
        return outcome;
    }

    let mut resolved = match resolve(cmd, shell, &mut *err_writer(err_sink, sink)) {
        Ok(r) => r,
        Err(code) => { drain_procsubs(shell, procsub_base); return ExecOutcome::Continue(code); }
    };

    // `command CMD args` (bare form): run CMD suppressing shell-FUNCTION lookup
    // (builtins + $PATH still resolve). `-v`/`-V` introspection is left to the
    // `command` builtin (not intercepted here).
    let mut bypass_functions = false;
    let mut command_prefix: Vec<String> = Vec::new();
    while resolved.program == "command" {
        // Scan leading flags in resolved.args.
        let mut idx = 0;
        let mut introspect = false;
        loop {
            match resolved.args.get(idx).map(String::as_str) {
                Some("-v") | Some("-V") => {
                    introspect = true;
                    break;
                }
                Some("-p") => idx += 1, // accept; v99 uses current $PATH
                Some("--") => {
                    idx += 1;
                    break;
                }
                Some(s) if s.starts_with('-') && s.len() > 1 => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: command: {s}: invalid option"); }
                    drain_procsubs(shell, procsub_base);
                    return ExecOutcome::Continue(2);
                }
                _ => break, // first operand (or end)
            }
        }
        if introspect {
            break; // leave program=="command" -> dispatch runs builtin_command (-v/-V)
        }
        // Bare form: the operand at `idx` (if any) becomes the new program.
        match resolved.args.get(idx) {
            None => { drain_procsubs(shell, procsub_base); return ExecOutcome::Continue(0); } // `command` / `command -p` alone
            Some(_) => {
                command_prefix.push("command".to_string());
                command_prefix.extend(resolved.args[..idx].iter().cloned());
                let new_program = resolved.args[idx].clone();
                let new_args = resolved.args[idx + 1..].to_vec();
                resolved.program = new_program;
                resolved.args = new_args;
                // The inner CMD is not the outer `command`'s declaration form.
                // (A `command <decl-builtin> …` is intercepted pre-resolve above
                // and never reaches here, so this is always a non-declaration.)
                resolved.decl_args = None;
                bypass_functions = true;
                // loop: collapse `command command …`
            }
        }
    }

    // `builtin NAME args` (v142): run NAME as a shell BUILTIN ONLY, suppressing
    // function/alias lookup; error if NAME is not a builtin. Sibling to `command`.
    // (A declaration target is intercepted pre-resolve and never reaches here.)
    let mut require_builtin = false;
    while resolved.program == "builtin" {
        match resolved.args.first() {
            None => { drain_procsubs(shell, procsub_base); return ExecOutcome::Continue(0); } // `builtin` alone
            Some(_) => {
                let new_program = resolved.args[0].clone();
                let new_args = resolved.args[1..].to_vec();
                resolved.program = new_program;
                resolved.args = new_args;
                resolved.decl_args = None;
                bypass_functions = true;
                require_builtin = true;
                // loop: collapse `builtin builtin …`
            }
        }
    }
    // A declaration builtin can still surface here via a `command`-led nest
    // (`command builtin local`): the command strip loop reduced command→builtin,
    // this loop builtin→the declaration, and decl_args was discarded at resolve
    // time. Rather than panic in run_builtin, report it. (Maximally pathological;
    // a documented divergence — bash runs it. The `builtin`-led forms are handled
    // by the pre-resolve interception with decl_args rebuilt.)
    if require_builtin && builtins::is_declaration_command(&resolved.program) {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err,
            "huck: builtin: {}: declaration builtins must not be wrapped by `command builtin`",
            resolved.program
        ); }
        drain_procsubs(shell, procsub_base);
        return ExecOutcome::Continue(1);
    }
    if require_builtin && !builtins::is_builtin(&resolved.program) {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: builtin: {}: not a shell builtin", resolved.program); }
        drain_procsubs(shell, procsub_base);
        return ExecOutcome::Continue(1);
    }

    // `$_` value for THIS simple command: the last argument (post-expansion),
    // or the program name when there are no arguments. Computed here — after
    // the `command`/`builtin` prefixes are stripped and arguments are fully
    // expanded — so it covers builtins, functions, and externals alike (bash:
    // `echo a b c; echo "$_"` → `c`; `: foo` → `foo`; `ls` → `ls`). It is
    // applied to `shell.last_arg` AFTER dispatch (below), so that a function /
    // eval / source body's own nested commands don't leave a stale `$_`
    // (bash: `f(){ :; }; f a b; echo "$_"` → `b`, the outer call's last arg).
    let next_last_arg = resolved
        .args
        .last()
        .cloned()
        .unwrap_or_else(|| resolved.program.clone());

    // Apply inline assignments (e.g. FOO=bar in `FOO=bar cmd args`) before
    // dispatch. The snapshot is used to restore state for temporary-scope
    // targets (regular builtins and externals). Persistent-scope targets
    // (control builtins, special builtins per POSIX 2.14, and functions per
    // POSIX 2.9.1) skip the restore step.
    let snap = match apply_inline_assignments(&cmd.inline_assignments, shell, sink, err_sink) {
        Ok(s) => s,
        Err(s) => {
            restore_inline_assignments(s, shell);
            if builtins::is_special_builtin(&resolved.program) {
                shell.posix_fatal(127);
            }
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
    };

    // xtrace (`set -x`): print the expanded command to stderr, prefixed by
    // `$PS4` (default `+ `), BEFORE dispatch so a hanging command is traced
    // first. Use the already-expanded `resolved.program`/`resolved.args` (do
    // NOT re-expand). Each inline assignment on `VAR=v cmd` is traced on its
    // own preceding line (bash-style), read back via lookup_var. Then the
    // command line itself: program + args (or decl_args for declare/local/
    // etc.), every word xtrace-quoted, with the `command` prefix preserved.
    // An empty program (redirect-only command) emits no command line.
    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        // Inline-assignment prefix: each on its own preceding line (bash).
        for a in &cmd.inline_assignments {
            let name = a.target.name();
            let val = shell.lookup_var(name).unwrap_or_default();
            xtrace_emit(&format!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val)));
        }
        if !resolved.program.is_empty() {
            let body = if let Some(dargs) = &resolved.decl_args {
                let mut parts: Vec<String> = command_prefix
                    .iter()
                    .map(|w| crate::param_expansion::xtrace_quote(w))
                    .collect();
                parts.push(crate::param_expansion::xtrace_quote(&resolved.program));
                for da in dargs {
                    match da {
                        crate::command::DeclArg::Plain(s) => {
                            parts.push(crate::param_expansion::xtrace_quote(s))
                        }
                        crate::command::DeclArg::Assign(a) => {
                            let name = a.target.name();
                            if let Some(elems) = array_literal_elements(&a.value) {
                                // Array-literal RHS: best-effort `name=(e1 e2 ...)`
                                // (spec: arrays best-effort, must not crash;
                                // expand_assignment would panic on the literal).
                                // Route through the shared reconstructor so the
                                // render matches the eval/declare re-parse path.
                                let rendered =
                                    crate::expand::reconstruct_array_literal(elems, shell);
                                parts.push(format!("{name}={rendered}"));
                            } else {
                                let rhs = match crate::command::word_literal_text(&a.value) {
                                    Some(t) => t.to_string(),
                                    None => crate::expand::expand_assignment(&a.value, shell),
                                };
                                parts.push(format!(
                                    "{name}={}",
                                    crate::param_expansion::xtrace_quote(&rhs)
                                ));
                            }
                        }
                    }
                }
                parts.join(" ")
            } else {
                xtrace_command_line(&command_prefix, &resolved.program, &resolved.args)
            };
            xtrace_emit(&format!("{p4}{body}"));
        }
    }

    // Determine whether the assignments should persist after the command.
    // POSIX special builtins persist their prefix assignments. User functions
    // and regular builtins/externals do NOT — they snapshot/restore (temporary
    // scope), matching bash in both default and posix mode.
    // `export`/`readonly` absorb their named variable in BOTH modes (bash
    // assignment-builtin semantics). Every other special builtin persists its
    // prefix only under `set -o posix`; default mode restores it (POSIX 2.14 is
    // posix-mode-only in bash). `declare`/`typeset`/`local` are not special, so
    // they already restore correctly.
    let persistent = matches!(resolved.program.as_str(), "export" | "readonly")
        || (builtins::is_special_builtin(&resolved.program) && shell.shell_options.posix);

    // `exec`: a special builtin that does NOT fork. With a command operand it
    // replaces the shell process image; with only redirections it applies them
    // permanently to the shell's own fds. Neither fits the in-process-builtin or
    // fork-an-external models, so intercept here — after the inline assignments
    // are applied and exported (so they reach the replacement's environment) and
    // after xtrace, but before the dispatch machinery. Its inline assignments
    // persist (special builtin), so no restore on return.
    if resolved.program == "exec" {
        if crate::restricted::is_restricted(shell)
            && let Err(msg) = crate::restricted::check_exec()
        {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "{msg}"); }
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
        let outcome = run_exec_builtin(&resolved, cmd, shell, sink, err_sink);
        // POSIX case #1: `exec -z` (bad option) exits a non-interactive posix
        // shell. A `command exec`/`builtin exec` wrapper strips it (matches bash).
        // exec returns early here (it never reaches the post-dispatch consume), so
        // mirror that consume on this path, gated identically on a BARE invocation.
        if !wrapped && command_prefix.is_empty() && !require_builtin
            && let Some(st) = shell.builtin_usage_error.take()
        {
            shell.posix_fatal(st);
        }
        drain_procsubs(shell, procsub_base);
        return outcome;
    }

    // Section 3: track this command's snapshotted names on a shell-managed stack
    // so a nested posix special-builtin persist can delete them from enclosing
    // scopes. finalize_inline_scope (every exit path below) pops exactly this.
    shell.inline_scopes.push(snap.iter().map(|(n, _)| n.clone()).collect());

    if crate::restricted::is_restricted(shell)
        && let Err(msg) = crate::restricted::check_command_name(&resolved.program)
    {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "{msg}"); }
        finalize_inline_scope(snap, persistent, shell);
        drain_procsubs(shell, procsub_base);
        return ExecOutcome::Continue(1);
    }

    // 1. Control builtins always win — they cannot be shadowed by functions.
    // 2. User-defined function lookup.
    // 3. Regular builtin.
    // 4. PATH-exec.
    let outcome = if is_control_builtin(&resolved.program) {
        // v156 task 7: ALL redirects flow through one ordered RedirectScope (via
        // run_builtin_with_redirects), so `break 2>&1 >file` etc. are source-ordered
        // exactly like compounds/externals. Control builtins persist their inline
        // assignments (POSIX special-builtin), so no restore on the redirect-open
        // failure path inside the helper.
        run_builtin_with_redirects(&resolved, &cmd.redirects, shell, sink, err_sink)
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
        let name = resolved.program.clone();
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, sink, err_sink,
                move |shell, inner_sink, inner_err_sink| call_function(&name, body, args, shell, inner_sink, inner_err_sink))
        } else {
            call_function(&name, body, args, shell, sink, err_sink)
        }
    // eval/source must run their commands with the ENCLOSING sink (so `$(eval …)`
    // / `$(source …)` captures the output) and honour redirects — like a function
    // call. The generic builtin path below routes them through run_builtin, which
    // resets to a fresh Terminal sink (leaking the output and re-entering job
    // control inside a substitution → the nvm ls-remote hang), so intercept here.
    } else if resolved.program == "eval" {
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, sink, err_sink,
                move |shell, inner_sink, inner_err_sink| builtins::eval_in_sink(&args, shell, inner_sink, inner_err_sink))
        } else {
            builtins::eval_in_sink(&args, shell, sink, err_sink)
        }
    } else if resolved.program == "source" || resolved.program == "." {
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, sink, err_sink,
                move |shell, inner_sink, inner_err_sink| {
                    builtins::source_in_sink(&args, shell, inner_sink, inner_err_sink)
                })
        } else {
            builtins::source_in_sink(&args, shell, sink, err_sink)
        }
    } else if builtins::is_builtin(&resolved.program) {
        // v156 task 7: ALL redirects flow through one ordered RedirectScope (via
        // run_builtin_with_redirects) applied to the real fds in source order, so
        // `echo x 2>&1 >file`, `>file 3>&1`, `read < file`, `cmd <<<here` etc. all
        // honor source order and fd>2 uniformly — no more last-wins bridge. On a
        // redirect-open failure the helper rolls back and returns Continue(1); we
        // still owe the inline-assignment restore for temporary-scope targets.
        run_builtin_with_redirects(&resolved, &cmd.redirects, shell, sink, err_sink)
    } else {
        // v156 task 4: lower the FULL ordered redirect list (on the original
        // ExecCommand, not the bridged ResolvedCommand) into a child replay plan.
        // Files are opened (and heredoc writers forked) in the parent here; the
        // child replays the dup2/close ops in source order. This handles fd>2,
        // `<&` dup-in, `N>&-` close, and `<>` uniformly with fds 0/1/2.
        match build_child_redir_plan(&cmd.redirects, shell, sink, err_sink) {
            Ok(plan) => run_subprocess(&resolved, plan, shell, sink, err_sink),
            Err(code) => {
                finalize_inline_scope(snap, persistent, shell);
                drain_procsubs(shell, procsub_base);
                return ExecOutcome::Continue(code);
            }
        }
    };

    // POSIX case #1: a BARE special builtin that hit a usage / bad-option /
    // bad-assignment error exits a non-interactive posix shell (bash EX_USAGE).
    // `command`/`builtin` wrappers (command_prefix non-empty, require_builtin, or
    // reached via the pre-resolve decl-rewrite — `wrapped`) do NOT exit; they
    // leave the signal for the next command's top-of-fn clear. Runtime errors
    // never set the signal, so they fall through here untouched.
    if !wrapped && command_prefix.is_empty() && !require_builtin
        && builtins::is_special_builtin(&resolved.program)
        && let Some(st) = shell.builtin_usage_error.take()
    {
        shell.posix_fatal(st);
    }

    finalize_inline_scope(snap, persistent, shell);
    // Apply `$_` AFTER dispatch so a function/eval/source body's nested
    // commands don't leave a stale value (see `next_last_arg` above).
    shell.last_arg = next_last_arg;
    // A STOPPED command (Ctrl-Z) leaves its process substitutions alive — drain
    // them non-blocking (a blocking waitpid on a live procsub child whose
    // consumer is also stopped would deadlock the shell). Both variants pop the
    // pending entries, so the leak debug_assert below holds either way.
    if std::mem::take(&mut shell.fg_stopped) {
        drain_procsubs_nonblocking(shell, procsub_base);
    } else {
        drain_procsubs(shell, procsub_base);
    }
    debug_assert_eq!(
        shell.procsub_pending.len(), procsub_base,
        "process-substitution leak: a return path in run_exec_single skipped drain_procsubs"
    );
    outcome
}

/// Parsed `exec` options. `operand_start` is the index in `exec`'s args where
/// the command operand (and its args) begin; `args.len()` means none.
struct ExecFlags {
    /// `-c`: run the command with an empty environment.
    clear_env: bool,
    /// `-l`: prepend a `-` to argv[0] (login-shell convention).
    login: bool,
    /// `-a NAME`: use NAME as argv[0].
    argv0: Option<String>,
    operand_start: usize,
}

/// Parses leading `-c`/`-l`/`-a NAME` flags from `exec`'s arguments. Stops at
/// the first non-flag word or `--`. Returns an error message on a bad flag.
fn parse_exec_flags(args: &[String]) -> Result<ExecFlags, String> {
    let mut f = ExecFlags { clear_env: false, login: false, argv0: None, operand_start: args.len() };
    let mut i = 0;
    'outer: while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            f.operand_start = i + 1;
            return Ok(f);
        }
        let Some(body) = arg.strip_prefix('-') else {
            f.operand_start = i;
            return Ok(f);
        };
        if body.is_empty() {
            // A bare `-` is a command operand, not a flag.
            f.operand_start = i;
            return Ok(f);
        }
        let chars: Vec<char> = body.chars().collect();
        let mut j = 0;
        while j < chars.len() {
            match chars[j] {
                'c' => f.clear_env = true,
                'l' => f.login = true,
                'a' => {
                    // `-a NAME`: the rest of this word, else the next word.
                    let rest: String = chars[j + 1..].iter().collect();
                    if rest.is_empty() {
                        i += 1;
                        if i >= args.len() {
                            return Err("exec: -a: option requires an argument".to_string());
                        }
                        f.argv0 = Some(args[i].clone());
                    } else {
                        f.argv0 = Some(rest);
                    }
                    i += 1;
                    continue 'outer;
                }
                other => return Err(format!("exec: -{other}: invalid option")),
            }
            j += 1;
        }
        i += 1;
    }
    Ok(f)
}

/// Job-control signals huck `SIG_IGN`s at the shell level (see
/// `install_job_control_signals`). They must be reset to `SIG_DFL` for an
/// `exec` replacement, since `SIG_IGN` is inherited across `execve`.
const EXEC_RESET_SIGNALS: [libc::c_int; 3] =
    [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU];

/// Set the job-control signals to `SIG_DFL` for the about-to-exec replacement,
/// returning their prior handlers. Because `CommandExt::exec` does NOT fork,
/// any change here persists in the shell on the (rare) exec-failure path — so
/// the caller restores them via `restore_exec_signals` if exec returns.
unsafe fn reset_exec_signals_saving() -> [libc::sighandler_t; 3] {
    // Default each slot to SIG_DFL so that on the (practically impossible — these
    // were SIG_IGN'd at startup) SIG_ERR return, restore becomes a no-op instead
    // of passing SIG_ERR back to signal() (POSIX-undefined). Mirrors the SIG_ERR
    // guard in install_job_control_signals.
    let mut prev = [libc::SIG_DFL; 3];
    for (i, &sig) in EXEC_RESET_SIGNALS.iter().enumerate() {
        let old = unsafe { libc::signal(sig, libc::SIG_DFL) };
        if old != libc::SIG_ERR {
            prev[i] = old;
        }
    }
    prev
}

unsafe fn restore_exec_signals(prev: [libc::sighandler_t; 3]) {
    for (i, &sig) in EXEC_RESET_SIGNALS.iter().enumerate() {
        unsafe { libc::signal(sig, prev[i]); }
    }
}

/// Applies `cmd`'s stdin/stdout/stderr redirects to the SHELL's own fds and
/// keeps them (no restore) — `exec`'s permanent-redirect semantics. Mirrors the
/// open logic of `with_redirect_scope` but, on full success, discards the saved
/// originals instead of restoring them. A redirect that fails to open prints a
/// diagnostic, rolls back any already-applied redirects (the scope's Drop), and
/// returns `Err` (atomic: all-or-nothing).
fn apply_redirects_permanently(
    cmd: &ExecCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<(), ()> {
    let mut scope = RedirectScope::new();

    // Apply each redirection in source order via the ordered RedirectScope
    // applier. `apply` already dispatches `RedirFd::Var` to `apply_var`, so
    // a single call handles numeric fds, dup, close, heredoc, and `{var}`.
    // On failure, `scope` Drop rolls back any already-applied redirects
    // atomically (temporary semantics) and we return Err(()) to the caller.
    for redir in &cmd.redirects {
        if scope.apply(redir, shell, sink, err_sink).is_err() {
            // Reap any heredoc writers spawned by already-applied redirs before
            // the scope drops (Drop is writer-agnostic) — else a zombie until
            // shell exit. Mirrors with_redirect_scope's error path.
            scope.reap_heredoc_writers();
            return Err(()); // scope Drop restores partial → atomic rollback
        }
    }

    // SUCCESS → make the redirections permanent.
    //
    // For `{var}` redirections `apply_var` leaves the high fd OPEN and does
    // NOT register it in `scope.saved`, so it already persists beyond this
    // function — no special-casing needed.
    //
    // For heredoc writers: reap them NOW (before forget) so the writer
    // process doesn't become a zombie. The write end was installed onto the
    // target fd; the writer will finish and exit once its data is consumed.
    scope.reap_heredoc_writers();

    // Close each saved-original fd (or skip -1 = "was closed before us") so
    // Drop's restore loop has nothing to do. Draining first means Drop sees an
    // empty `saved` vec even if it somehow runs.
    for (_target, saved) in scope.saved.drain(..) {
        if saved >= 0 {
            unsafe { libc::close(saved); }
        }
        // saved == -1 means the target fd was previously free/closed — there is
        // no original fd to restore, so we leave the target fd open (permanent).
    }

    // Belt-and-suspenders: forget the scope so its Drop never runs the
    // (now-empty) restore loop, and the (already-drained) heredoc_writers
    // vec is not re-reaped.
    std::mem::forget(scope);
    Ok(())
}

/// The `exec` special builtin. With a command operand, replaces the shell
/// process image (no fork) via `CommandExt::exec`, which only returns on
/// failure. With only redirections, applies them permanently to the shell's
/// fds and returns 0. Inline assignments preceding `exec` were already applied
/// and exported by the caller, so they reach the replacement's environment.
fn run_exec_builtin(
    resolved: &ResolvedCommand,
    cmd: &ExecCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let flags = match parse_exec_flags(&resolved.args) {
        Ok(f) => f,
        Err(msg) => {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {msg}"); }
            // POSIX case #1: bad option is a usage error (the exec interception
            // consumes this for a bare invocation).
            shell.builtin_usage_error = Some(2);
            return ExecOutcome::Continue(2);
        }
    };

    // POSIX order: perform the redirections first. For `exec` they are PERMANENT
    // (no restore). bash does NOT exit on a failed `exec` redirect (unlike a
    // failed exec COMMAND below) — it prints the error and returns failure, so
    // the shell continues. Match that with Continue(1).
    let exit_or_continue = |code: i32, shell: &Shell| {
        if shell.is_interactive { ExecOutcome::Continue(code) } else { ExecOutcome::Exit(code) }
    };
    if has_any_redirect(cmd) {
        flush_stdout();
        if apply_redirects_permanently(cmd, shell, sink, err_sink).is_err() {
            return ExecOutcome::Continue(1);
        }
    }

    let operands = &resolved.args[flags.operand_start..];
    if operands.is_empty() {
        // Redirect-only (or bare) `exec`: redirections done, status 0.
        return ExecOutcome::Continue(0);
    }

    let name = &operands[0];
    let prog_path = match builtins::search_path_for(name, shell) {
        Some(p) => p,
        None => {
            let (msg, code) = if name.contains('/') && std::path::Path::new(name).exists() {
                ("cannot execute: Permission denied", 126)
            } else {
                ("not found", 127)
            };
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: exec: {name}: {msg}"); }
            return exit_or_continue(code, shell);
        }
    };

    flush_stdout();
    use std::os::unix::process::CommandExt;
    let mut process = ProcessCommand::new(&prog_path);
    process.args(&operands[1..]);
    // argv[0]: `-a NAME` overrides; `-l` prepends `-`; default is the name as given.
    let base0 = flags.argv0.clone().unwrap_or_else(|| name.clone());
    process.arg0(if flags.login { format!("-{base0}") } else { base0 });
    process.env_clear();
    if !flags.clear_env {
        process.envs(shell.exported_env());
        process.envs(shell.exported_function_env());
    }

    // Reset job-control signals to default for the replacement (huck SIG_IGNs
    // them and that is inherited across execve), saving them so a failed exec
    // restores the shell's own handlers (exec() does not fork).
    let saved = unsafe { reset_exec_signals_saving() };
    let err = process.exec();
    unsafe { restore_exec_signals(saved); }

    let code = match err.raw_os_error() {
        Some(libc::ENOENT) => 127,
        _ => 126, // EACCES / ENOEXEC / EISDIR / etc.: "cannot execute".
    };
    { let mut errw = err_writer(err_sink, sink); e!(&mut *errw, "huck: exec: {name}: {err}"); }
    exit_or_continue(code, shell)
}

/// A single pure-fd operation replayed in a child `pre_exec` (v156 task 4).
/// Both variants use only async-signal-safe libc calls (`dup2`/`close`). File
/// opens and heredoc-writer forks happen in the PARENT before the spawn; the
/// resulting source fd is inherited across fork and named here by number.
#[derive(Clone, Copy)]
enum ChildRedirOp {
    /// `dup2(source, target)` — wire `target` to whatever `source` points at.
    Dup { target: i32, source: i32 },
    /// `close(target)` — for `N>&-` / `N<&-`.
    Close { target: i32 },
}

/// Replay an ordered child-redirection op list onto the real fds. Async-signal-safe
/// (only dup2/close/fcntl — no allocation), so it is callable from a pre_exec hook.
/// `source == target` means a parent-opened file landed exactly on its target fd:
/// skip the dup2 and clear FD_CLOEXEC so it survives exec.
unsafe fn replay_redir_ops(ops: &[ChildRedirOp]) -> std::io::Result<()> {
    for op in ops {
        match *op {
            ChildRedirOp::Dup { target, source } => {
                if source == target {
                    // dup2(fd, fd) is a no-op that does NOT clear
                    // FD_CLOEXEC — but a parent-opened file landed
                    // exactly on `target` and was CLOEXEC'd, so it would
                    // vanish on exec. Clear CLOEXEC so it survives.
                    let flags = unsafe { libc::fcntl(target, libc::F_GETFD) };
                    if flags < 0 || unsafe { libc::fcntl(target, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                } else if unsafe { libc::dup2(source, target) } < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            ChildRedirOp::Close { target } => {
                // Lenient: closing an already-closed fd (EBADF) matches
                // bash; only a non-EBADF error aborts the spawn.
                if unsafe { libc::close(target) } < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.raw_os_error() != Some(libc::EBADF) {
                        return Err(e);
                    }
                }
            }
        }
    }
    Ok(())
}

/// The parent-side result of lowering `cmd.redirects` into an ordered replay
/// list for an external (forked) command. `ops` is applied IN ORDER in the
/// child's `pre_exec`. `held` keeps the parent-opened files / heredoc read-ends
/// alive (and FD_CLOEXEC'd, so they vanish on the child's exec while the dup2'd
/// targets survive) until after `spawn`. `heredoc_writers` are forked body
/// writers to reap after the child finishes.
struct ChildRedirPlan {
    ops: Vec<ChildRedirOp>,
    held: Vec<std::os::fd::OwnedFd>,
    heredoc_writers: Vec<libc::pid_t>,
}

/// Set FD_CLOEXEC on a raw fd so it does NOT leak into the exec'd program. The
/// child's `dup2(source, target)` clears CLOEXEC on `target`, so the redirect
/// survives exec while the parent-opened source fd is closed automatically.
fn set_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Relocate a freshly-opened parent fd to a high number (>= 10) with FD_CLOEXEC,
/// returning the new fd and closing the original. This keeps parent-opened
/// redirect *source* fds out of the low 0..9 range that explicit redirect
/// *targets* (e.g. `3>&1 2>&3`) operate on, so a source fd never collides with a
/// fd the child is still swapping (matches how bash relocates redirect fds).
/// On fcntl failure the original fd is returned unchanged (best-effort).
fn relocate_high_cloexec(fd: RawFd) -> RawFd {
    unsafe {
        let new = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
        if new < 0 {
            // Could not relocate (e.g. EMFILE) — fall back to the original fd
            // with CLOEXEC set; collisions are unlikely in the common case.
            set_cloexec(fd);
            return fd;
        }
        libc::close(fd);
        new
    }
}

/// Allocate a free fd >= 10 duped from `src_fd`. CLOEXEC is OFF so the fd is
/// inherited by an exec'd child (bash leaves {var}/exec fds open across exec).
fn alloc_high_fd(src_fd: RawFd) -> io::Result<RawFd> {
    let fd = unsafe { libc::fcntl(src_fd, libc::F_DUPFD, 10) }; // F_DUPFD (not _CLOEXEC)
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Lower `redirects` (in source order) into a `ChildRedirPlan` for an external
/// command: open files / spawn heredoc writers in the PARENT, resolve dup
/// sources, and emit an ordered `dup2`/`close` op list the child replays. On
/// any error a diagnostic is printed and `Err(1)` is returned (the held fds and
/// heredoc writers built so far are dropped/leaked-cleanly via `held`).
/// `RedirFd::Var` (`{var}>file`) allocates a free fd >= 10 (non-CLOEXEC), assigns
/// it to `$var`, and emits a source==target replay op so the child inherits it.
fn build_child_redir_plan(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<ChildRedirPlan, i32> {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    let mut plan = ChildRedirPlan { ops: Vec::new(), held: Vec::new(), heredoc_writers: Vec::new() };
    for redir in redirects {
        // `{var}` named-fd: allocate a free fd >= 10 in the PARENT (non-CLOEXEC so
        // the child inherits it), assign $var (persists), and emit a source==target
        // replay op so the child clears CLOEXEC on it (survives exec). The parent
        // keeps it in `held` and closes it after spawn (normal-command semantics;
        // exec's permanence is Task 6's caller decision).
        if let RedirFd::Var(name) = &redir.fd {
            // `{var}>&-` / `{var}<&-`: close the fd currently named by $var.
            if matches!(&redir.op, RedirOp::Close) {
                let cur = shell.lookup_var(name).unwrap_or_default();
                let fd: i32 = match cur.trim().parse::<i32>() {
                    Ok(n) if n >= 0 => n,
                    _ => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: ambiguous redirect"); }
                        return Err(1);
                    }
                };
                plan.ops.push(ChildRedirOp::Close { target: fd });
                continue;
            }
            // Resolve the source fd in the parent: an opened file, a dup source, or
            // a forked heredoc/here-string read end. `owns_src` => we close it after
            // duping to `high`; a Dup source belongs to the shell.
            let (src, owns_src): (RawFd, bool) = match &redir.op {
                RedirOp::File { mode, target: word } => {
                    let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                        Ok(p) => p,
                        Err(()) => return Err(1),
                    };
                    if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                        return Err(1);
                    }
                    let file: File = match mode {
                        FileMode::ReadOnly => match File::open(&path) {
                            Ok(f) => f,
                            Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                        },
                        FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                            let resolved = match mode {
                                FileMode::Append => ResolvedRedirect::Append(path),
                                FileMode::Clobber => ResolvedRedirect::Truncate(path),
                                _ if shell.shell_options.noclobber => {
                                    ResolvedRedirect::NoclobberTruncate(path)
                                }
                                _ => ResolvedRedirect::Truncate(path),
                            };
                            match open_resolved(&resolved) {
                                Ok(f) => f,
                                Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", resolved_path(&resolved)); } return Err(1); }
                            }
                        }
                        FileMode::ReadWrite => {
                            match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
                                Ok(f) => f,
                                Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                            }
                        }
                    };
                    (file.into_raw_fd(), true)
                }
                RedirOp::Dup { source, .. } => {
                    let src = match resolve_fd_target(source, shell) {
                        Ok(fd) => fd,
                        Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); } return Err(1); }
                    };
                    (src, false)
                }
                RedirOp::Heredoc { body, .. } => {
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((rfd, pid)) => { plan.heredoc_writers.push(pid); (rfd, true) }
                        Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); } return Err(1); }
                    }
                }
                RedirOp::HereString(w) => {
                    let mut bytes = expand_assignment(w, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((rfd, pid)) => { plan.heredoc_writers.push(pid); (rfd, true) }
                        Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); } return Err(1); }
                    }
                }
                RedirOp::Close => unreachable!("Close handled above"),
            };
            let high = match alloc_high_fd(src) {
                Ok(h) => h,
                Err(e) => {
                    if owns_src { unsafe { libc::close(src) }; }
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: {e}"); }
                    return Err(1);
                }
            };
            if owns_src {
                unsafe { libc::close(src) };
            }
            // NOTE: bash does NOT assign $var in the PARENT for an external
            // command — the redirect + var-assignment happen in the forked child,
            // so the parent's `$var` is untouched (verified: `fd=99; /bin/echo hi
            // {fd}>f` leaves $fd == 99). The child still inherits `high` OPEN
            // (non-CLOEXEC) so the exec'd program sees the descriptor; we do not
            // export $var to it (bash doesn't either). Hence: no `shell.set` here.
            let _ = name;
            // source == target makes the child's replay clear CLOEXEC on `high`
            // (it is non-CLOEXEC already, but the branch must NOT dup2 onto a
            // lower fd — `high` itself is the inherited descriptor).
            plan.ops.push(ChildRedirOp::Dup { target: high, source: high });
            plan.held.push(unsafe { OwnedFd::from_raw_fd(high) });
            continue;
        }
        let Some(target) = redir.target_fd() else {
            // RedirFd::Var is handled above; any other None is unexpected.
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ambiguous redirect"); }
            return Err(1);
        };
        let target = target as i32;
        match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => return Err(1),
                };
                if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                    return Err(1);
                }
                let file: File = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f,
                        Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f,
                            Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", resolved_path(&resolved)); } return Err(1); }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
                            Ok(f) => f,
                            Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                        }
                    }
                };
                // Relocate above fd 9 so the source never collides with a low
                // explicit-redirect target (e.g. `2>file 3>&2`).
                let raw = relocate_high_cloexec(file.into_raw_fd());
                let owned = unsafe { OwnedFd::from_raw_fd(raw) };
                plan.ops.push(ChildRedirOp::Dup { target, source: raw });
                plan.held.push(owned);
            }
            RedirOp::Dup { source, .. } => {
                // `>&w` / `<&w`: resolve the source fd in the parent. The fd
                // refers to a descriptor the child inherits (e.g. `&1`), so the
                // number is valid in the child after fork.
                let src = match resolve_fd_target(source, shell) {
                    Ok(fd) => fd,
                    Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); } return Err(1); }
                };
                plan.ops.push(ChildRedirOp::Dup { target, source: src });
            }
            RedirOp::Close => {
                plan.ops.push(ChildRedirOp::Close { target });
            }
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        plan.heredoc_writers.push(pid);
                        let rfd = relocate_high_cloexec(rfd);
                        let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                        plan.ops.push(ChildRedirOp::Dup { target, source: rfd });
                        plan.held.push(owned);
                    }
                    Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); } return Err(1); }
                }
            }
            RedirOp::HereString(w) => {
                let mut bytes = expand_assignment(w, shell).into_bytes();
                bytes.push(b'\n');
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        plan.heredoc_writers.push(pid);
                        let rfd = relocate_high_cloexec(rfd);
                        let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                        plan.ops.push(ChildRedirOp::Dup { target, source: rfd });
                        plan.held.push(owned);
                    }
                    Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); } return Err(1); }
                }
            }
        }
    }
    Ok(plan)
}

/// Additive (pipeline-stage) variant of `build_child_redir_plan`: lowers ONLY
/// the redirects the 0/1/2 slot fast-path does NOT consume (fd>2 File/Dup/Close,
/// `<&` dup-in, `N>&-` close, `<>` ReadWrite) into a replay list, opening files
/// in the PARENT. The fast-path-consumed 0/1/2 file/dup ops are applied by the
/// caller's existing pipe/stdio mechanism BEFORE this replay; the extra ops then
/// add the higher / cross-direction fds on top.
///
/// RESIDUAL LIMITATION (v156 task 7): this is the ONLY path still on the slot
/// fast-path. Source ordering between a 0/1/2 op and an extra op is NOT preserved
/// for pipeline stages (`cmd 2>&1 >file | …` is last-wins, unlike a single
/// command), and a heredoc/here-string on an fd>2 of a *pipeline-stage* external
/// is dropped (it would need a reaped writer threaded through the pipeline wait
/// point). The single-command builtin/external paths do NOT use this — they apply
/// the full ordered `cmd.redirects` (so L-08 + fd>2 heredoc are fixed there).
fn build_child_extra_ops(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<(Vec<ChildRedirOp>, Vec<std::os::fd::OwnedFd>), i32> {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    let extra = stage_extra_redirects(redirects);
    let mut ops: Vec<ChildRedirOp> = Vec::new();
    let mut held: Vec<OwnedFd> = Vec::new();
    for redir in &extra {
        let Some(target) = redir.target_fd() else {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: ambiguous redirect"); }
            return Err(1);
        };
        let target = target as i32;
        match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => return Err(1),
                };
                if check_restricted_redirect(mode, &path, shell, sink, err_sink).is_err() {
                    return Err(1);
                }
                let file: File = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f,
                        Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => ResolvedRedirect::NoclobberTruncate(path),
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f,
                            Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", resolved_path(&resolved)); } return Err(1); }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path) {
                            Ok(f) => f,
                            Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); } return Err(1); }
                        }
                    }
                };
                let raw = relocate_high_cloexec(file.into_raw_fd());
                ops.push(ChildRedirOp::Dup { target, source: raw });
                held.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
            RedirOp::Dup { source, .. } => {
                let src = match resolve_fd_target(source, shell) {
                    Ok(fd) => fd,
                    Err(e) => { { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); } return Err(1); }
                };
                ops.push(ChildRedirOp::Dup { target, source: src });
            }
            RedirOp::Close => ops.push(ChildRedirOp::Close { target }),
            RedirOp::Heredoc { .. } | RedirOp::HereString(_) => {
                // Documented additive gap (see fn doc): an fd>2 heredoc on a
                // pipeline-stage external. The bridge dropped this already.
            }
        }
    }
    Ok((ops, held))
}

/// v156 task 4: the single (non-pipeline) external command path. `plan` is the
/// ordered `dup2`/`close` replay list lowered from `cmd.redirects` by
/// `build_child_redir_plan` in the PARENT (files already opened, heredoc writers
/// already forked). The child replays `plan.ops` IN SOURCE ORDER in a `pre_exec`
/// after the signal-reset hook, so e.g. `3>&1 1>&2 2>&3` performs the fd swap
/// correctly. fds 0/1/2 and fd>2 are all handled uniformly by the replay; this
/// function no longer wires `.stdin/.stdout/.stderr` from opened files (only the
/// capture pipe, which any explicit fd-1 redirect in the replay then overrides).
fn run_subprocess(
    cmd: &ResolvedCommand,
    mut plan: ChildRedirPlan,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);

    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());
    process.envs(shell.exported_function_env());

    // Reset job-control signals to SIG_DFL in every child (foreground and
    // background). The shell SIG_IGNs these, and SIG_IGN is inherited across
    // exec — without this, Ctrl-Z would never stop foreground children like
    // vim/less, and background readers could never receive SIGTTIN.
    use std::os::unix::process::CommandExt;
    unsafe { process.pre_exec(reset_job_control_signals_in_child); }

    // Replay the ordered redirect ops in the child (AFTER the signal-reset
    // pre_exec). All ops are pure dup2/close (async-signal-safe). On any failure
    // return Err so spawn() fails cleanly.
    let ops = std::mem::take(&mut plan.ops);
    unsafe {
        process.pre_exec(move || replay_redir_ops(&ops));
    }

    if interactive {
        process.process_group(0);
    }

    // The heredoc/herestring writers were forked by build_child_redir_plan; their
    // read-ends are in `plan.held` (FD_CLOEXEC, inherited across fork, replayed by
    // the ops above) and their pids are reaped after the child's status.
    let want_capture = matches!(sink, StdoutSink::Capture(_));
    let want_capture_err = matches!(err_sink, StderrSink::Capture(_));
    let merged_err = matches!(err_sink, StderrSink::Merged);
    if want_capture {
        // Pipe fd 1 back to the parent for capture. An explicit `>file` (or other
        // fd-1 redirect) in `plan.ops` overrides this in the child replay, so the
        // capture pipe correctly sees EOF when the command redirects its stdout.
        process.stdout(Stdio::piped());
    }
    if want_capture_err {
        // Pipe fd 2 back to the parent for capture. Mirrors the want_capture path
        // for stderr. An explicit `2>file` in `plan.ops` overrides this in the
        // child replay (same as stdout).
        process.stderr(Stdio::piped());
    } else if merged_err {
        // StderrSink::Merged: route fd 2 onto fd 1 in the child via a pre_exec
        // dup2 after std's stdio bridge sets up fd 1. This is the kernel-level
        // analog of `2>&1`: a single ordered byte stream into whatever fd 1 is
        // (the inherited terminal OR the capture pipe set by `process.stdout`
        // above). Bash-compatible byte ordering falls out naturally.
        unsafe {
            process.pre_exec(|| {
                if libc::dup2(1, 2) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Flush pending parent stdout before spawning so the child's output is
    // ordered after buffered parent bytes (M-118 sibling: ordering).
    flush_stdout();
    let spawn_result = process.spawn();
    // The child has now forked and inherited the parent-opened redirect fds in
    // `plan.held` (FD_CLOEXEC). Drop the parent's copies so they don't leak and
    // so heredoc/here-string read-ends fully close once the child exits.
    drop(plan.held);
    let heredoc_writers = plan.heredoc_writers;
    match spawn_result {
        Ok(mut child) => {
            // The heredoc/herestring body (if any) is written by the forked
            // writer process whose read end is the child's stdin; nothing to
            // write here. The writer pids are reaped after the child's status.

            let pid = child.id() as i32;

            // Register pid in the live-children registry so the timeout timer
            // thread can SIGTERM it if the deadline fires. The guard pops on
            // every exit path (including early returns / panics).
            let live_pids = shell.live_external_children.clone();
            live_pids.lock().unwrap().push(pid as libc::pid_t);
            let _pid_guard = LiveChildGuard { pids: &live_pids, pid: pid as libc::pid_t };

            let outcome = if interactive {
                // Race-close: also setpgid in the parent so the child's pgrp
                // is guaranteed to exist before we call tcsetpgrp.
                unsafe {
                    if libc::setpgid(pid, pid) != 0 {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        debug_assert!(
                            errno == libc::ESRCH || errno == libc::EACCES,
                            "setpgid({pid}, {pid}) failed with unexpected errno {errno}"
                        );
                    }
                }
                give_terminal_to(pid);

                // wait_with_untraced already waitpid'd the child, so each arm
                // mem::forget's the Child to keep its Drop from re-reaping
                // (already-reaped pid would give -ECHILD).
                match wait_with_untraced(pid) {
                    Ok((raw_status, true)) => {
                        // Child was stopped (e.g. Ctrl-Z / SIGTSTP).
                        let sig = libc::WSTOPSIG(raw_status);
                        let job_id = shell.jobs.add(pid, vec![pid], cmd.program.clone());
                        for job in shell.jobs.jobs_mut() {
                            if job.id == job_id {
                                job.state = crate::jobs::JobState::Stopped(sig);
                                job.notified = true;
                                break;
                            }
                        }
                        let line = shell.jobs.iter()
                            .find(|j| j.id == job_id)
                            .map(|j| crate::jobs::notification_line(j, '+'))
                            .unwrap_or_default();
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "\n{line}"); }
                        // The command was STOPPED: its process substitutions are
                        // still alive (tied to the stopped job), so the drain in
                        // run_exec_single's epilogue must be non-blocking.
                        shell.fg_stopped = true;
                        std::mem::forget(child);
                        give_terminal_to(shell.shell_pgid);
                        ExecOutcome::Continue(128 + sig)
                    }
                    Ok((raw_status, false)) => {
                        // Child exited or was killed by a signal.
                        let code = if libc::WIFEXITED(raw_status) {
                            libc::WEXITSTATUS(raw_status)
                        } else if libc::WIFSIGNALED(raw_status) {
                            let sig = libc::WTERMSIG(raw_status);
                            if sig == libc::SIGINT {
                                shell
                                    .sigint_flag
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            128 + sig
                        } else {
                            1
                        };
                        std::mem::forget(child);
                        give_terminal_to(shell.shell_pgid);
                        ExecOutcome::Continue(code)
                    }
                    Err(()) => {
                        std::mem::forget(child);
                        give_terminal_to(shell.shell_pgid);
                        ExecOutcome::Continue(1)
                    }
                }
            } else {
                // Non-interactive (capture-or-pure) path: poll piped fds AND wait
                // on the child concurrently via stream_loop::external_capture_loop.
                // This runs on the embedder's thread (no drainer thread), so
                // streaming callbacks fire in real time as bytes arrive.
                use std::os::fd::IntoRawFd;
                let pipe_out: RawFd = child
                    .stdout
                    .take()
                    .map(|cs| cs.into_raw_fd())
                    .unwrap_or(-1);
                let pipe_err: RawFd = child
                    .stderr
                    .take()
                    .map(|cs| cs.into_raw_fd())
                    .unwrap_or(-1);
                // Stash a separate capture buffer for stderr so we don't have to
                // juggle two &mut Vec<u8> borrows of the *sink/err_sink enums
                // simultaneously inside CaptureSinks.
                let mut stderr_capture: Vec<u8> = Vec::new();
                let stdout_sink_buf: Option<&mut Vec<u8>> = match sink {
                    StdoutSink::Capture(buf) => Some(*buf),
                    StdoutSink::Terminal => None,
                };
                let stderr_sink_buf: Option<&mut Vec<u8>> = if want_capture_err {
                    Some(&mut stderr_capture)
                } else {
                    None
                };
                let sinks = crate::stream_loop::CaptureSinks {
                    stdout: stdout_sink_buf,
                    stderr: stderr_sink_buf,
                };
                let loop_result = crate::stream_loop::external_capture_loop(
                    pid as libc::pid_t,
                    pipe_out,
                    pipe_err,
                    sinks,
                    || None,
                );
                // Close pipe fds we took ownership of.
                if pipe_out >= 0 {
                    unsafe { libc::close(pipe_out); }
                }
                if pipe_err >= 0 {
                    unsafe { libc::close(pipe_err); }
                }
                // We have already reaped the child via waitpid(WNOHANG) inside
                // the loop. Tell `Child` not to reap (or wait on) again — the
                // pid has been collected and would otherwise be -ECHILD.
                std::mem::forget(child);
                match loop_result {
                    Ok(raw_status) => {
                        // Fold the separately-captured stderr bytes into err_sink
                        // now that the stdout &mut borrow is released.
                        if want_capture_err
                            && let StderrSink::Capture(buf) = err_sink
                        {
                            buf.extend_from_slice(&stderr_capture);
                        }
                        let code = if libc::WIFEXITED(raw_status) {
                            libc::WEXITSTATUS(raw_status)
                        } else if libc::WIFSIGNALED(raw_status) {
                            let sig = libc::WTERMSIG(raw_status);
                            if sig == libc::SIGINT {
                                shell
                                    .sigint_flag
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            128 + sig
                        } else {
                            1
                        };
                        ExecOutcome::Continue(code)
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", cmd.program); }
                        ExecOutcome::Continue(1)
                    }
                }
            };
            // Reap the forked heredoc/herestring writers now the consumer has
            // exited (M-120). They are internal helpers — not jobs, not $!.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe { libc::waitpid(wpid, &mut st, 0); }
            }
            outcome
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Spawn failed: reap any heredoc writers so they don't linger.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe { libc::waitpid(wpid, &mut st, 0); }
            }
            // bash format: `<src>: line N: <name>: command not found` (the name
            // precedes the phrase; error_prefix supplies the prologue + mode split).
            {
                let prefix = shell.error_prefix(None);
                let mut err = err_writer(err_sink, sink);
                e!(&mut *err, "{prefix}{}: command not found", cmd.program);
            }
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe { libc::waitpid(wpid, &mut st, 0); }
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: {e}", cmd.program); }
            ExecOutcome::Continue(1)
        }
    }
}

// ----- multi-stage pipeline -------------------------------------------------

/// Per-stage outcome after spawning: a forked child pid to be waited for.
enum PipelineStage {
    Forked(i32),
}

/// Start a coprocess: fork the body with stdin/stdout wired to two pipes, hold
/// the shell-side ends (relocated high + close-on-exec) as NAME[0] (read) /
/// NAME[1] (write), publish NAME_PID, $!, and a job. Returns 0 on a successful
/// spawn (the coproc runs asynchronously), 1 on pipe/fork failure. coproc
/// ALWAYS forks (no builtin/function fast-path).
fn run_coproc(
    name: &str,
    body: &Command,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // v157 single-active: warn (but proceed) if a coproc is already live.
    if let Some(existing) = shell.coprocs.first() {
        { let mut err = err_writer(err_sink, sink); e!(&mut *err,
            "huck: warning: execute_coproc: coproc [{}:{}] still exists",
            existing.pid, existing.name
        ); }
    }
    // make_pipe() returns (read_end, write_end).
    // pipe_in: shell writes in_w -> coproc reads in_r (its stdin).
    // pipe_out: coproc writes out_w (its stdout) -> shell reads out_r.
    let (in_r, in_w) = match make_pipe() {
        Ok(p) => p,
        Err(e) => {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: coproc: {e}"); }
            return ExecOutcome::Continue(1);
        }
    };
    let (out_r, out_w) = match make_pipe() {
        Ok(p) => p,
        Err(e) => {
            unsafe {
                libc::close(in_r);
                libc::close(in_w);
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: coproc: {e}"); }
            return ExecOutcome::Continue(1);
        }
    };
    flush_stdout();
    // Fork the body: child stdin = in_r, stdout = out_w, stderr inherited; its
    // own process group (pgid_target 0); the child must NOT keep the shell ends.
    let pid = match fork_and_run_in_subshell(
        body,
        shell,
        in_r,
        out_w,
        libc::STDERR_FILENO,
        0,
        &[in_w, out_r],
        None,
        None,
    ) {
        Ok(pid) => pid,
        Err(e) => {
            unsafe {
                libc::close(in_r);
                libc::close(in_w);
                libc::close(out_r);
                libc::close(out_w);
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: coproc: {e}"); }
            return ExecOutcome::Continue(1);
        }
    };
    // Parent: close the child ends; relocate the shell ends high + cloexec.
    unsafe {
        libc::close(in_r);
        libc::close(out_w);
    }
    let read_fd = match alloc_high_fd(out_r) {
        Ok(hi) => {
            unsafe {
                libc::close(out_r);
            }
            set_cloexec(hi);
            hi
        }
        Err(e) => {
            unsafe {
                libc::close(out_r);
                libc::close(in_w);
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: coproc: {e}"); }
            return ExecOutcome::Continue(1);
        }
    };
    let write_fd = match alloc_high_fd(in_w) {
        Ok(hi) => {
            unsafe {
                libc::close(in_w);
            }
            set_cloexec(hi);
            hi
        }
        Err(e) => {
            unsafe {
                libc::close(read_fd);
                libc::close(in_w);
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: coproc: {e}"); }
            return ExecOutcome::Continue(1);
        }
    };
    // Publish: NAME=(read write), NAME_PID, $!, a job, and the record.
    let mut elems: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    elems.insert(0, read_fd.to_string());
    elems.insert(1, write_fd.to_string());
    let _ = shell.replace_indexed(name, elems);
    shell.set(format!("{name}_PID").as_str(), pid.to_string());
    shell.last_bg_pid = Some(pid);
    shell.jobs.add(pid, vec![pid], format!("coproc {name}"));
    shell.coprocs.push(crate::shell_state::Coproc {
        name: name.to_string(),
        pid,
        read_fd,
        write_fd,
    });
    ExecOutcome::Continue(0)
}

/// Opens a `libc::pipe()` and returns `(read_end, write_end)` as raw fds.
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

/// Create an inter-stage pipe for a downstream pipeline reader, where
/// the upstream stage's stdout is going elsewhere (an explicit file
/// redirect). Closes the write-end immediately so the downstream reader
/// sees EOF instead of inheriting parent stdin or blocking on an
/// orphaned write-end. Returns the read-end fd to thread into
/// `prev_pipe_read`. On `make_pipe` failure, the caller propagates the
/// error.
#[allow(dead_code)]
fn make_orphan_pipe_for_eof_reader() -> io::Result<RawFd> {
    let (r, w) = make_pipe()?;
    unsafe { libc::close(w); }
    Ok(r)
}

/// Rewrites `run_multi_stage` around raw `libc::pipe` fds.
///
/// Each stage is classified via `classify_stage`:
/// - External (single unquoted literal program, not a builtin or function):
///   spawned via `spawn_external_with_fds`.
/// - InProcess (builtins, functions, compounds, dynamic program words, Assign):
///   forked via `fork_and_run_in_subshell` (which runs the body via `run_command`
///   in the child, then `_exit`s).
///
/// Pipe-fd lifecycle:
///   - For each inter-stage boundary, a `libc::pipe()` pair is created.
///   - The write-end is given to stage N as stdout_fd (consumed by spawn/fork).
///   - The read-end is kept in `prev_pipe_read` and passed to stage N+1 as stdin_fd.
///   - After spawning a stage, the parent closes any fd it passed to the child
///     (so that EOF propagates correctly when the child exits).
///   - Heredoc body bytes are written from the parent to a pipe write-end
///     immediately after the child is spawned.
///
/// v23 inline-assignment scoping: apply before spawning, restore after.
/// v24 heredoc plumbing: create a pipe for the heredoc body; parent writes it.
/// I-04 fix: every stage now runs in its own forked subshell → side effects
///   (cd, variable mutation) are confined to the child.
fn run_multi_stage(
    commands: &[Command],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // Job-control process-grouping is only correct in the top-level shell. Inside
    // a forked subshell it places the inner pipeline in a background process group
    // with default SIGTTOU/SIGTTIN handling, deadlocking the subshell's wait on a
    // controlling terminal (M-104). A subshell's inner pipeline uses the
    // non-job-control path (stages stay in the subshell's pgrp), matching bash.
    let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);
    let n = commands.len();

    // Fd for the capture-sink case: last stage's stdout is piped back to parent.
    let mut capture_read_fd: Option<RawFd> = None;

    // Pid tracking.
    let mut first_pid: Option<i32> = None;
    let mut stage_pids: Vec<i32> = Vec::with_capacity(n);

    // Live-children registry: every stage pid is published while the pipeline
    // is running so the timeout timer thread can SIGTERM all stages in one pass
    // when the deadline fires. Cleared in one bulk pass after wait_pipeline_raw.
    let live_pids_arc = shell.live_external_children.clone();

    // All forked stages (pid + optional inline exit status for Done stages).
    let mut pipeline_stages: Vec<PipelineStage> = Vec::with_capacity(n);

    // Read-end of the pipe from the previous stage (None for stage 0).
    let mut prev_pipe_read: Option<RawFd> = None;

    // All raw fds the parent currently holds (for the child's
    // parent_fds_to_close list so it doesn't inherit stale pipe ends).
    let mut parent_held: Vec<RawFd> = Vec::new();

    // For StderrSink::Capture: one shared pipe whose write-end is wired into every
    // stage's fd 2 (bash `pipe1 | pipe2 | … 2>err` semantics — each stage's stderr
    // lands in the same buffer). For Merged the per-stage stderr_fd is aliased
    // to the active stdout fd (the inter-stage pipe write-end for non-last stages,
    // and the final-stage stdout target for the last stage), giving kernel-level
    // 2>&1 ordering. For Terminal stderr inherits as before.
    let (capture_err_pipe_write_fd, mut capture_err_read_fd): (Option<RawFd>, Option<RawFd>) =
        if matches!(err_sink, StderrSink::Capture(_)) {
            match make_pipe() {
                Ok((r, w)) => {
                    parent_held.push(r);
                    parent_held.push(w);
                    (Some(w), Some(r))
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            (None, None)
        };

    // PIDs of forked heredoc/herestring writer processes (M-120); reaped at the
    // pipeline wait point. They are internal helpers — never jobs, never $!,
    // never part of $PIPESTATUS.
    let mut heredoc_writers: Vec<libc::pid_t> = Vec::new();

    // Snapshot the procsub stack before any stage word expansion. Process
    // substitutions realized during argument/redirect expansion (in expand_single
    // or expand calls inside the spawn loop) push onto shell.procsub_pending.
    // We must drain [procsub_base..] on every exit path — the parent fd must
    // stay open until all stages have run (so it is drained AFTER
    // wait_pipeline_raw, not per-stage), but on error paths we drain early so
    // the inner child gets EOF/SIGPIPE and exits.
    let procsub_base = shell.procsub_pending.len();

    for (i, stage_cmd) in commands.iter().enumerate() {
        let is_last = i == n - 1;

        // ---- Assign-only stages: no-op, just pass stdin through as empty ----
        if let Command::Simple(SimpleCommand::Assign(items, aline)) = stage_cmd {
            // In a pipeline, assignment-only stages are a no-op: they don't
            // produce output and are run as InProcess. But since assignments
            // in a subshell don't affect the parent, they're truly inert.
            // Close any incoming pipe read-end (it goes nowhere).
            if let Some(r) = prev_pipe_read.take() {
                let pos = parent_held.iter().position(|&fd| fd == r);
                if let Some(p) = pos { parent_held.remove(p); }
                unsafe { libc::close(r); }
            }
            // Run the assignments via fork so they're isolated.
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone(), *aline));
            let pgid_target = if interactive { first_pid.unwrap_or(0) } else { NO_PGROUP };
            let stdin_fd = libc::STDIN_FILENO;
            let stdout_fd = if !is_last {
                // Create a pipe; next stage reads from it (will be empty).
                match make_pipe() {
                    Ok((r, w)) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                        parent_held.push(w);
                        w
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        // Clean up all held fds.
                        drain_procsubs(shell, procsub_base);
                        for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            } else {
                match sink {
                    StdoutSink::Capture(_) => {
                        match make_pipe() {
                            Ok((r, w)) => {
                                capture_read_fd = Some(r);
                                parent_held.push(r);
                                w
                            }
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    StdoutSink::Terminal => libc::STDOUT_FILENO,
                }
            };
            let fds_to_close: Vec<RawFd> = parent_held.iter().copied()
                .filter(|&fd| fd != stdout_fd)
                .collect();
            match fork_and_run_in_subshell(&assign_cmd, shell, stdin_fd, stdout_fd, libc::STDERR_FILENO, pgid_target, &fds_to_close, None, None) {
                Ok(pid) => {
                    // Close the stdout fd we gave to the child.
                    if stdout_fd > 2 {
                        let pos = parent_held.iter().position(|&fd| fd == stdout_fd);
                        if let Some(p) = pos { parent_held.remove(p); }
                        unsafe { libc::close(stdout_fd); }
                    }
                    if interactive && first_pid.is_none() {
                        first_pid = Some(pid);
                    }
                    stage_pids.push(pid);
                    live_pids_arc.lock().unwrap().push(pid as libc::pid_t);
                    pipeline_stages.push(PipelineStage::Forked(pid));
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: fork: {e}"); }
                    drain_procsubs(shell, procsub_base);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
            continue;
        }

        // ---- Apply inline assignments (v23 per-stage scoping) ---------------
        let inline_assignments: &[crate::command::Assignment] =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                &exec.inline_assignments
            } else {
                &[]
            };
        let snap = match apply_inline_assignments(inline_assignments, shell, sink, err_sink) {
            Ok(s) => s,
            Err(s) => {
                restore_inline_assignments(s, shell);
                drain_procsubs(shell, procsub_base);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                return ExecOutcome::Continue(1);
            }
        };

        // ---- Build stdin fd --------------------------------------------------
        // Priority: explicit redirect on ExecCommand > prev_pipe_read > STDIN_FILENO.
        // For InProcess compound stages, there are no explicit redirects at the
        // stage level; the child handles them internally via run_command.

        // A heredoc/herestring stdin is fed by a forked writer process (M-120):
        // the body is expanded NOW (while inline assignments are applied so that
        // $var references see the stage's own inline assignments — v24 deferred-
        // heredoc contract), handed to `spawn_heredoc_writer`, and the read end
        // becomes this stage's stdin. The parent never holds the pipe write end.
        let stdin_fd: RawFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.slot_stdin() {
                Some(Redirect::Read(word)) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin, so that pipe would otherwise be leaked
                    // into parent_held, keeping the write-end alive and
                    // deadlocking the previous stage's writer.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    };
                    use std::os::unix::io::IntoRawFd;
                    match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                Some(Redirect::Heredoc { body, .. }) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin via heredoc, so that pipe would otherwise
                    // be leaked into parent_held, deadlocking the previous
                    // stage's writer once the pipe buffer fills.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    // Expand the body NOW while inline assignments are still applied,
                    // then hand it to a forked writer process (M-120): a body larger
                    // than the pipe buffer must not block the parent before the
                    // consumer drains. The parent never holds the write end.
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, pid)) => {
                            heredoc_writers.push(pid);
                            r
                        }
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                Some(Redirect::HereString(body)) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin via here-string.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    // Expand NOW (inline assignments still applied) + trailing newline,
                    // then feed via a forked writer (M-120).
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, pid)) => {
                            heredoc_writers.push(pid);
                            r
                        }
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: heredoc: {e}"); }
                            restore_inline_assignments(snap, shell);
                            drain_procsubs(shell, procsub_base);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                _ => {
                    // No explicit stdin redirect: use prev_pipe_read or STDIN_FILENO.
                    prev_pipe_read.take().unwrap_or(libc::STDIN_FILENO)
                }
            }
        } else {
            // Compound command: no explicit stdin at stage level.
            prev_pipe_read.take().unwrap_or(libc::STDIN_FILENO)
        };

        // stdin_fd is now consumed by this stage; remove from parent_held if it was there.
        {
            let pos = parent_held.iter().position(|&fd| fd == stdin_fd);
            if let Some(p) = pos { parent_held.remove(p); }
        }

        // ---- Determine stdout redirect (from ExecCommand if Simple) ----------
        let explicit_stdout_fd: Option<RawFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stdout() {
                    Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        let guard = shell.shell_options.noclobber
                            && !matches!(r, Redirect::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Determine stderr redirect (from ExecCommand if Simple) ----------
        let explicit_stderr_fd: Option<RawFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stderr() {
                    Some(r @ (Redirect::Truncate(w) | Redirect::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        let guard = shell.shell_options.noclobber
                            && !matches!(r, Redirect::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {path}: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Build stdout fd -------------------------------------------------
        // Priority: explicit redirect > inter-stage pipe > Capture sink pipe > STDOUT_FILENO.
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            // Upstream stdout goes to the file. For a non-final stage we
            // STILL need to create an inter-stage pipe so the downstream
            // stage reads EOF instead of inheriting parent stdin (M-125).
            if !is_last {
                match make_orphan_pipe_for_eof_reader() {
                    Ok(r) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        restore_inline_assignments(snap, shell);
                        if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                        if let Some(efd) = explicit_stderr_fd { unsafe { libc::close(efd); } }
                        unsafe { libc::close(fd); } // close the open file fd we won't use
                        drain_procsubs(shell, procsub_base);
                        for pfd in parent_held.drain(..) { unsafe { libc::close(pfd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            fd
        } else if !is_last {
            // Create the inter-stage pipe.
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    // w is given to the child; track it so other children can close it.
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            match sink {
                StdoutSink::Capture(_) => {
                    match make_pipe() {
                        Ok((r, w)) => {
                            capture_read_fd = Some(r);
                            parent_held.push(r);
                            w
                        }
                        Err(e) => {
                            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                            restore_inline_assignments(snap, shell);
                            if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                            if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                            drain_procsubs(shell, procsub_base);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                StdoutSink::Terminal => libc::STDOUT_FILENO,
            }
        };

        // ---- Build stderr fd -------------------------------------------------
        // Priority: explicit redirect (`2>file` / `2>&n`) > sink-derived.
        //   StderrSink::Terminal → STDERR_FILENO (inherit).
        //   StderrSink::Merged   → stdout_fd (kernel-level 2>&1; for non-last
        //                          stages this aliases the inter-stage pipe,
        //                          matching bash `pipe1 2>&1 | pipe2 2>&1`).
        //   StderrSink::Capture  → dup of the shared capture_err write-end. We
        //                          dup PER STAGE because spawn_external_with_fds
        //                          consumes its stderr_fd via OwnedFd (closes
        //                          parent's copy) and fork_and_run_in_subshell
        //                          paths close it explicitly after spawn — both
        //                          would otherwise destroy the shared write-end
        //                          after the first stage.
        let stderr_fd = if let Some(fd) = explicit_stderr_fd {
            fd
        } else {
            match err_sink {
                StderrSink::Terminal => libc::STDERR_FILENO,
                StderrSink::Merged => stdout_fd,
                StderrSink::Capture(_) => {
                    let shared = capture_err_pipe_write_fd
                        .expect("capture_err_pipe_write_fd set when err_sink is Capture");
                    let fd = unsafe { libc::dup(shared) };
                    if fd < 0 {
                        let e = io::Error::last_os_error();
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: dup: {e}"); }
                        restore_inline_assignments(snap, shell);
                        if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                        if stdout_fd > 2 {
                            parent_held.retain(|&x| x != stdout_fd);
                            unsafe { libc::close(stdout_fd); }
                        }
                        if let Some(r) = capture_read_fd {
                            parent_held.retain(|&x| x != r);
                            unsafe { libc::close(r); }
                        }
                        drain_procsubs(shell, procsub_base);
                        for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                        return ExecOutcome::Continue(1);
                    }
                    fd
                }
            }
        };

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if interactive { first_pid.unwrap_or(0) } else { NO_PGROUP };

        // parent_fds_to_close: all fds the parent currently holds that the
        // child must close (so it doesn't hold downstream pipe write-ends open,
        // which would prevent EOF propagation). We exclude the fds being passed
        // to this stage as stdio (those are the child's to keep). The heredoc
        // pipe's write end lives in the forked writer process, not here, so
        // there is nothing extra to add.
        let fds_to_close_in_child: Vec<RawFd> = parent_held.iter().copied()
            .filter(|&fd| fd != stdout_fd && fd != stdin_fd && fd != stderr_fd)
            .collect();

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.slot_stdout() {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(r) = capture_read_fd {
                                    parent_held.retain(|&fd| fd != r);
                                    unsafe { libc::close(r); }
                                }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.slot_stderr() {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(r) = capture_read_fd {
                                    parent_held.retain(|&fd| fd != r);
                                    unsafe { libc::close(r); }
                                }
                                drain_procsubs(shell, procsub_base);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                (sdt, sedt)
            } else {
                (None, None)
            };

        // Track whether we went External (in which case spawn_external_with_fds
        // consumes stdin/stdout/stderr via OwnedFd and the parent must NOT close
        // them) or InProcess (in which case the parent must close them since
        // fork_and_run_in_subshell only dup2's in the child).
        let went_external;
        let spawn_result = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => {
                went_external = true;
                // spawn_external_with_fds takes ownership of stdin_fd/stdout_fd/stderr_fd
                // via OwnedFd::from_raw_fd. The parent's copies are closed inside the
                // function. Do NOT close them in the parent after this call.
                spawn_external_with_fds(
                    simple,
                    shell,
                    sink,
                    err_sink,
                    stdin_fd,
                    stdout_fd,
                    stderr_fd,
                    pgid_target,
                    &fds_to_close_in_child,
                )
            }
            StageKind::InProcess(cmd) => {
                went_external = false;
                fork_and_run_in_subshell(
                    cmd,
                    shell,
                    stdin_fd,
                    stdout_fd,
                    stderr_fd,
                    pgid_target,
                    &fds_to_close_in_child,
                    stdout_dup_target,
                    stderr_dup_target,
                )
            }
        };

        // ---- Restore inline assignments (v23 scoping) -----------------------
        restore_inline_assignments(snap, shell);

        let pid = match spawn_result {
            Ok(p) => p,
            Err(e) => {
                { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {e}"); }
                // For InProcess (fork failed), close the fds we were going to
                // pass. For External, they were already consumed by OwnedFd.
                if !went_external {
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if stdout_fd > 2 { unsafe { libc::close(stdout_fd); } }
                    if stderr_fd > 2 { unsafe { libc::close(stderr_fd); } }
                }
                // Remove consumed fds from parent_held.
                for fd in [stdout_fd, stdin_fd, stderr_fd] {
                    if fd > 2 {
                        let pos = parent_held.iter().position(|&x| x == fd);
                        if let Some(p) = pos { parent_held.remove(p); }
                    }
                }
                // Exclude capture_read_fd from the drain: it will be closed
                // explicitly below, avoiding a double-close.
                if let Some(r) = capture_read_fd {
                    parent_held.retain(|&fd| fd != r);
                }
                drain_procsubs(shell, procsub_base);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                if let Some(r) = capture_read_fd { unsafe { libc::close(r); } }
                return ExecOutcome::Continue(1);
            }
        };

        // ---- Close fds the child consumed in the parent ---------------------
        // For InProcess: the child dup2'd stdin/stdout/stderr but the parent's
        // copies still exist. Close them here.
        // For External: spawn_external_with_fds consumed them via OwnedFd;
        // they are already closed. Do NOT close again.
        if !went_external {
            if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
            // stdout_fd will be closed below (shared with External path).
            if stderr_fd > 2 { unsafe { libc::close(stderr_fd); } }
        }
        // stdout_fd (write-end of the inter-stage pipe or explicit redirect):
        // - For External: already closed by OwnedFd inside spawn_external_with_fds.
        // - For InProcess: closed above if stderr, here for stdout.
        // But we still need to remove it from parent_held in both cases so
        // subsequent stages don't include it in their fds_to_close_in_child.
        if stdout_fd > 2 {
            let pos = parent_held.iter().position(|&fd| fd == stdout_fd);
            if let Some(p) = pos { parent_held.remove(p); }
            // Only close if InProcess (External already closed it).
            if !went_external {
                unsafe { libc::close(stdout_fd); }
            }
        }

        // (A heredoc/herestring body, if any, is written by the forked writer
        // process spawned above; the parent holds no write end here.)

        // ---- Track pid -------------------------------------------------------
        if interactive && first_pid.is_none() {
            first_pid = Some(pid);
        }
        stage_pids.push(pid);
        live_pids_arc.lock().unwrap().push(pid as libc::pid_t);
        pipeline_stages.push(PipelineStage::Forked(pid));
    }

    // Close any remaining parent-held fds that weren't consumed
    // (e.g., if the last stage had an explicit stdout redirect, prev_pipe_read
    // might still hold a stale value from a stage with a broken pipe — but that
    // shouldn't happen in a well-formed pipeline).
    // Keep the capture_read_fd (stdout) AND capture_err_read_fd open here — both
    // are drained below BEFORE the wait (M-119), so they must survive this
    // bulk-close. The capture_err_pipe_write_fd IS closed here (intentional —
    // every stage has its own dup, so closing the parent's copy is what makes
    // the read-end see EOF after the last stage exits).
    for fd in parent_held.iter().copied() {
        if Some(fd) != capture_read_fd && Some(fd) != capture_err_read_fd {
            unsafe { libc::close(fd); }
        }
    }
    parent_held.retain(|&fd| Some(fd) == capture_read_fd || Some(fd) == capture_err_read_fd);

    // Drain stdout AND stderr capture pipes via a single poll loop on the
    // embedder's thread. Real-time streaming callbacks fire as bytes
    // arrive; no drainer threads. The loop exits when BOTH pipes hit EOF,
    // which happens once every writer (the last stage for stdout; all stages
    // for the shared stderr pipe) has exited. Capture sink => interactive ==
    // false, so the terminal-handoff/stopped blocks below are no-ops.
    let pipe_out: RawFd = capture_read_fd.take().unwrap_or(-1);
    let pipe_err: RawFd = capture_err_read_fd.take().unwrap_or(-1);
    let mut stderr_capture: Vec<u8> = Vec::new();
    if pipe_out >= 0 || pipe_err >= 0 {
        let stdout_sink_buf: Option<&mut Vec<u8>> = match &mut *sink {
            StdoutSink::Capture(buf) => Some(*buf),
            StdoutSink::Terminal => None,
        };
        let stderr_sink_buf: Option<&mut Vec<u8>> = if matches!(err_sink, StderrSink::Capture(_)) {
            Some(&mut stderr_capture)
        } else {
            None
        };
        let sinks = crate::stream_loop::CaptureSinks {
            stdout: stdout_sink_buf,
            stderr: stderr_sink_buf,
        };
        let _ = crate::stream_loop::pipeline_drain_loop(pipe_out, pipe_err, sinks);
    }
    if pipe_out >= 0 {
        unsafe { libc::close(pipe_out); }
    }
    if pipe_err >= 0 {
        unsafe { libc::close(pipe_err); }
    }
    // Fold captured stderr bytes into err_sink now that &mut sink is released.
    if let StderrSink::Capture(buf) = err_sink {
        buf.extend_from_slice(&stderr_capture);
    }

    // Give the terminal to the pipeline's process group if interactive.
    if interactive && let Some(pgid) = first_pid {
        give_terminal_to(pgid);
    }

    // ---- Wait for all stages ------------------------------------------------
    let last_status = wait_pipeline_raw(&pipeline_stages, &stage_pids, first_pid, shell, sink, err_sink, interactive);

    // Clear this pipeline's stage pids from the live-children registry in one
    // pass. They were published per-stage above so the timeout timer thread
    // could SIGTERM every stage on a deadline fire.
    {
        let mut guard = live_pids_arc.lock().unwrap();
        guard.retain(|p| !stage_pids.iter().any(|s| (*s as libc::pid_t) == *p));
    }

    // Reap any forked heredoc/herestring writer processes (M-120). They are not
    // pipeline stages, so they are excluded from $PIPESTATUS and the wait above;
    // ECHILD or any error is fine — they are transient helpers.
    for wpid in heredoc_writers {
        let mut st = 0;
        unsafe { libc::waitpid(wpid, &mut st, 0); }
    }

    if interactive {
        give_terminal_to(shell.shell_pgid);
        if let PipelineWaitResult::Stopped(sig) = &last_status {
            let sig = *sig;
            // Intentionally do NOT set $PIPESTATUS here: bash does not set it
            // for a stopped (Ctrl-Z) pipeline. (The capture fd, if any, was
            // already drained and taken above — capture sink => non-interactive,
            // so this stopped path never carries a live capture fd.)
            // NON-blocking drain: the pipeline's process substitutions (e.g.
            // `find | tee >(awk)`) are still alive, tied to the now-stopped job.
            // A blocking `waitpid` on a procsub child whose consumer (`tee`) is
            // also stopped would deadlock the shell (no prompt back after Ctrl-Z).
            drain_procsubs_nonblocking(shell, procsub_base);
            return ExecOutcome::Continue(128 + sig);
        }
    }

    let status = match last_status {
        PipelineWaitResult::AllExited(stages) => {
            shell.set_pipestatus(&stages);
            if shell.shell_options.pipefail {
                // rightmost non-zero stage, else 0
                stages.iter().rev().find(|&&s| s != 0).copied().unwrap_or(0)
            } else {
                stages.last().copied().unwrap_or(0)
            }
        }
        PipelineWaitResult::Stopped(sig) => 128 + sig,
    };
    // Drain any process substitutions realized during stage word expansion.
    // We drain here (after wait_pipeline_raw + heredoc_writers reap), not
    // per-stage, because the parent_fd must stay open until all stages that
    // reference /dev/fd/N have run.
    drain_procsubs(shell, procsub_base);
    ExecOutcome::Continue(status)
}

enum PipelineWaitResult {
    AllExited(Vec<i32>),
    Stopped(i32),
}

/// Waits for all forked stages in a pipeline. Works with raw pids (both
/// External and InProcess stages produce a pid).
///
/// For interactive pipelines: uses `waitpid(-pgid, WUNTRACED)` on the whole
/// process group (B-09 pattern). On stop, registers the job and returns
/// `Stopped`. The terminal is NOT reclaimed here — the caller does it.
///
/// For non-interactive pipelines: waits on each pid sequentially.
#[allow(clippy::too_many_arguments)]
fn wait_pipeline_raw(
    stages: &[PipelineStage],
    stage_pids: &[i32],
    first_pid: Option<i32>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    interactive: bool,
) -> PipelineWaitResult {
    // All stages are Forked; initialize status slots to None.
    let mut stage_status: Vec<Option<i32>> = stages.iter().map(|_| None).collect();

    let pid_per_stage: Vec<Option<i32>> = stages
        .iter()
        .map(|s| match s {
            PipelineStage::Forked(pid) => Some(*pid),
        })
        .collect();

    if interactive {
        if let Some(pgid) = first_pid {
            let mut remaining: std::collections::HashSet<i32> =
                pid_per_stage.iter().filter_map(|p| *p).collect();

            while !remaining.is_empty() {
                let mut raw: libc::c_int = 0;
                let r = loop {
                    let r = unsafe { libc::waitpid(-pgid, &mut raw, libc::WUNTRACED) };
                    if r < 0 {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        if errno == libc::EINTR {
                            continue;
                        }
                    }
                    break r;
                };
                if r < 0 {
                    // ECHILD — fill unfilled with 1.
                    for slot in stage_status.iter_mut() {
                        if slot.is_none() { *slot = Some(1); }
                    }
                    break;
                }
                if libc::WIFSTOPPED(raw) {
                    let sig = libc::WSTOPSIG(raw);
                    let display = format!("(pipeline pid {pgid})");
                    let job_id = shell.jobs.add(pgid, stage_pids.to_vec(), display);
                    for job in shell.jobs.jobs_mut() {
                        if job.id == job_id {
                            job.state = crate::jobs::JobState::Stopped(sig);
                            job.notified = true;
                            break;
                        }
                    }
                    let line = shell
                        .jobs
                        .iter()
                        .find(|j| j.id == job_id)
                        .map(|j| crate::jobs::notification_line(j, '+'))
                        .unwrap_or_default();
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "\n{line}"); }
                    return PipelineWaitResult::Stopped(sig);
                }
                if libc::WIFEXITED(raw) || libc::WIFSIGNALED(raw) {
                    let s = if libc::WIFEXITED(raw) {
                        libc::WEXITSTATUS(raw)
                    } else {
                        let sig = libc::WTERMSIG(raw);
                        if sig == libc::SIGINT {
                            shell
                                .sigint_flag
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        128 + sig
                    };
                    if let Some(idx) = pid_per_stage.iter().position(|p| *p == Some(r)) {
                        stage_status[idx] = Some(s);
                    }
                    remaining.remove(&r);
                }
            }
        }
    } else {
        // Non-interactive: wait on each pid in order.
        for (stage, slot) in stages.iter().zip(stage_status.iter_mut()) {
            let PipelineStage::Forked(pid) = stage;
            let mut raw: libc::c_int = 0;
            loop {
                let r = unsafe { libc::waitpid(*pid, &mut raw, 0) };
                if r < 0 {
                    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    if errno == libc::EINTR { continue; }
                    *slot = Some(1);
                    break;
                }
                if libc::WIFEXITED(raw) {
                    *slot = Some(libc::WEXITSTATUS(raw));
                } else if libc::WIFSIGNALED(raw) {
                    let sig = libc::WTERMSIG(raw);
                    if sig == libc::SIGINT {
                        shell
                            .sigint_flag
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    *slot = Some(128 + sig);
                }
                break;
            }
        }
    }

    crate::traps::dispatch_pending_traps(shell);
    let stages: Vec<i32> = stage_status.iter().map(|s| s.unwrap_or(1)).collect();
    PipelineWaitResult::AllExited(stages)
}

// ----- inline-assignment apply/restore helpers ------------------------------

/// Snapshot entry for one applied inline assignment: name + the full
/// prior `Variable` (or `None` if the var was unset before apply).
/// Cloning the entire `Variable` (rather than just its scalar value
/// and export flag) is what lets v71 array-valued inline prefixes
/// round-trip correctly through restore.
type AssignmentSnapshot = Vec<(String, Option<crate::shell_state::Variable>)>;

/// Expands and applies `assignments` left-to-right, exporting each, and
/// returns a snapshot the caller can pass to `restore_inline_assignments`
/// (for temporary-scope targets) or discard (for persistent-scope targets).
fn apply_inline_assignments(
    assignments: &[crate::command::Assignment],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<AssignmentSnapshot, AssignmentSnapshot> {
    let mut snap: AssignmentSnapshot = Vec::with_capacity(assignments.len());
    for a in assignments {
        let name = a.target.name();

        // Determine which variable's state must be saved/restored.  For a
        // nameref `r`, the write goes to the RESOLVED TARGET (e.g. `x`), so
        // we must snapshot/restore that target — not the nameref binding itself.
        // Non-nameref: snap_name == name (byte-identical to the old path).
        let snap_name: String = if shell.is_nameref(name) {
            match shell.resolve_nameref(name) {
                // Resolved to a plain variable — snapshot that variable.
                crate::shell_state::ResolvedName::Name(n) => n,
                // Resolved to arr[subscript] — snapshot the whole array (covers the element).
                crate::shell_state::ResolvedName::Element { name: arr, .. } => arr,
                // Unbound nameref: the assignment will BIND the nameref itself → snapshot name.
                // Cycle: write is dropped; snapshot name as a safe fallback.
                crate::shell_state::ResolvedName::Unbound(_)
                | crate::shell_state::ResolvedName::Cycle => name.to_string(),
            }
        } else {
            name.to_string()
        };

        let prior = shell.snapshot_var(&snap_name);
        // For namerefs, skip the early readonly check; assign() checks the resolved target.
        if !shell.is_nameref(name) && shell.is_readonly(name) {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: readonly variable"); }
            return Err(snap);
        }
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
            return Err(snap);
        }
        // Bash semantics: inline-prefix assignments are exported for the
        // duration of the command. Only scalar bare-name assignments to
        // a scalar (or new) variable carry the export flag — array
        // variables aren't placed in the child environment, but the
        // export flag still flips during the temporary scope so that a
        // later `a=val cmd` (scalar reassignment of the same name) sees
        // the expected state. Match the pre-v71 behavior by toggling the
        // export bit only when the value is a bare scalar.
        // For a nameref the export should mark the resolved target, so use snap_name.
        if matches!(&a.target, crate::command::AssignTarget::Bare(_))
            && !is_array_value_word(&a.value)
        {
            shell.export(&snap_name);
        }
        snap.push((snap_name, prior));
    }
    Ok(snap)
}

/// Restores each snapshot entry in reverse order, so repeated names
/// unwind LIFO and end up at their pre-prefix value.
fn restore_inline_assignments(snap: AssignmentSnapshot, shell: &mut Shell) {
    for (name, prior) in snap.into_iter().rev() {
        shell.restore_var(&name, prior);
    }
}

/// Pops the top `inline_scopes` entry pushed by `run_exec_single` and finalizes
/// this command's inline assignments. NON-persistent: restore LIFO, but skip any
/// name a nested posix special-builtin persist deleted from this scope.
/// PERSISTENT (posix special builtin / export / readonly): keep the live values
/// and delete these names from every enclosing scope so their restores skip them.
fn finalize_inline_scope(snap: AssignmentSnapshot, persistent: bool, shell: &mut Shell) {
    let kept = shell.inline_scopes.pop().unwrap_or_default();
    if persistent {
        // Only POSIX mode propagates a persist THROUGH an enclosing
        // temp-assignment scope. In default mode export/readonly keep their
        // value at their own level (snap not restored here) but must NOT
        // survive an enclosing same-name restore — so skip the deletion.
        if shell.shell_options.posix {
            for (name, _) in &snap {
                for scope in shell.inline_scopes.iter_mut() {
                    scope.remove(name);
                }
            }
        }
    } else {
        for (name, prior) in snap.into_iter().rev() {
            if kept.contains(&name) {
                shell.restore_var(&name, prior);
            }
        }
    }
}

/// Returns the static text of `word` iff it is a single unquoted `Literal`
/// part (e.g. a statically-written `command`/`declare`/`-p`). Returns `None`
/// for dynamic words (`$x`, quoted, multi-part). Mirrors
/// `ExecCommand::program_static_text` at the `Word` level.
fn word_static_text(word: &crate::lexer::Word) -> Option<String> {
    if word.0.len() == 1
        && let crate::lexer::WordPart::Literal { text, quoted: false } = &word.0[0]
    {
        return Some(text.clone());
    }
    None
}

/// For a `command …` invocation, scans the RAW arg Words for the leading
/// `-p` / `--` flags (statically) and returns the index of the first operand
/// IFF that operand is statically a declaration builtin (`declare`/`export`/…).
/// Returns `None` otherwise (no operand, dynamic flag/operand, or non-decl
/// operand), in which case the normal post-resolve `command` path handles it.
///
/// This narrow static match exists only to keep a compound `arr=(x y z)` RHS
/// (a parser-internal `ArrayLiteral` WordPart) away from the outer `resolve`,
/// which would panic trying to `expand()` it under the non-declaration
/// `command` program.
fn command_decl_operand_index(args: &[crate::lexer::Word]) -> Option<usize> {
    let mut i = 0;
    loop {
        match word_static_text(args.get(i)?).as_deref() {
            Some("-p") => i += 1,
            Some("--") => {
                i += 1;
                break;
            }
            Some(s) if s.starts_with('-') && s.len() > 1 => return None,
            _ => break, // first operand (or a dynamic word)
        }
    }
    let prog = word_static_text(args.get(i)?)?;
    if builtins::is_declaration_command(&prog) {
        Some(i)
    } else {
        None
    }
}

/// True iff `word` has a trailing `ArrayLiteral` WordPart (the lexer
/// shape produced for `name=(...)` / `name+=(...)`).
fn is_array_value_word(word: &crate::lexer::Word) -> bool {
    matches!(
        word.0.last(),
        Some(crate::lexer::WordPart::ArrayLiteral(_))
    )
}

/// Applies one `Assignment` to `shell`. Dispatches on the four
/// combinations of (target kind, value kind):
///   1. Bare + compound array RHS  →  `replace_indexed` / `extend_indexed`
///   2. Bare + scalar RHS          →  `try_set` / scalar+=value
///   3. Indexed + scalar RHS       →  `set_indexed_element` / `append_indexed_element`
///   4. Indexed + compound array   →  rejected (matches bash)
///
/// Returns `Err(())` on readonly violation or other write failure
/// (diagnostic printed by the mutator). On success returns `Ok(())`.
pub(crate) fn apply_one_assignment(
    a: &crate::command::Assignment,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<(), ()> {
    use crate::command::AssignTarget;

    let trailing_array_literal: Option<&Vec<crate::lexer::ArrayLiteralElement>> =
        a.value.0.last().and_then(|wp| {
            if let crate::lexer::WordPart::ArrayLiteral(els) = wp {
                Some(els)
            } else {
                None
            }
        });

    // ───── Associative variant dispatch ─────
    // If the target name is currently bound as an associative array,
    // subscripts are string-evaluated and writes route through the
    // associative mutators. Positional-list `m=(x y z)` and scalar
    // `m=v` are rejected (bash type-mismatch).
    //
    // Note: guarding with `!shell.is_nameref(target_name)` here would break
    // `declare -n r=assoc_arr; r[k]=v` (associative write through a nameref).
    // The minor double-warning for cyclic namerefs (get_associative resolves
    // the chain once, then the funnel resolves again) is stderr-only and benign.
    let target_name = a.target.name();
    if shell.get_associative(target_name).is_some() {
        match (&a.target, trailing_array_literal) {
            (AssignTarget::Bare(name), Some(elements)) => {
                if a.append {
                    // Pre-validate readonly so the loop below cannot partial-write.
                    // Skip for namerefs — the individual element writes go through
                    // assign() which checks the resolved target.
                    if !shell.is_nameref(target_name) && shell.is_readonly(target_name) {
                        e!(err, "huck: {target_name}: readonly variable");
                        return Err(());
                    }
                    let new_pairs = build_associative_map(elements, shell, err)?;
                    for (k, v) in new_pairs {
                        shell
                            .set_associative_element(name, k, v)
                            .map_err(|_| ())?;
                    }
                    return Ok(());
                } else {
                    let pairs = build_associative_map(elements, shell, err)?;
                    return shell.replace_associative(name, pairs).map_err(|_| ());
                }
            }
            (AssignTarget::Bare(name), None) => {
                e!(err,
                    "huck: {name}: {} not valid on associative array",
                    if a.append { "scalar append" } else { "scalar assignment" }
                );
                return Err(());
            }
            (AssignTarget::Indexed { name, subscript }, None) => {
                let key = crate::expand::eval_subscript_key(subscript, shell);
                let val = crate::param_expansion::expand_word_to_string(&a.value, shell);
                if a.append {
                    return shell
                        .append_associative_element(name, &key, &val)
                        .map_err(|_| ());
                } else {
                    return shell
                        .set_associative_element(name, key, val)
                        .map_err(|_| ());
                }
            }
            (AssignTarget::Indexed { name, .. }, Some(_)) => {
                e!(err,
                    "huck: {name}: cannot assign array literal to associative array element"
                );
                return Err(());
            }
        }
    }

    match (&a.target, trailing_array_literal) {
        // Bare name + compound array RHS.
        (AssignTarget::Bare(name), Some(elements)) => {
            if a.append {
                // a+=(elements): field-expand each bare element (split/glob/[@])
                // and append after the current max index, honoring explicit
                // [i]=v elements. Readonly pre-check avoids a partial write.
                // Skip for namerefs — assign() checks the resolved target.
                if !shell.is_nameref(name) && shell.is_readonly(name) {
                    e!(err, "huck: {name}: readonly variable");
                    return Err(());
                }
                // Starting auto-index: max+1 for an existing array; 1 for a
                // scalar (which promotes to element 0); 0 when unset — matching
                // extend_indexed's promotion.
                // get_indexed is nameref-aware, so it sees the target's array for namerefs.
                let start = if shell.get_indexed(name).is_some() {
                    shell.array_max_index(name).map_or(0, |m| m + 1)
                } else if shell.lookup_var(name).is_some() {
                    // Use lookup_var (nameref-aware) so a nameref to a scalar gives start=1.
                    1
                } else {
                    0
                };
                let map = expand_array_elements(elements, name, shell, start, err)?;
                shell.extend_indexed(name, map).map_err(|_| ())
            } else {
                // a=(elements): replace whole array.
                let map = build_array_map(elements, name, shell, err)?;
                shell.replace_indexed(name, map).map_err(|_| ())
            }
        }
        // Bare name + scalar RHS.
        (AssignTarget::Bare(name), None) => {
            let s = expand_assignment(&a.value, shell);
            if a.append {
                // a+=v: on a scalar, concatenate; on an array, append to element 0
                // (bash: `a=(x y); a+=z; echo "${a[0]}"` → "xz").
                match shell.get_indexed(name) {
                    Some(_) => shell
                        .append_indexed_element(name, 0, &s)
                        .map_err(|_| ()),
                    None => {
                        // Use lookup_var (nameref-aware) so that `r+=v` where r is a
                        // nameref prepends with the TARGET's current value, not the
                        // raw nameref binding string.
                        let existing = shell.lookup_var(name).unwrap_or_default();
                        // For an integer-flagged target, `+=` is ARITHMETIC addition
                        // (bash: `declare -i s=5; s+=3` -> 8), not string concatenation.
                        // Check the effective target's integer attribute (resolving a
                        // nameref only when needed, to avoid the hot-path allocation).
                        let target_is_integer = if shell.is_nameref(name) {
                            matches!(
                                shell.resolve_nameref(name),
                                crate::shell_state::ResolvedName::Name(ref n) if shell.is_integer(n)
                            )
                        } else {
                            shell.is_integer(name)
                        };
                        if target_is_integer {
                            let cur = arith_eval_operand(&existing, shell).unwrap_or(0);
                            let add = arith_eval_operand(&s, shell).unwrap_or(0);
                            shell.try_set(name, (cur + add).to_string()).map_err(|_| ())
                        } else {
                            shell.try_set(name, existing + &s).map_err(|_| ())
                        }
                    }
                }
            } else {
                shell.try_set(name, s).map_err(|_| ())
            }
        }
        // Subscripted lvalue + scalar RHS.
        (AssignTarget::Indexed { name, subscript }, None) => {
            let idx = match crate::expand::eval_subscript(subscript, shell, name) {
                Ok(i) => i,
                Err(msg) => {
                    e!(err, "huck: {msg}");
                    return Err(());
                }
            };
            let v = expand_assignment(&a.value, shell);
            if a.append {
                // Integer array element `a[i]+=v` is ARITHMETIC addition, like
                // the scalar case (bash: `declare -ai a=(5); a[0]+=3` -> 8).
                if shell.is_integer(name) {
                    let existing = shell.lookup_indexed_element(name, idx).unwrap_or_default();
                    let cur = arith_eval_operand(&existing, shell).unwrap_or(0);
                    let add = arith_eval_operand(&v, shell).unwrap_or(0);
                    shell.set_indexed_element(name, idx, (cur + add).to_string()).map_err(|_| ())
                } else {
                    shell.append_indexed_element(name, idx, &v).map_err(|_| ())
                }
            } else {
                shell.set_indexed_element(name, idx, v).map_err(|_| ())
            }
        }
        // Subscripted lvalue + compound array RHS: bash rejects this.
        (AssignTarget::Indexed { name, .. }, Some(_)) => {
            e!(err, "huck: {name}: cannot assign array literal to array element");
            Err(())
        }
    }
}

/// Builds an associative-array initializer from the compound literal's
/// elements. Each element MUST have an explicit subscript ([key]=value);
/// positional elements (no subscript) are an error.
fn build_associative_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<Vec<(String, String)>, ()> {
    let mut out: Vec<(String, String)> = Vec::new();
    for elem in elements {
        let key = match &elem.subscript {
            Some(sw) => crate::expand::eval_subscript_key(sw, shell),
            None => {
                e!(err, "huck: associative array initializer requires [key]=value form");
                return Err(());
            }
        };
        let val = crate::param_expansion::expand_word_to_string(&elem.value, shell);
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = val;
        } else {
            out.push((key, val));
        }
    }
    Ok(out)
}

/// Builds the `BTreeMap<usize, String>` for a compound `name=(...)`
/// RHS. Implicit subscripts continue from the highest explicit
/// subscript seen so far (bash's rule for sparse mixed-form literals).
/// Field-expands a compound array literal's elements into an explicit
/// `(index → value)` map, starting bare-element auto-indexing at `start`.
///
/// Bare elements (no `[subscript]=`) go through the SAME field+glob path
/// command arguments use (`glob_expand_word`): unquoted word-splitting,
/// command-substitution splitting, pathname globbing, and the
/// quoted/unquoted `${arr[@]}`/`$@` multi-field rule — one element may yield
/// zero, one, or many values, and the implicit index advances per produced
/// FIELD. Subscripted `[i]=value` elements keep single-value semantics (no
/// splitting, via `expand_assignment`) and reset the implicit index to
/// `i + 1`. (M-112)
fn expand_array_elements(
    elements: &[crate::lexer::ArrayLiteralElement],
    name: &str,
    shell: &mut Shell,
    start: usize,
    err: &mut dyn std::io::Write,
) -> Result<std::collections::BTreeMap<usize, String>, ()> {
    let mut map: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    let mut implicit = start;
    for elem in elements {
        match &elem.subscript {
            Some(sw) => {
                let idx = match crate::expand::eval_subscript(sw, shell, name) {
                    Ok(i) => i,
                    Err(msg) => {
                        e!(err, "huck: {msg}");
                        return Err(());
                    }
                };
                map.insert(idx, expand_assignment(&elem.value, shell));
                implicit = idx + 1;
                if shell.pending_fatal_status.is_some() {
                    return Err(());
                }
            }
            None => {
                for field in glob_expand_word(&elem.value, shell, err)? {
                    map.insert(implicit, field);
                    implicit += 1;
                }
                if shell.pending_fatal_status.is_some() {
                    return Err(());
                }
            }
        }
    }
    Ok(map)
}

fn build_array_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    name: &str,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<std::collections::BTreeMap<usize, String>, ()> {
    expand_array_elements(elements, name, shell, 0, err)
}

// ----- job-control helpers -------------------------------------------------

/// Best-effort: give the controlling terminal to `pgid`. Swallows ENOTTY
/// (non-tty environments like cargo test) and EPERM (race: pgrp already
/// exited). Other errors are silently ignored too.
fn give_terminal_to(pgid: i32) {
    unsafe {
        let _ = libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
    }
}

/// Block-wait for a single child pid with WUNTRACED. Returns:
///   `Ok((raw_status, stopped))` where `stopped` is true if WIFSTOPPED.
///   `Err(())` on waitpid failure.
fn wait_with_untraced(pid: i32) -> Result<(libc::c_int, bool), ()> {
    let mut status: libc::c_int = 0;
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
    if r < 0 {
        return Err(());
    }
    Ok((status, libc::WIFSTOPPED(status)))
}

/// pre_exec closure that resets SIGTSTP/SIGTTIN/SIGTTOU to SIG_DFL in the
/// child. Required because huck SIG_IGNs these at the shell level and
/// SIG_IGN is inherited across exec — without this, Ctrl-Z would never
/// stop a foreground job, and a background reader could never SIGTTIN.
fn reset_job_control_signals_in_child() -> std::io::Result<()> {
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        libc::signal(libc::SIGTTIN, libc::SIG_DFL);
        libc::signal(libc::SIGTTOU, libc::SIG_DFL);
    }
    Ok(())
}

// ----- fork subshell helper -------------------------------------------------

/// Forks a subshell and runs `cmd` in the child with the supplied stdio
/// fds dup2'd to 0/1/2. After the body runs, the child `_exit`s with the
/// resulting status. Returns the child pid in the parent.
///
/// `parent_fds_to_close` lists pipe fds the parent holds that this child
/// must close (else EOF propagation fails downstream).
///
/// `pgid_target`: 0 = become own pgrp leader; >0 = join this pgrp.
///
/// `stdout_dup_target` / `stderr_dup_target`: if `Some(fd)`, after the
/// normal stdio dup2s, apply `dup2(fd, 1)` and/or `dup2(fd, 2)` in the
/// child. Used for `Redirect::Dup` (e.g. `2>&1`). Resolution happens in
/// the parent (pre-fork) so this is always an i32, never a Word.
#[allow(clippy::too_many_arguments)]
pub fn fork_and_run_in_subshell(
    cmd: &Command,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    stdout_dup_target: Option<i32>,
    stderr_dup_target: Option<i32>,
) -> Result<i32, io::Error> {
    // Flush buffered parent stdout BEFORE forking so the child does not inherit
    // (and then re-flush, duplicating) any pending partial line, and so pending
    // parent bytes are ordered ahead of the child's output.
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // CHILD: async-signal-safe-ish operations only until we dive into
        // `run_command`. huck is single-threaded so this is fine.
        unsafe {
            // 1. Reset job-control signals.
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::signal(libc::SIGTTIN, libc::SIG_DFL);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            // v137: a forked pipeline stage / subshell dies on a broken pipe
            // like bash. Redundant with the startup reset in the common case,
            // but also correct for the PIPE-trap case: bash resets a trapped
            // signal to default inside a subshell, so a forked stage must not
            // inherit a top-level PIPE handler.
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            // 2. Join the pgrp (leader if pgid_target == 0); NO_PGROUP (< 0)
            //    means "stay in the shell's group" (job control off).
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
            // 3. dup2 the stdio fds to 0/1/2.
            if stdin_fd != 0 {
                libc::dup2(stdin_fd, 0);
            }
            if stdout_fd != 1 {
                libc::dup2(stdout_fd, 1);
            }
            if stderr_fd != 2 {
                libc::dup2(stderr_fd, 2);
            }
            // 4. Close the originals if not already at 0/1/2.
            for fd in [stdin_fd, stdout_fd, stderr_fd] {
                if fd > 2 {
                    libc::close(fd);
                }
            }
            // 5. Close every other pipe fd the parent held, skipping any
            //    that were one of our stdio sources (they may have been > 2
            //    and are already closed above, but guard against the case
            //    where a parent_fds_to_close entry coincides with stdin/
            //    stdout/stderr — we must not close what we just dup2'd from).
            for &fd in parent_fds_to_close {
                if fd != stdin_fd && fd != stdout_fd && fd != stderr_fd {
                    libc::close(fd);
                }
            }
            // 6. Apply Dup redirects: stdout-dup BEFORE stderr-dup (matches
            //    canonical `>file 2>&1` semantics where stdout is set first).
            //    These run after the normal stdio dup2s so the target fds are
            //    already at their final positions.
            if let Some(fd) = stdout_dup_target {
                libc::dup2(fd, 1);
            }
            if let Some(fd) = stderr_dup_target {
                libc::dup2(fd, 2);
            }
        }
        // POSIX: subshells reset traps to default. Clear all huck
        // trap state so the parent's EXIT trap and real-signal traps
        // don't inherit into the child.
        crate::traps::clear_for_subshell(shell);
        // Mark this process as a forked subshell so its inner pipelines skip
        // interactive job-control process-grouping (deadlocks on a tty — M-104).
        shell.in_subshell = true;
        // 8. Run the body via the existing dispatcher.
        //    The child's stdout is now fd 1 (the dup2'd pipe end), so
        //    StdoutSink::Terminal routes writes to the right destination.
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        // Anti-recursion guard: when a Command::Subshell is used as a
        // pipeline stage, the pipeline forks via this helper.  If we called
        // run_command here, it would fork AGAIN.  Instead, dispatch via
        // execute() so that body.background is honoured — `(cmd &)` inside a
        // subshell must background the inner command and let the subshell exit.
        // execute() calls execute_sequence_body when background is false
        // (the common case), preserving the single-fork invariant.
        let outcome = match cmd {
            Command::Subshell { body } => execute(body, shell, "(subshell)"),
            other => run_command(other, shell, &mut sink, &mut err_sink),
        };
        // 9. Translate outcome to an 8-bit exit status.
        let status: i32 = match outcome {
            ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
            ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
            ExecOutcome::FunctionReturn(n) => n,
            ExecOutcome::Interrupted(InterruptReason::Sigint) => 130,
            ExecOutcome::Interrupted(InterruptReason::Timeout) => 124,
        };
        let status = status.rem_euclid(256);
        // Flush the builtin's buffered stdout to the dup2'd fd 1 (pipe or
        // terminal) before _exit (M-118): _exit skips Rust's flush, which is
        // wanted for parent-state side effects but would drop a trailing line.
        flush_stdout();
        // _exit bypasses Drop and Rust's atexit/flush machinery, which is
        // exactly what we want: the parent's history.save() etc. must not run.
        unsafe { libc::_exit(status) };
    }
    // PARENT: defensive setpgid to close the race with the child's setpgid.
    // Skipped when pgid_target == NO_PGROUP (job control off — stay in shell group).
    if pgid_target >= 0 {
        unsafe {
            libc::setpgid(pid, pgid_target);
        }
    }
    Ok(pid)
}

// ----- stage classification + raw-fd external spawn (Task 4) ---------------

/// Decides whether a pipeline stage should run via `std::process::Command`
/// (External) or via `fork_and_run_in_subshell` (InProcess).
///
/// Returns `External` only when:
///   - `cmd` is `Command::Simple(SimpleCommand::Exec(exec))`,
///   - AND `exec.program_static_text()` returns `Some(name)` (single unquoted Literal),
///   - AND `name` is NOT in `shell.functions`,
///   - AND NOT in `builtins::is_builtin`.
///
/// Everything else (compounds, function calls, builtins, dynamic program words,
/// assignment-only stages) → `InProcess`.
///
enum StageKind<'a> {
    /// A `SimpleCommand::Exec` that resolves to an external binary.
    External(&'a SimpleCommand),
    /// Everything else: builtins, functions, compounds, dynamic program words.
    InProcess(&'a Command),
}

fn classify_stage<'a>(cmd: &'a Command, shell: &Shell) -> StageKind<'a> {
    if let Command::Simple(simple) = cmd
        && let SimpleCommand::Exec(exec) = simple
        && let Some(prog) = exec.program_static_text()
        && !shell.functions.contains_key(&prog)
        && !builtins::is_builtin(&prog)
    {
        return StageKind::External(simple);
    }
    StageKind::InProcess(cmd)
}

/// Spawns an external command with pre-opened raw stdio fds.
///
/// Converts `stdin_fd`/`stdout_fd`/`stderr_fd` to `Stdio` via
/// `OwnedFd::from_raw_fd` (transfers ownership — the caller must NOT close
/// these fds after calling this function; `std::process::Command` handles
/// closing them in the parent after the fork).
///
/// `pgid_target`: 0 = become own pgrp leader; >0 = join this pgrp.
///
/// `parent_fds_to_close`: pipe fds held by the parent that the child must
/// close in its `pre_exec` hook so EOF propagates correctly downstream.
///
/// Returns the child's pid. The `Child` handle is `mem::forget`'d (matching
/// the B-09 pattern) since the caller is responsible for `waitpid`.
///
#[allow(clippy::too_many_arguments)]
fn spawn_external_with_fds(
    cmd: &SimpleCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    // Flush pending parent stdout before spawning an external stage so its output
    // does not race ahead of buffered parent bytes (M-118 sibling: ordering).
    flush_stdout();
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let SimpleCommand::Exec(exec) = cmd else {
        // Assign-only stages are classified as InProcess by classify_stage;
        // reaching here is a caller bug.
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "spawn_external_with_fds called on Assign stage"));
    };

    // Resolve (expand) the command — same path as run_exec_single / run_multi_stage.
    let resolved = resolve(exec, shell, &mut *err_writer(err_sink, sink))
        .map_err(|code| io::Error::other(format!("resolve failed with code {code}")))?;

    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}{}",
                  xtrace_command_line(&[], &resolved.program, &resolved.args)));
    }

    // Resolve Dup targets pre-fork (Word expansion may allocate; not async-signal-safe).
    // stdout-dup BEFORE stderr-dup matches canonical `>file 2>&1` semantics.
    let stdout_dup_target: Option<i32> = match &exec.slot_stdout() {
        Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
        _ => None,
    };
    let stderr_dup_target: Option<i32> = match &exec.slot_stderr() {
        Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
        _ => None,
    };

    // v156 task 4 (additive): lower the redirects the 0/1/2 bridge does NOT
    // consume (fd>2, `<&` dup-in, `N>&-` close, `<>`) into an ordered replay
    // applied in the child AFTER the bridge stdio/dup. `extra_held` keeps the
    // parent-opened files alive (FD_CLOEXEC) until after spawn.
    let (extra_ops, extra_held) = build_child_extra_ops(&exec.redirects, shell, sink, err_sink)
        .map_err(|code| io::Error::other(format!("redirect failed with code {code}")))?;

    let mut process = ProcessCommand::new(&resolved.program);
    process.args(&resolved.args);
    process.env_clear();
    process.envs(shell.exported_env());
    process.envs(shell.exported_function_env());

    // Reset job-control signals to SIG_DFL before exec.
    unsafe { process.pre_exec(reset_job_control_signals_in_child); }

    // If there are Dup redirects, chain a second pre_exec to apply dup2 in the
    // child. This runs AFTER the signal-reset pre_exec (registration order).
    // stdout-dup BEFORE stderr-dup matches canonical `>file 2>&1` semantics.
    if stdout_dup_target.is_some() || stderr_dup_target.is_some() {
        unsafe {
            process.pre_exec(move || {
                if let Some(fd) = stdout_dup_target && libc::dup2(fd, 1) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if let Some(fd) = stderr_dup_target && libc::dup2(fd, 2) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Collect the target fds that extra_ops will set up in the child, so we
    // can exclude them from fds_to_close below (Fix B: a dup2 then close on
    // the same fd would silently defeat the redirect).
    let extra_targets: Vec<RawFd> = extra_ops.iter().map(|op| match *op {
        ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target,
    }).collect();

    // Replay the extra (fd>2 / dup-in / close / ReadWrite) ops in source order,
    // AFTER the bridge stdio + dup-target pre_execs above. Pure dup2/close, so
    // async-signal-safe. Runs even when the bridge dup pre_exec is absent.
    if !extra_ops.is_empty() {
        let ops = extra_ops;
        unsafe {
            process.pre_exec(move || replay_redir_ops(&ops));
        }
    }

    // Join the pgrp (leader if pgid_target == 0); NO_PGROUP (< 0) means "stay in
    // the shell's group" (job control off) — skip the pre-exec setpgid entirely
    // (process_group(-1) would setpgid(0, -1) → EINVAL).
    if pgid_target >= 0 {
        process.process_group(pgid_target);
    }

    // Convert raw fds to Stdio. For fds that are already at their "natural"
    // slot (stdin=0, stdout=1, stderr=2), use Stdio::inherit() so we don't
    // accidentally close the parent's standard streams. For other fds, use
    // OwnedFd::from_raw_fd which transfers ownership — std::process::Command
    // closes the original fd in the parent after forking.
    // For Dup-redirect fds, always use Stdio::inherit() to avoid the
    // close-on-drop trap of OwnedFd (the dup2 pre_exec handles the actual
    // redirect in the child).
    let stdin_stdio = if stdin_fd == 0 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stdin_fd)) }
    };
    let stdout_stdio = if stdout_dup_target.is_some() {
        // Dup on stdout: inherit so the dup2 pre_exec can redirect to target.
        // We must still consume stdout_fd so it isn't leaked in the parent.
        if stdout_fd != 1 {
            unsafe { libc::close(stdout_fd); }
        }
        Stdio::inherit()
    } else if stdout_fd == 1 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stdout_fd)) }
    };
    let stderr_stdio = if stderr_dup_target.is_some() {
        // Dup on stderr: inherit so the dup2 pre_exec can redirect to target.
        if stderr_fd != 2 {
            unsafe { libc::close(stderr_fd); }
        }
        Stdio::inherit()
    } else if stderr_fd == 2 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stderr_fd)) }
    };

    process.stdin(stdin_stdio);
    process.stdout(stdout_stdio);
    process.stderr(stderr_stdio);

    // In the child's pre_exec, close every parent-held pipe fd that this
    // child shouldn't inherit (so downstream readers see EOF).
    // Exclude any fd that extra_ops already claimed as a redirect target: a
    // dup2 into that fd followed by close(fd) would silently defeat the redirect.
    // The closure must be async-signal-safe; libc::close is.
    let fds_to_close: Vec<RawFd> = parent_fds_to_close.iter()
        .copied()
        .filter(|fd| !extra_targets.contains(fd))
        .collect();
    unsafe {
        process.pre_exec(move || {
            for &fd in &fds_to_close {
                libc::close(fd);
            }
            Ok(())
        });
    }

    let spawn_result = process.spawn();
    // The child inherited the parent-opened extra-redirect fds (FD_CLOEXEC).
    // Drop the parent's copies now so they don't leak.
    drop(extra_held);
    let child = spawn_result?;
    let pid = child.id() as i32;

    // Defensive setpgid in parent to close the race with the child's setpgid
    // (set via process_group above, which runs pre-exec in the child).
    // Skipped when pgid_target == NO_PGROUP (job control off — stay in shell group).
    if pgid_target >= 0 {
        unsafe {
            let _ = libc::setpgid(pid, pgid_target);
        }
    }

    // mem::forget the Child handle — the caller waitpids manually (B-09 pattern).
    std::mem::forget(child);

    Ok(pid)
}

// ----- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, ExecCommand, IfClause, Pipeline, Sequence, SimpleCommand};
    use crate::lexer::{Word, WordPart};
    use crate::test_support::CWD_LOCK;

    fn exec_args(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn render_test_leaf_forms() {
        let mut shell = Shell::new();
        shell.set("v", "hi".into());
        let parse_expr = |src: &str| {
            let toks = crate::lexer::tokenize(src).expect("lex");
            match crate::command::parse(toks).expect("parse").expect("seq").first {
                crate::command::Command::DoubleBracket { expr, .. } => *expr,
                other => panic!("expected [[ ]], got {other:?}"),
            }
        };
        assert_eq!(render_test_leaf(&parse_expr("[[ -n $v ]]"), &mut shell), "-n hi");
        assert_eq!(render_test_leaf(&parse_expr("[[ -z \"\" ]]"), &mut shell), "-z ''");
        assert_eq!(render_test_leaf(&parse_expr("[[ $v == h* ]]"), &mut shell), "hi == h*");
        assert_eq!(render_test_leaf(&parse_expr("[[ 5 -gt 3 ]]"), &mut shell), "5 -gt 3");
    }

    #[test]
    fn parse_exec_flags_plain_command() {
        let f = parse_exec_flags(&exec_args(&["echo", "hi"])).unwrap();
        assert!(!f.clear_env && !f.login && f.argv0.is_none());
        assert_eq!(f.operand_start, 0);
    }

    #[test]
    fn parse_exec_flags_c_l_and_a_separate() {
        let f = parse_exec_flags(&exec_args(&["-c", "-l", "-a", "NAME", "prog"])).unwrap();
        assert!(f.clear_env && f.login);
        assert_eq!(f.argv0.as_deref(), Some("NAME"));
        assert_eq!(f.operand_start, 4);
    }

    #[test]
    fn parse_exec_flags_clustered_and_inline_a() {
        // `-cla NAME` clusters -c, -l, and -a with NAME as the next word.
        let f = parse_exec_flags(&exec_args(&["-cla", "NAME", "prog"])).unwrap();
        assert!(f.clear_env && f.login);
        assert_eq!(f.argv0.as_deref(), Some("NAME"));
        assert_eq!(f.operand_start, 2);
        // `-aZERO prog`: argv0 is the inline remainder of the word.
        let f2 = parse_exec_flags(&exec_args(&["-aZERO", "prog"])).unwrap();
        assert_eq!(f2.argv0.as_deref(), Some("ZERO"));
        assert_eq!(f2.operand_start, 1);
    }

    #[test]
    fn parse_exec_flags_double_dash_and_bare_dash() {
        let f = parse_exec_flags(&exec_args(&["--", "-prog"])).unwrap();
        assert_eq!(f.operand_start, 1);
        // A bare `-` is an operand, not a flag.
        let f2 = parse_exec_flags(&exec_args(&["-"])).unwrap();
        assert_eq!(f2.operand_start, 0);
    }

    #[test]
    fn parse_exec_flags_errors() {
        assert!(parse_exec_flags(&exec_args(&["-Z"])).is_err());
        assert!(parse_exec_flags(&exec_args(&["-a"])).is_err()); // -a needs an argument
    }

    #[test]
    fn parse_exec_flags_no_command_only_flags() {
        let f = parse_exec_flags(&exec_args(&["-c"])).unwrap();
        assert!(f.clear_env);
        assert_eq!(f.operand_start, 1); // == args.len(): no operand
    }

    #[test]
    fn ps4_cmdsub_preserves_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(7);
        shell.set("PS4", "$(false)+ ".to_string());
        let _ = ps4(&mut shell);
        assert_eq!(shell.last_status(), 7, "rendering PS4 must not clobber $?");
    }

    #[test]
    fn ps4_cmdsub_under_xtrace_does_not_recurse() {
        let mut shell = Shell::new();
        shell.shell_options.xtrace = true;
        shell.set("PS4", "$(true) ".to_string());
        // Without the xtrace-suppression fix, expanding PS4 runs `true` which is
        // traced -> re-enters ps4() -> infinite recursion -> stack overflow.
        let _ = ps4(&mut shell);
        // Reaching here (no abort) IS the assertion. Also confirm xtrace restored:
        assert!(shell.shell_options.xtrace, "xtrace must be restored after ps4");
    }

    #[test]
    fn open_writable_guard_creates_new_file() {
        let dir = std::env::temp_dir().join(format!("huck_nc_new_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("new.txt");
        let _ = std::fs::remove_file(&p);
        let f = open_writable(p.to_str().unwrap(), true);
        assert!(f.is_ok(), "guarded open should create a nonexistent file");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn open_writable_guard_blocks_existing_regular_file() {
        let dir = std::env::temp_dir().join(format!("huck_nc_block_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("exists.txt");
        std::fs::write(&p, b"orig").unwrap();
        let f = open_writable(p.to_str().unwrap(), true);
        assert!(f.is_err(), "guarded open must refuse an existing regular file");
        assert_eq!(f.err().unwrap().to_string(), "cannot overwrite existing file");
        assert_eq!(std::fs::read(&p).unwrap(), b"orig");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn open_writable_guard_exempts_dev_null() {
        let f = open_writable("/dev/null", true);
        assert!(f.is_ok(), "guarded open must allow non-regular files like /dev/null");
    }

    #[test]
    fn open_writable_unguarded_truncates_existing() {
        let dir = std::env::temp_dir().join(format!("huck_nc_trunc_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("trunc.txt");
        std::fs::write(&p, b"original-content").unwrap();
        { let _f = open_writable(p.to_str().unwrap(), false).unwrap(); }
        assert_eq!(std::fs::read(&p).unwrap(), b"", "unguarded open should truncate");
        let _ = std::fs::remove_file(&p);
    }

    /// A top-level sequence wrapping a single Command.
    fn seq_of(cmd: Command) -> Sequence {
        Sequence { first: cmd, rest: vec![], background: false }
    }

    /// A one-pipeline Sequence running `echo <word>`.
    fn echo_seq(word: &str) -> Sequence {
        let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: ww("echo"),
                    args: vec![ww(word)],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    /// A one-pipeline condition Sequence with a known exit status: true
    /// (exit 0) when `succeed`, false (exit 1) otherwise. Built from the
    /// side-effect-free `test` builtin — `test 0 -eq 0` succeeds,
    /// `test 1 -eq 0` fails.
    fn cond_seq(succeed: bool) -> Sequence {
        let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
        let lhs = if succeed { "0" } else { "1" };
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: ww("test"),
                    args: vec![ww(lhs), ww("-eq"), ww("0")],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    fn lit_word(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    fn bare_assign(name: &str, value: Word) -> crate::command::Assignment {
        crate::command::Assignment {
            target: crate::command::AssignTarget::Bare(name.to_string()),
            value,
            append: false,
        }
    }

    fn exec(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: lit_word(program),
            args: args.iter().map(|a| lit_word(a)).collect(),
            redirects: Vec::new(),
            line: 0,
        })
    }

    fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline { negate: false, commands: vec![Command::Simple(cmd)] }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn if_true_condition_runs_then_body() {
        let clause = IfClause {
            condition: cond_seq(true),
            then_body: echo_seq("yes"),
            elif_branches: vec![],
            else_body: None,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
        assert_eq!(out.trim(), "yes");
        assert_eq!(status, 0);
    }

    #[test]
    fn if_false_condition_runs_else_body() {
        let clause = IfClause {
            condition: cond_seq(false),
            then_body: echo_seq("yes"),
            elif_branches: vec![],
            else_body: Some(echo_seq("no")),
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
        assert_eq!(out.trim(), "no");
    }

    #[test]
    fn if_false_no_else_runs_nothing_status_zero() {
        let clause = IfClause {
            condition: cond_seq(false),
            then_body: echo_seq("yes"),
            elif_branches: vec![],
            else_body: None,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn if_elif_selects_matching_branch() {
        use crate::command::ElifBranch;
        let clause = IfClause {
            condition: cond_seq(false),
            then_body: echo_seq("a"),
            elif_branches: vec![ElifBranch {
                condition: cond_seq(true),
                body: echo_seq("b"),
            }],
            else_body: Some(echo_seq("c")),
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
        assert_eq!(out.trim(), "b");
    }

    #[test]
    fn execute_capturing_echo_returns_raw_output_with_newline() {
        // execute_capturing does NOT strip; that happens in expand::run_substitution.
        let seq = one_command_sequence(exec("echo", &["hi"]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "hi\n");
        assert_eq!(status, 0);
    }

    #[test]
    fn execute_capturing_exit_returns_status() {
        let seq = one_command_sequence(exec("exit", &["7"]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "");
        assert_eq!(status, 7);
    }

    #[test]
    fn execute_capturing_empty_echo() {
        let seq = one_command_sequence(exec("echo", &[]));
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "\n");
        assert_eq!(status, 0);
    }

    #[test]
    fn execute_capturing_builtin_pipeline_captures_terminal_stage() {
        // Two-stage pipeline: `echo first | echo second`. The terminal stage
        // is a builtin (echo) whose output should land in the capture buffer.
        // The first stage's output is discarded by echo (which doesn't read
        // stdin), so we just confirm the terminal echo's output is captured.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(exec("echo", &["first"])), Command::Simple(exec("echo", &["second"]))],
            }),
            rest: vec![],
            background: false,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "second\n");
        assert_eq!(status, 0);
    }

    

    #[test]
    fn background_pure_builtin_forks_and_registers_job() {
        // Post-fix: `echo hi &` (a single-stage pure-builtin pipeline) now forks
        // a subshell rather than running synchronously in the parent. The job
        // should appear in the table immediately after execute() returns (before
        // wait/reap), because the fork registered it as Running.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(exec("echo", &["hi"]))],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let outcome = execute(&seq, &mut shell, "echo hi &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // last_bg_pid must have been set to a real forked pid.
        assert!(shell.last_bg_pid.is_some(), "last_bg_pid should be set after pure-builtin &");
        let pid = shell.last_bg_pid.unwrap();
        assert!(pid > 0, "pid should be positive, got {pid}");
    }

    #[test]
    fn background_pure_builtin_does_not_mutate_parent_env() {
        // Post-fix: `HUCK_TEST_BG_ASSIGN=v &` runs in a forked subshell, so
        // the assignment must NOT leak back to the parent shell's environment.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Assign(vec![
                    bare_assign("HUCK_TEST_BG_ASSIGN", lit_word("v")),
                ], 0))],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let _ = execute(&seq, &mut shell, "HUCK_TEST_BG_ASSIGN=v &");
        // The assignment ran in a forked subshell — should NOT be visible in parent.
        assert_eq!(shell.get("HUCK_TEST_BG_ASSIGN"), None);
    }

    #[test]
    fn execute_capturing_ignores_background_flag_runs_synchronously() {
        // `$(cmd &)` must wait and capture, not spawn an escaped bg job.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(exec("echo", &["captured"]))],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "captured\n");
        assert_eq!(status, 0);
        // And nothing should have been registered in the job table.
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn give_terminal_to_silently_succeeds_on_non_tty() {
        // cargo test runs without a controlling terminal; tcsetpgrp returns
        // ENOTTY. The helper must swallow it.
        give_terminal_to(1); // bogus pgid; we only care that we don't panic
    }

    #[test]
    fn stray_break_at_top_level_errors_with_status_0() {
        // `break` with no enclosing loop (loop_depth==0): emits diagnostic
        // to stderr and returns status 0 (matches bash 5.2 behavior —
        // bash returns 0, not 1, for break/continue outside a loop).
        use crate::command::{ExecCommand, Pipeline};
        use crate::lexer::{Word, WordPart};
        let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: ww("break"),
                    args: vec![],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        };
        let mut shell = Shell::new();
        let (_out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(status, 0);
    }

    #[test]
    fn brace_group_assignments_affect_current_shell() {
        // A brace group has NO subshell isolation — `x=value` inside it
        // is visible after.
        let assign = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Assign(vec![
                    bare_assign("BG_X", Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }])),
                ], 0))],
            }),
            rest: vec![],
            background: false,
        };
        let group = Sequence {
            first: Command::BraceGroup(Box::new(assign)),
            rest: vec![],
            background: false,
        };
        let mut shell = Shell::new();
        let (_, status) = execute_capturing(&group, &mut shell);
        assert_eq!(status, 0);
        assert_eq!(shell.get("BG_X"), Some("hello"));
    }

    #[test]
    fn redirect_target_does_not_glob() {
        let _g = CWD_LOCK.lock().unwrap();
        // Create a temp dir with a real file matching the literal pattern name.
        let tmp = tempfile::tempdir().unwrap();
        // The file is named literally "starfile" — `*` should not glob to it.
        std::fs::write(tmp.path().join("starfile"), b"hello\n").unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        // Build a redirect target word containing unquoted `*` and verify expand_single
        // returns the literal "*" (not a glob match) — proving redirects bypass globbing.
        let word = crate::lexer::Word(vec![
            crate::lexer::WordPart::Literal { text: "*".to_string(), quoted: false }
        ]);
        let mut shell = crate::shell_state::Shell::new();
        let result = expand_single(&word, &mut shell, &mut std::io::stderr());

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(result, Ok("*".to_string()));
    }

    use crate::command::WhileClause;

    /// A Sequence wrapping a single `while`/`until` clause.
    fn while_seq(clause: WhileClause) -> Sequence {
        Sequence { first: Command::While(Box::new(clause)), rest: vec![], background: false }
    }

    use crate::command::ForClause;

    /// A Sequence wrapping a single `for` clause.
    fn for_seq(clause: ForClause) -> Sequence {
        Sequence { first: Command::For(Box::new(clause)), rest: vec![], background: false }
    }

    /// A one-pipeline Sequence running `echo $<var>` (the variable expanded).
    fn echo_var_seq(var: &str) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: vec![Word(vec![WordPart::Var { name: var.to_string(), quoted: false }])],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    /// A one-pipeline Sequence running the `continue` builtin.
    fn continue_seq() -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "continue".to_string(), quoted: false }]),
                    args: vec![],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn for_iterates_each_value_in_order() {
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
            has_in: true,
            body: echo_var_seq("x"),
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a", "b", "c"]);
        assert_eq!(status, 0);
    }

    #[test]
    fn for_empty_list_runs_body_zero_times() {
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![],
            has_in: true,
            body: echo_seq("hi"),
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn select_empty_list_runs_no_body_and_restores_depth() {
        let mut sh = Shell::new();
        // `select x in ; do exit 7; done` — empty `in` → body never runs → status 0.
        let outcome = crate::shell::process_line("select x in; do exit 7; done", &mut sh, false);
        assert_eq!(sh.loop_depth, 0, "loop_depth must be restored");
        // The shell did not exit 7 (body never ran):
        assert!(!matches!(outcome, ExecOutcome::Exit(7)));
    }

    #[test]
    fn for_without_in_iterates_positionals() {
        // M-24a: `for x; do ... done` with no `in` iterates "$@".
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![],
            has_in: false,
            body: echo_var_seq("x"),
        };
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a", "b", "c"]);
        assert_eq!(status, 0);
    }

    #[test]
    fn for_variable_holds_last_value_after_loop() {
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
            has_in: true,
            body: echo_var_seq("x"),
        };
        let mut shell = Shell::new();
        execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(shell.get("x"), Some("c"));
    }

    #[test]
    fn for_break_stops_iteration() {
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
            has_in: true,
            body: break_seq(),
        };
        let mut shell = Shell::new();
        let (_out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(shell.get("x"), Some("a"));
        assert_eq!(status, 0);
    }

    #[test]
    fn for_continue_advances_through_all_values() {
        // body: `continue ; echo NOPE` — `continue` must skip the echo on
        // every iteration, so nothing prints, yet all values are visited.
        let echo_nope = Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                args: vec![Word(vec![WordPart::Literal { text: "NOPE".to_string(), quoted: false }])],
                redirects: Vec::new(),
                line: 0,
            }))],
        });
        let mut body = continue_seq();
        body.rest.push((crate::command::Connector::Semi, echo_nope));
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
            has_in: true,
            body,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(out.trim(), "", "continue should skip the echo: {out:?}");
        assert_eq!(shell.get("x"), Some("c"));
        assert_eq!(status, 0);
    }

    /// A one-pipeline Sequence running the `break` builtin.
    fn break_seq() -> Sequence {
        use crate::command::{ExecCommand, Pipeline};
        use crate::lexer::{Word, WordPart};
        let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: ww("break"),
                    args: vec![],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn while_false_condition_runs_body_zero_times() {
        let clause = WhileClause {
            condition: cond_seq(false),
            body: echo_seq("x"),
            until: false,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&while_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn while_true_body_breaks_runs_once() {
        // while (true); do break; done — `break` ends the loop after one
        // iteration. Reaching the assertion at all proves termination.
        let clause = WhileClause {
            condition: cond_seq(true),
            body: break_seq(),
            until: false,
        };
        let mut shell = Shell::new();
        let (_out, status) = execute_capturing(&while_seq(clause), &mut shell);
        assert_eq!(status, 0);
    }

    #[test]
    fn until_true_condition_runs_body_zero_times() {
        // until (test 0 -eq 0 -> true); do echo x; done — `until` stops
        // immediately when the condition is true.
        let clause = WhileClause {
            condition: cond_seq(true),
            body: echo_seq("x"),
            until: true,
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&while_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
    }

    // ----- case statement tests -----------------------------------------------

    use crate::command::{CaseClause, CaseItem, CaseTerminator};

    /// A Sequence wrapping a single `case` clause.
    fn case_seq(clause: CaseClause) -> Sequence {
        Sequence { first: Command::Case(Box::new(clause)), rest: vec![], background: false }
    }

    /// A CaseItem with a `;;` (Break) terminator.
    fn item(patterns: &[&str], body: Option<Sequence>) -> CaseItem {
        CaseItem {
            patterns: patterns.iter().map(|p| lit_word(p)).collect(),
            body,
            terminator: CaseTerminator::Break,
        }
    }

    #[test]
    fn case_runs_first_matching_clause() {
        let clause = CaseClause {
            subject: lit_word("foo"),
            items: vec![
                item(&["foo"], Some(echo_seq("matched"))),
                item(&["bar"], Some(echo_seq("other"))),
            ],
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.trim(), "matched");
        assert_eq!(status, 0);
    }

    #[test]
    fn case_glob_pattern_matches() {
        let clause = CaseClause {
            subject: lit_word("report.txt"),
            items: vec![item(&["*.txt"], Some(echo_seq("text")))],
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.trim(), "text");
    }

    #[test]
    fn case_alternation_matches_any() {
        let clause = CaseClause {
            subject: lit_word("b"),
            items: vec![item(&["a", "b", "c"], Some(echo_seq("hit")))],
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.trim(), "hit");
    }

    #[test]
    fn case_no_match_is_status_zero_no_output() {
        let clause = CaseClause {
            subject: lit_word("x"),
            items: vec![item(&["y"], Some(echo_seq("nope")))],
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn case_empty_body_is_status_zero() {
        let clause = CaseClause {
            subject: lit_word("x"),
            items: vec![item(&["x"], None)],
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn case_fall_through_runs_next_body() {
        // a) echo one ;&  *) echo two ;;
        let clause = CaseClause {
            subject: lit_word("a"),
            items: vec![
                CaseItem {
                    patterns: vec![lit_word("a")],
                    body: Some(echo_seq("one")),
                    terminator: CaseTerminator::FallThrough,
                },
                item(&["*"], Some(echo_seq("two"))),
            ],
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
    }

    #[test]
    fn case_continue_match_keeps_testing() {
        // a) echo one ;;&  a) echo two ;;
        let clause = CaseClause {
            subject: lit_word("a"),
            items: vec![
                CaseItem {
                    patterns: vec![lit_word("a")],
                    body: Some(echo_seq("one")),
                    terminator: CaseTerminator::ContinueMatch,
                },
                item(&["a"], Some(echo_seq("two"))),
            ],
        };
        let mut shell = Shell::new();
        let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
        assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
    }

    #[test]
    fn function_def_registers_and_returns_zero() {
        let body = Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }]),
                    args: vec![Word(vec![WordPart::Literal { text: "hi".into(), quoted: false }])],
                    redirects: Vec::new(),
                    line: 0,
                }))],
            }),
            rest: vec![],
            background: false,
        };
        let def = Sequence {
            first: Command::FunctionDef {
                name: "f".to_string(),
                body: Box::new(Command::BraceGroup(Box::new(body))),
            },
            rest: vec![],
            background: false,
        };
        let mut shell = Shell::new();
        let (_, status) = execute_capturing(&def, &mut shell);
        assert_eq!(status, 0);
        assert!(shell.functions.contains_key("f"));
    }

    #[test]
    fn case_quoted_metacharacter_matches_literally() {
        // pattern is a quoted `*` — matches the literal string "*", not "abc"
        let star_pattern = Word(vec![WordPart::Literal { text: "*".to_string(), quoted: true }]);
        let make = |subj: &str| CaseClause {
            subject: lit_word(subj),
            items: vec![CaseItem {
                patterns: vec![star_pattern.clone()],
                body: Some(echo_seq("hit")),
                terminator: CaseTerminator::Break,
            }],
        };
        let mut shell = Shell::new();
        let (out_star, _) = execute_capturing(&case_seq(make("*")), &mut shell);
        assert_eq!(out_star.trim(), "hit", "literal * should match the string \"*\"");
        let (out_abc, _) = execute_capturing(&case_seq(make("abc")), &mut shell);
        assert_eq!(out_abc.trim(), "", "quoted * must not act as a wildcard");
    }

    // ----- apply/restore inline assignment helper tests ----------------------

    #[test]
    fn apply_inline_assignments_sets_and_exports_left_to_right() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/home/test".to_string());
        let assigns = vec![
            bare_assign("A", lit_word("1")),
            bare_assign("B", Word(vec![WordPart::Var { name: "A".to_string(), quoted: false }])),
        ];
        let snap = { let mut sink = StdoutSink::Terminal; let mut err_sink = StderrSink::Terminal; apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink) }.expect("ok");
        assert_eq!(shell.get("A"), Some("1"));
        assert_eq!(shell.get("B"), Some("1"));
        assert!(shell.is_exported("A"));
        assert!(shell.is_exported("B"));
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn restore_inline_assignments_restores_prior_unset_state() {
        let mut shell = Shell::new();
        let assigns = vec![bare_assign("FOO", lit_word("bar"))];
        let snap = { let mut sink = StdoutSink::Terminal; let mut err_sink = StderrSink::Terminal; apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink) }.expect("ok");
        assert_eq!(shell.get("FOO"), Some("bar"));
        restore_inline_assignments(snap, &mut shell);
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn restore_inline_assignments_restores_prior_value_unexported() {
        let mut shell = Shell::new();
        shell.set("FOO", "outer".to_string());
        assert!(!shell.is_exported("FOO"));
        let assigns = vec![bare_assign("FOO", lit_word("inner"))];
        let snap = { let mut sink = StdoutSink::Terminal; let mut err_sink = StderrSink::Terminal; apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink) }.expect("ok");
        assert_eq!(shell.get("FOO"), Some("inner"));
        assert!(shell.is_exported("FOO"));
        restore_inline_assignments(snap, &mut shell);
        assert_eq!(shell.get("FOO"), Some("outer"));
        assert!(!shell.is_exported("FOO"));
    }

    #[test]
    fn restore_inline_assignments_restores_prior_value_exported() {
        let mut shell = Shell::new();
        shell.export_set("FOO", "outer".to_string());
        let assigns = vec![bare_assign("FOO", lit_word("inner"))];
        let snap = { let mut sink = StdoutSink::Terminal; let mut err_sink = StderrSink::Terminal; apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink) }.expect("ok");
        restore_inline_assignments(snap, &mut shell);
        assert_eq!(shell.get("FOO"), Some("outer"));
        assert!(shell.is_exported("FOO"));
    }

    #[test]
    fn restore_inline_assignments_handles_repeated_name() {
        let mut shell = Shell::new();
        shell.set("FOO", "outer".to_string());
        let assigns = vec![
            bare_assign("FOO", lit_word("a")),
            bare_assign("FOO", lit_word("b")),
        ];
        let snap = { let mut sink = StdoutSink::Terminal; let mut err_sink = StderrSink::Terminal; apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink) }.expect("ok");
        assert_eq!(shell.get("FOO"), Some("b"));
        restore_inline_assignments(snap, &mut shell);
        assert_eq!(shell.get("FOO"), Some("outer"));
        assert!(!shell.is_exported("FOO"));
    }

    // ----- run_exec_single inline assignment integration tests ---------------

    #[test]
    fn run_exec_single_external_command_inline_assignment_restores_after() {
        let mut shell = Shell::new();
        shell.set("FOO", "outer".to_string());
        let cmd = SimpleCommand::Exec(ExecCommand {
            inline_assignments: vec![bare_assign("FOO", lit_word("inner"))],
            program: lit_word("true"),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=inner true");
        assert_eq!(shell.get("FOO"), Some("outer"));
        assert!(!shell.is_exported("FOO"));
    }

    #[test]
    fn run_exec_single_function_call_inline_assignment_does_not_persist() {
        let mut shell = Shell::new();
        // Define a no-op function via the parser.
        if let Some(tokens) = crate::lexer::tokenize("myfunc() { echo ok; }").ok()
            && let Ok(Some(seq)) = crate::command::parse(tokens)
        {
            let _ = execute(&seq, &mut shell, "myfunc() { echo ok; }");
        }
        let cmd = SimpleCommand::Exec(ExecCommand {
            inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
            program: lit_word("myfunc"),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=val myfunc");
        // bash: a prefix assignment does NOT persist across a function call.
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn prefix_assign_restores_prior_value_over_function_global_mutation() {
        // Function's own global write to the same var is clobbered by the restore.
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ v=99; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_restores_prior_value_over_function_local() {
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ local v=99; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_restores_unset_over_function_unset() {
        // Function unsets the var; restore reinstates the prior value.
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ unset v; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_with_no_prior_var_is_unset_after_function() {
        let mut shell = Shell::new();
        exec_script("f(){ :; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), None);
    }

    #[test]
    fn posix_special_persist_survives_enclosing_prefix() {
        // func3.sub line 155: outer prefix restore must NOT clobber the inner
        // posix special-builtin persist.
        let mut shell = Shell::new();
        exec_script(
            "set -o posix\nvar=0\nf(){ var=20 return 5; }\nvar=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("var"), Some("20"));
        assert!(shell.inline_scopes.is_empty(), "scope stack balanced");
    }

    #[test]
    fn export_under_enclosing_prefix_does_not_survive_restore_in_default_mode() {
        // export is persistent (absorbs its named var), but in DEFAULT mode that
        // persist must NOT propagate through an enclosing same-name prefix-restore.
        // bash 5.2.21: FOO=30 f restores FOO to 0 even though f did `FOO=20 export FOO`.
        let mut shell = Shell::new();
        exec_script(
            "FOO=0\nf(){ FOO=20 export FOO; }\nFOO=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("FOO"), Some("0"));
        assert!(shell.inline_scopes.is_empty());
    }

    #[test]
    fn export_under_enclosing_prefix_survives_in_posix_mode() {
        let mut shell = Shell::new();
        exec_script(
            "set -o posix\nFOO=0\nf(){ FOO=20 export FOO; }\nFOO=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("FOO"), Some("20"));
        assert!(shell.inline_scopes.is_empty());
    }

    #[test]
    fn exec_with_redirect_does_not_leak_inline_scope() {
        // exec returns via its own early path; the scope-stack push must sit
        // below the exec block so exec never pushes (else it leaks an entry).
        let mut shell = Shell::new();
        exec_script("FOO=bar exec 3>&1\n", &mut shell);
        assert!(shell.inline_scopes.is_empty(), "exec must not leak an inline scope");
    }

    #[test]
    fn default_special_persist_does_not_survive_enclosing_prefix() {
        let mut shell = Shell::new();
        exec_script(
            "var=0\nf(){ var=20 return 5; }\nvar=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("var"), Some("0"));
        assert!(shell.inline_scopes.is_empty());
    }

    #[test]
    fn posix_special_persist_survives_multi_level_enclosing() {
        let mut shell = Shell::new();
        exec_script(
            "set -o posix\na=0\nm(){ a=3 return; }\no(){ a=2 m; }\na=1 o\n",
            &mut shell,
        );
        assert_eq!(shell.get("a"), Some("3"));
        assert!(shell.inline_scopes.is_empty());
    }

    #[test]
    fn run_exec_single_special_builtin_inline_assignment_persists() {
        let mut shell = Shell::new();
        let cmd = SimpleCommand::Exec(ExecCommand {
            inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
            program: lit_word("export"),
            args: vec![lit_word("FOO")],
            redirects: Vec::new(),
            line: 0,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=val export FOO");
        assert_eq!(shell.get("FOO"), Some("val"));
        assert!(shell.is_exported("FOO"));
    }

    #[test]
    fn special_builtin_prefix_does_not_persist_in_default_mode() {
        // `:` is a special builtin; in DEFAULT mode the prefix is temporary.
        let mut shell = Shell::new();
        exec_script("var=0\nvar=20 :\n", &mut shell);
        assert_eq!(shell.get("var"), Some("0"), "default mode restores the prefix");
    }

    #[test]
    fn special_builtin_prefix_persists_in_posix_mode() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nvar=0\nvar=20 :\n", &mut shell);
        assert_eq!(shell.get("var"), Some("20"), "posix mode persists the prefix");
    }

    #[test]
    fn export_prefix_persists_in_default_mode() {
        // export/readonly absorb their named var even in default mode (regression
        // guard alongside run_exec_single_special_builtin_inline_assignment_persists).
        let mut shell = Shell::new();
        exec_script("FOO=val export FOO\n", &mut shell);
        assert_eq!(shell.get("FOO"), Some("val"), "export keeps its named var");
    }

    /// Canonical "fork_and_run_in_subshell works" test.
    ///
    /// Pattern for future helpers: create a libc::pipe pair, fork via the
    /// helper with stdout = pipe.write, in the parent read pipe.read and
    /// waitpid the child, assert the buffer contains the expected output.
    #[test]
    fn fork_and_run_in_subshell_echo_stage_writes_to_pipe() {
        // 1. Create a pipe pair.
        let mut pipe_fds: [libc::c_int; 2] = [-1; 2];
        let rc = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
        assert_eq!(rc, 0, "libc::pipe failed");
        let (read_fd, write_fd) = (pipe_fds[0], pipe_fds[1]);

        // 2. Build `echo hi-from-subshell` as a Command.
        let cmd = Command::Simple(exec("echo", &["hi-from-subshell"]));

        // 3. Fork: child writes to write_fd; parent keeps read_fd.
        //    pass write_fd in parent_fds_to_close so the child closes its
        //    own copy (it dup2'd it to fd 1, so the original > 2 copy is dead).
        let mut shell = Shell::new();
        let child_pid = fork_and_run_in_subshell(
            &cmd,
            &mut shell,
            libc::STDIN_FILENO,  // stdin = terminal
            write_fd,            // stdout → pipe write end
            libc::STDERR_FILENO, // stderr = terminal
            0,                   // pgid_target: become own pgrp leader
            &[read_fd],          // close the read end in the child
            None,                // no Dup redirect
            None,
        )
        .expect("fork_and_run_in_subshell failed");

        // 4. Parent: close the write end so reading will eventually see EOF.
        unsafe { libc::close(write_fd) };

        // 5. Read from the pipe into a buffer.
        let mut buf = vec![0u8; 256];
        let mut total = 0usize;
        loop {
            let n = unsafe {
                libc::read(read_fd, buf.as_mut_ptr().add(total).cast(), buf.len() - total)
            };
            if n <= 0 {
                break;
            }
            total += n as usize;
        }
        unsafe { libc::close(read_fd) };
        let output = std::str::from_utf8(&buf[..total]).expect("utf8").to_string();

        // 6. Reap the child.
        let mut raw_status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(child_pid, &mut raw_status, 0) };
        assert_eq!(r, child_pid, "waitpid returned unexpected pid");
        assert!(libc::WIFEXITED(raw_status), "child did not exit normally");
        let exit_code = libc::WEXITSTATUS(raw_status);
        assert_eq!(exit_code, 0);

        // 7. Check output.
        assert!(
            output.contains("hi-from-subshell"),
            "expected 'hi-from-subshell' in pipe output, got: {output:?}"
        );
    }

    // ----- external-process stderr capture / Merged --------------------------

    /// `/bin/sh -c 'echo out; echo err >&2'` with split capture sinks:
    /// stdout lands in `buf_out`, stderr lands in `buf_err`. Exercises the
    /// `run_subprocess` Capture-stderr branch (Stdio::piped on fd 2 + threaded
    /// drain). Bash-equivalent: `bash -c '...' 1>out 2>err`.
    #[test]
    #[cfg(unix)]
    fn external_process_stderr_is_captured() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        {
            let mut out = StdoutSink::Capture(&mut buf_out);
            let mut err = StderrSink::Capture(&mut buf_err);
            let src = "/bin/sh -c 'echo out; echo err >&2'";
            let tokens = crate::lexer::tokenize(src).expect("lex");
            let seq = crate::command::parse(tokens).expect("parse").expect("seq");
            execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
        }
        assert_eq!(String::from_utf8_lossy(&buf_out), "out\n");
        assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
    }

    /// `/bin/sh -c 'printf out; printf err 1>&2; printf out2'` with
    /// `StderrSink::Merged` routes fd 2 onto fd 1 (the capture pipe) in the
    /// child via a `pre_exec` dup2(1,2). Both streams hit the same kernel pipe;
    /// kernel-level ordering matches the source-code writes.
    /// Bash-equivalent: `bash -c '...' 2>&1`.
    #[test]
    #[cfg(unix)]
    fn external_process_merged_stderr_interleaves_via_kernel() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        {
            let mut out = StdoutSink::Capture(&mut buf);
            let mut err = StderrSink::Merged;
            let src = "/bin/sh -c 'printf out; printf err 1>&2; printf out2'";
            let tokens = crate::lexer::tokenize(src).expect("lex");
            let seq = crate::command::parse(tokens).expect("parse").expect("seq");
            execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
        }
        assert_eq!(String::from_utf8_lossy(&buf), "outerrout2");
    }

    /// Multi-stage pipeline with a stage writing to stderr — the shared
    /// `StderrSink::Capture` pipe (per-stage dup'd write-end) should collect
    /// every stage's stderr into the same buffer. Bash-equivalent:
    /// `bash -c 'echo a; echo err >&2 | cat' 1>out 2>err` (rough analog).
    #[test]
    #[cfg(unix)]
    fn pipeline_stage_stderr_is_captured() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        {
            let mut out = StdoutSink::Capture(&mut buf_out);
            let mut err = StderrSink::Capture(&mut buf_err);
            // First stage prints to stderr (visible in err buf), pipes nothing.
            // Second stage `cat` reads (empty) and writes nothing → stdout empty.
            let src = "/bin/sh -c 'echo err >&2' | cat";
            let tokens = crate::lexer::tokenize(src).expect("lex");
            let seq = crate::command::parse(tokens).expect("parse").expect("seq");
            execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
        }
        assert_eq!(String::from_utf8_lossy(&buf_out), "");
        assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
    }

    /// `( echo out; echo err >&2 )` — a Subshell command, not an external. The
    /// subshell branch of `run_command` forks via `fork_and_run_in_subshell`;
    /// this test exercises the per-fork-site stderr pipe + threaded drain that
    /// mirrors the stdout-capture pipe pattern. Bash-equivalent: `( … ) 1>out 2>err`.
    #[test]
    #[cfg(unix)]
    fn subshell_stderr_is_captured() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        {
            let mut out = StdoutSink::Capture(&mut buf_out);
            let mut err = StderrSink::Capture(&mut buf_err);
            let src = "( echo out; echo err >&2 )";
            let tokens = crate::lexer::tokenize(src).expect("lex");
            let seq = crate::command::parse(tokens).expect("parse").expect("seq");
            execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
        }
        assert_eq!(String::from_utf8_lossy(&buf_out), "out\n");
        assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
    }

    // ----- classify_stage unit tests (Task 4) ----------------------------------

    /// Helper: builds `Command::Simple(SimpleCommand::Exec(...))` for `program`.
    fn simple_exec_cmd(program: &str) -> Command {
        Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: lit_word(program),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        }))
    }

    /// Helper: builds `Command::Simple(SimpleCommand::Exec(...))` with a
    /// dynamic (Var) program word — simulates `$cmd args`.
    fn dynamic_exec_cmd() -> Command {
        use crate::lexer::WordPart;
        Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![WordPart::Var { name: "cmd".to_string(), quoted: false }]),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        }))
    }

    #[test]
    fn classify_stage_external_for_unknown_command() {
        // `cat` is not a builtin and not in functions → External.
        let shell = Shell::new();
        let cmd = simple_exec_cmd("cat");
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::External(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_builtin() {
        // `cd` is a builtin → InProcess.
        let shell = Shell::new();
        let cmd = simple_exec_cmd("cd");
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_echo_builtin() {
        // `echo` is a builtin → InProcess.
        let shell = Shell::new();
        let cmd = simple_exec_cmd("echo");
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_function() {
        // A function named `myfunc` exists in shell.functions → InProcess.
        let mut shell = Shell::new();
        // Register myfunc in the function table via the parser.
        if let Ok(tokens) = crate::lexer::tokenize("myfunc() { :; }")
            && let Ok(Some(seq)) = crate::command::parse(tokens)
        {
            let _ = execute(&seq, &mut shell, "myfunc() { :; }");
        }
        let cmd = simple_exec_cmd("myfunc");
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_compound_if() {
        // An `if` clause is never External.
        use crate::command::IfClause;
        let shell = Shell::new();
        let cmd = Command::If(Box::new(IfClause {
            condition: cond_seq(true),
            then_body: echo_seq("yes"),
            elif_branches: vec![],
            else_body: None,
        }));
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_assign_only_stage() {
        // Assignment-only stage (SimpleCommand::Assign) → InProcess.
        let shell = Shell::new();
        let cmd = Command::Simple(SimpleCommand::Assign(vec![
            bare_assign("FOO", lit_word("bar")),
        ], 0));
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    #[test]
    fn classify_stage_inprocess_for_dynamic_program() {
        // `$cmd args` — program word is a Var → static text resolution fails → InProcess.
        let shell = Shell::new();
        let cmd = dynamic_exec_cmd();
        assert!(matches!(classify_stage(&cmd, &shell), StageKind::InProcess(_)));
    }

    // ----- resolve_fd_target unit tests (Task 2 / v29) -------------------------

    #[test]
    fn resolve_fd_target_parses_literal_number() {
        let mut shell = Shell::new();
        let word = lit_word("1");
        assert_eq!(resolve_fd_target(&word, &mut shell).unwrap(), 1);
    }

    #[test]
    fn resolve_fd_target_rejects_non_numeric() {
        let mut shell = Shell::new();
        let word = lit_word("notanumber");
        assert!(resolve_fd_target(&word, &mut shell).is_err());
    }

    // ----- program_static_text unit tests (Task 4) ----------------------------

    #[test]
    fn program_static_text_returns_some_for_plain_literal() {
        use crate::command::ExecCommand;
        let exec = ExecCommand {
            inline_assignments: Vec::new(),
            program: lit_word("cat"),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        };
        assert_eq!(exec.program_static_text(), Some("cat".to_string()));
    }

    #[test]
    fn program_static_text_returns_none_for_quoted_literal() {
        use crate::command::ExecCommand;
        use crate::lexer::WordPart;
        let exec = ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![WordPart::Literal { text: "cat".to_string(), quoted: true }]),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        };
        // Quoted literal → None (could be a function or builtin masked by quoting).
        assert_eq!(exec.program_static_text(), None);
    }

    #[test]
    fn program_static_text_returns_none_for_var_word() {
        use crate::command::ExecCommand;
        use crate::lexer::WordPart;
        let exec = ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![WordPart::Var { name: "cmd".to_string(), quoted: false }]),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        };
        assert_eq!(exec.program_static_text(), None);
    }

    #[test]
    fn program_static_text_returns_none_for_multi_part_word() {
        use crate::command::ExecCommand;
        use crate::lexer::WordPart;
        // Two parts: e.g. `cat` + some suffix (weird, but defensive).
        let exec = ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![
                WordPart::Literal { text: "ca".to_string(), quoted: false },
                WordPart::Literal { text: "t".to_string(), quoted: false },
            ]),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        };
        assert_eq!(exec.program_static_text(), None);
    }

    // --- v26 special parameters: executor wiring ---

    /// Helper: parse and execute a complete multi-statement script by
    /// accumulating lines and executing each parseable sequence in turn,
    /// mirroring how the interactive REPL processes input.
    fn exec_script(src: &str, shell: &mut Shell) {
        // The shell's normal execution reads one token stream at a time from
        // the parser. We can simulate this by iterating over lines and
        // accumulating until we have a parseable sequence.
        let mut buf = String::new();
        for line in src.lines() {
            buf.push_str(line);
            buf.push('\n');
            let tokens = match crate::lexer::tokenize(&buf) {
                Ok(t) if !t.is_empty() => t,
                _ => continue,
            };
            match crate::command::parse(tokens) {
                Ok(Some(seq)) => {
                    let outcome = execute(&seq, shell, &buf);
                    buf.clear();
                    if matches!(outcome, ExecOutcome::Exit(_)) {
                        return;
                    }
                }
                Ok(None) => {
                    buf.clear();
                }
                Err(_) => {
                    // Incomplete parse — keep accumulating.
                    continue;
                }
            }
        }
        // Execute any remaining buffered content.
        if !buf.is_empty()
            && let Ok(tokens) = crate::lexer::tokenize(&buf)
            && let Ok(Some(seq)) = crate::command::parse(tokens)
        {
            let _ = execute(&seq, shell, &buf);
        }
    }

    #[test]
    fn posix_source_not_found_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\n. /no/such/huck_file_xyz\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(1));
    }
    #[test]
    fn default_source_not_found_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script(". /no/such/huck_file_xyz\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
    #[test]
    fn posix_function_named_special_builtin_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\neval() { :; }\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(2));
        assert!(!shell.functions.contains_key("eval"), "function not defined");
    }
    #[test]
    fn default_function_named_special_builtin_is_allowed() {
        let mut shell = Shell::new();
        exec_script("eval() { :; }\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
        assert!(shell.functions.contains_key("eval"));
    }
    #[test]
    fn posix_readonly_for_var_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly i=1\nfor i in a b; do :; done\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn default_readonly_for_var_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script("readonly i=1\nfor i in a b; do :; done\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
    #[test]
    fn posix_assignment_no_command_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn posix_assignment_before_special_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2 export y\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn posix_assignment_before_regular_is_not_fatal() {
        // before a REGULAR command → abort-continue (deferred), NOT a shell exit.
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2 true\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
    #[test]
    fn default_assignment_no_command_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script("readonly x=1\nx=2\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }

    // ----- Case #1: special-builtin usage / assignment errors are posix-fatal --
    fn posix_run(src: &str) -> Option<i32> {
        let mut shell = Shell::new();
        exec_script(&format!("set -o posix\n{src}\n"), &mut shell);
        shell.pending_fatal_status
    }
    #[test]
    fn posix_special_builtin_usage_errors_exit() {
        assert_eq!(posix_run("set -o nosuchopt"), Some(2), "set bad option");
        assert_eq!(posix_run("unset -z"), Some(2), "unset bad option");
        assert_eq!(posix_run("export -z"), Some(2), "export bad option");
        assert_eq!(posix_run("export AA[4]=1"), Some(1), "export bad assignment");
        assert_eq!(posix_run("readonly AA[4]=1"), Some(1), "readonly bad assignment");
        assert_eq!(posix_run("return 2"), Some(2), "return outside function");
        assert_eq!(posix_run("exec -z"), Some(2), "exec bad option");
    }
    #[test]
    fn posix_set_unimplemented_option_does_not_exit() {
        // Valid-in-bash options huck hasn't implemented must NOT exit a posix shell.
        assert_eq!(posix_run("set -o emacs"), None, "set -o emacs");
        assert_eq!(posix_run("set -o vi"), None, "set -o vi");
        assert_eq!(posix_run("set -h"), None, "set -h single-char");
    }
    #[test]
    fn posix_set_invalid_option_name_exits() {
        assert_eq!(posix_run("set -o nosuchopt"), Some(2), "genuinely invalid -o name");
    }
    #[test]
    fn posix_special_builtin_runtime_errors_do_not_exit() {
        assert_eq!(posix_run("shift 99"), None, "shift out of range");
        assert_eq!(posix_run("shift -z"), None, "shift bad option");
        assert_eq!(posix_run("break"), None, "break outside loop");
        assert_eq!(posix_run("unset RO; readonly RO=1; unset RO"), None, "unset readonly var");
        assert_eq!(posix_run("eval false"), None, "eval propagates child status");
        assert_eq!(posix_run("f(){ return 2; }; f"), None, "legit return 2");
        assert_eq!(posix_run("trap x NOSUCHSIG"), None, "trap bad signal");
        assert_eq!(posix_run("export \"AA[4]\""), None, "export bad name no =");
    }
    #[test]
    fn posix_command_builtin_wrappers_strip_fatal() {
        assert_eq!(posix_run("command set -o bad"), None, "command strips");
        assert_eq!(posix_run("builtin set -o bad"), None, "builtin strips");
        assert_eq!(posix_run("command export AA[4]=1"), None, "command strips assignment");
    }

    #[test]
    fn set_o_posix_toggles_shell_option() {
        let mut shell = Shell::new();
        assert!(!shell.shell_options.posix, "posix defaults off");
        exec_script("set -o posix\n", &mut shell);
        assert!(shell.shell_options.posix, "set -o posix turns it on");
        exec_script("set +o posix\n", &mut shell);
        assert!(!shell.shell_options.posix, "set +o posix turns it off");
    }

    #[test]
    fn funcnest_limit_refuses_call_past_depth() {
        let mut shell = Shell::new();
        // FUNCNEST=3 allows depth 1,2,3; the 4th call is refused (rc 1).
        exec_script("FUNCNEST=3\nn=0\nf(){ n=$((n+1)); f; }\nf\n", &mut shell);
        assert_eq!(shell.get("n"), Some("3"), "should stop after depth 3");
        assert_eq!(shell.last_status(), 1, "refused call propagates rc 1");
    }

    #[test]
    fn funcnest_unlimited_allows_bounded_recursion() {
        let mut shell = Shell::new();
        // No FUNCNEST: a bounded 50-deep recursion completes without error.
        exec_script("n=0\nf(){ n=$((n+1)); if (( n >= 50 )); then return 7; fi; f; }\nf\n", &mut shell);
        assert_eq!(shell.get("n"), Some("50"));
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn call_function_keeps_arg0_during_body() {
        // bash: `$0` is NOT rebound to the function name on entry — it stays the
        // shell/script invocation name throughout the function body.
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        exec_script("myfunc() { CAPTURED=$0; }\nmyfunc\n", &mut shell);
        assert_eq!(shell.get("CAPTURED"), Some("my-shell"));
    }

    #[test]
    fn call_function_pops_arg0_after_return() {
        let mut shell = Shell::new();
        exec_script("myfunc() { :; }\nmyfunc\n", &mut shell);
        assert!(shell.call_stack.is_empty(),
            "call_stack should be empty after function returns, got: {:?}",
            shell.call_stack);
    }

    #[test]
    fn function_with_local_does_not_leak_var() {
        let mut shell = Shell::new();
        exec_script("f() { local XYZ_LOCAL_E1=in; }\nf\n", &mut shell);
        assert!(shell.lookup_var("XYZ_LOCAL_E1").is_none());
    }

    #[test]
    fn function_local_restores_outer_var() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_E2", "outer".to_string());
        exec_script("f() { local XYZ_LOCAL_E2=inner; }\nf\n", &mut shell);
        assert_eq!(shell.lookup_var("XYZ_LOCAL_E2").as_deref(), Some("outer"));
    }

    #[test]
    fn nested_function_calls_have_isolated_locals() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_E3", "top".to_string());
        let script = "outer() { local XYZ_LOCAL_E3=outer_val; inner; }\n\
                      inner() { local XYZ_LOCAL_E3=inner_val; }\n\
                      outer\n";
        exec_script(script, &mut shell);
        // After both functions return, the outer `top` value is restored.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_E3").as_deref(), Some("top"));
    }

    #[test]
    fn run_background_sequence_sets_last_bg_pid() {
        // Background an external command and check that last_bg_pid is set.
        let mut shell = Shell::new();
        exec_script("/usr/bin/true &\n", &mut shell);
        assert!(shell.last_bg_pid.is_some(), "last_bg_pid should be set after background command");
        // Reap the child to avoid zombies.
        if let Some(pid) = shell.last_bg_pid {
            let mut status: libc::c_int = 0;
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG); }
        }
    }

    #[test]
    fn execute_bg_chain_returns_immediately_status_0() {
        // `true && true &` — parent should return Continue(0) without
        // waiting for the child.
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        let toks = crate::lexer::tokenize("true && true &").unwrap();
        let seq = crate::command::parse(toks).unwrap().unwrap();
        let outcome = execute(&seq, &mut shell, "true && true &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Cleanup: SIGTERM any bg job so the test doesn't leak.
        for job in shell.jobs.iter() {
            unsafe { libc::kill(job.pgid, libc::SIGTERM); }
        }
    }

    #[test]
    fn execute_bg_chain_registers_job() {
        // After `sleep 30 && true &`, the bg sequence should register
        // as one job entry. The sleep ensures the child is alive long
        // enough to observe.
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        let toks = crate::lexer::tokenize("sleep 30 && true &").unwrap();
        let seq = crate::command::parse(toks).unwrap().unwrap();
        let _ = execute(&seq, &mut shell, "sleep 30 && true &");
        assert_eq!(shell.jobs.iter().count(), 1, "expected exactly one job");
        // Cleanup.
        for job in shell.jobs.iter() {
            unsafe { libc::kill(job.pgid, libc::SIGTERM); }
        }
    }

    // ----- v54: readonly enforcement at executor-layer write paths ----------

    #[test]
    fn top_level_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script("X=new\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn inline_assignment_to_readonly_aborts_command() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        // Inline `X=new echo hi` — bash aborts the command. Use a
        // builtin (echo) to keep the assertion deterministic.
        exec_script("X=new echo hi\n", &mut shell);
        // X is still its original value (not changed by the failed
        // inline). The echo should NOT have run. Status is 1.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn for_loop_iter_var_readonly_aborts_at_first_iter() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script(
            "for X in a b c; do echo got=$X; done\n",
            &mut shell,
        );
        // X unchanged; status 1; body should not have executed.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn param_expansion_default_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "".to_string());
        shell.mark_readonly("X");
        // `: ${X:=hello}` — colon command + AssignDefault that
        // tries to write hello to readonly X.
        exec_script(": ${X:=hello}\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn arith_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "0".to_string());
        shell.mark_readonly("X");
        // The arith expansion machinery in expand.rs maps any
        // ArithError to "huck: arithmetic: <msg>" + set_last_status(1)
        // with empty substitution; the surrounding command may then
        // overwrite the status (echo returns 0). The load-bearing
        // assertion is that the readonly X was NOT clobbered.
        exec_script("echo $((X=5))\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some("0"));
    }

    #[test]
    fn local_readonly_in_function_errors() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script(
            "f() { local X=inner; }\nf\n",
            &mut shell,
        );
        // local should have errored; X unchanged.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    /// Smoke-test for `with_redirect_scope` via `run_redirected`: a brace
    /// group redirected to a file writes its output there (not to stdout).
    ///
    /// Cross-process FD-1 race: while this test has FD 1 dup2'd to the
    /// target file, `cargo test`'s libtest runner may print sibling test
    /// progress lines (`"test foo ... ok\n"`) to the same FD 1 from a
    /// peer thread, and those land in our file too. We can't serialize
    /// against libtest (it doesn't take our lock) and we can't redirect
    /// libtest's writes (they go to the inherited real FD 1). So the
    /// assertion verifies the actual claim — that the redirected
    /// `echo HI` output is present as an exact line — and tolerates any
    /// libtest noise that may have leaked in alongside it. (In real
    /// shell use no other thread writes to FD 1 during the redirect
    /// window; the noise is a `cargo test` artifact only.)
    #[test]
    fn compound_stdout_redirect_writes_to_file() {
        let dir = std::env::temp_dir()
            .join(format!("huck_redir_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("out.txt");
        let _ = std::fs::remove_file(&p);

        let mut shell = Shell::new();
        exec_script(
            &format!("{{ echo HI; }} > {}\n", p.display()),
            &mut shell,
        );

        let content = std::fs::read_to_string(&p)
            .expect("redirect target file should exist");
        assert!(
            content.lines().any(|l| l == "HI"),
            "redirected `echo HI` should appear as a line in the file, got {content:?}",
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn make_orphan_pipe_for_eof_reader_yields_immediate_eof() {
        use std::io::Read;
        use std::os::unix::io::FromRawFd;
        let r = make_orphan_pipe_for_eof_reader().expect("pipe");
        // Read should return 0 bytes (EOF) immediately, not block.
        let mut f = unsafe { std::fs::File::from_raw_fd(r) };
        let mut buf = [0u8; 8];
        let n = f.read(&mut buf).expect("read");
        assert_eq!(n, 0, "expected EOF, got {n} bytes");
    }
}

#[cfg(test)]
mod array_assign_tests {
    use super::*;
    use crate::shell_state::Shell;

    /// Drive a fragment through the same execute path the REPL uses.
    fn run_line(shell: &mut Shell, line: &str) {
        let mut src = String::from(line);
        if !src.ends_with('\n') {
            src.push('\n');
        }
        let tokens = crate::lexer::tokenize(&src).expect("tokenize");
        let seq = crate::command::parse(tokens)
            .expect("parse ok")
            .expect("non-empty parse");
        execute(&seq, shell, &src);
    }

    #[test]
    fn compound_assign_creates_array() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        let m = s.get_indexed("a").expect("a should be an array");
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn sparse_compound_assign_respects_explicit_subscripts() {
        let mut s = Shell::new();
        run_line(&mut s, "a=([5]=x [2]=y)");
        let m = s.get_indexed("a").expect("a should be an array");
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&5).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("y"));
    }

    #[test]
    fn element_assign_creates_array() {
        let mut s = Shell::new();
        run_line(&mut s, "a[3]=hello");
        let m = s.get_indexed("a").expect("a should be an array");
        assert_eq!(m.get(&3).map(String::as_str), Some("hello"));
    }

    #[test]
    fn element_assign_promotes_scalar() {
        let mut s = Shell::new();
        run_line(&mut s, "a=old");
        run_line(&mut s, "a[2]=new");
        let m = s.get_indexed("a").expect("scalar should promote to array");
        assert_eq!(m.get(&0).map(String::as_str), Some("old"));
        assert_eq!(m.get(&2).map(String::as_str), Some("new"));
    }

    #[test]
    fn append_array_extends() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y)");
        run_line(&mut s, "a+=(z w)");
        let m = s.get_indexed("a").unwrap();
        assert_eq!(
            m.values().cloned().collect::<Vec<_>>(),
            vec![
                "x".to_string(),
                "y".to_string(),
                "z".to_string(),
                "w".to_string()
            ]
        );
    }

    #[test]
    fn append_element_concatenates() {
        let mut s = Shell::new();
        run_line(&mut s, "a[0]=hello");
        run_line(&mut s, "a[0]+=_world");
        let m = s.get_indexed("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("hello_world"));
    }

    #[test]
    fn readonly_blocks_compound_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a=(changed)");
        let m = s.get_indexed("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("initial"));
    }

    #[test]
    fn readonly_blocks_element_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a[5]=new");
        let m = s.get_indexed("a").unwrap();
        assert!(m.get(&5).is_none());
    }

    #[test]
    fn unset_element_removes_one_key() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a[1]");
        let m = s.get_indexed("a").unwrap();
        assert!(m.get(&1).is_none());
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn unset_whole_array_removes_variable() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a");
        assert!(s.get_indexed("a").is_none());
        assert!(s.get("a").is_none());
    }

    #[test]
    fn scalar_append_to_existing_array_writes_element_zero() {
        // `a=(x y); a+=z` in bash appends to element 0 (i.e. concatenates
        // with a[0]), yielding a[0]="xz".
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y)");
        run_line(&mut s, "a+=z");
        let m = s.get_indexed("a").expect("still an array");
        assert_eq!(m.get(&0).map(String::as_str), Some("xz"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    }

    #[test]
    fn indexed_lvalue_compound_rhs_rejected() {
        // `a[i]=(...)` is a syntax-level error in bash; huck rejects
        // it with a diagnostic and leaves `a` empty.
        let mut s = Shell::new();
        run_line(&mut s, "a[0]=(x y)");
        assert!(s.get_indexed("a").is_none());
    }

    #[test]
    fn unset_with_empty_subscript_errors() {
        // bash treats `unset a[]` as a syntax error
        // ("bad array subscript") and leaves `a` untouched.
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a[]");
        let m = s.get_indexed("a").expect("a should still exist");
        assert_eq!(m.len(), 3);
    }
}

#[cfg(test)]
mod assoc_assign_tests {
    use crate::shell_state::Shell;

    fn run(shell: &mut Shell, line: &str) {
        crate::shell::process_line(line, shell, false);
    }

    #[test]
    fn element_assign_on_declared_associative_uses_string_key() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[foo]=bar");
        assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
    }

    #[test]
    fn element_assign_without_declare_creates_indexed() {
        // Bash gotcha: `m[foo]=v` on unset `m` creates indexed (foo→0).
        let mut s = Shell::new();
        run(&mut s, "m[foo]=bar");
        assert!(s.get_indexed("m").is_some());
        assert!(s.get_associative("m").is_none());
        assert_eq!(s.lookup_indexed_element("m", 0), Some("bar".into()));
    }

    #[test]
    fn compound_literal_on_associative_uses_keys() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m=([a]=1 [b]=2)");
        assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
        assert_eq!(s.lookup_associative_element("m", "b"), Some("2".into()));
    }

    #[test]
    fn append_compound_on_associative_merges() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m=([a]=1 [b]=2)");
        run(&mut s, "m+=([c]=3 [a]=99)");
        let pairs = s.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 3);
        assert_eq!(s.lookup_associative_element("m", "a"), Some("99".into()));
        assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
    }

    #[test]
    fn append_element_on_associative_concatenates() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[k]=hello");
        run(&mut s, "m[k]+=_world");
        assert_eq!(
            s.lookup_associative_element("m", "k"),
            Some("hello_world".into())
        );
    }

    #[test]
    fn positional_literal_on_associative_rejects() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "preexisting".into(), "x".into())
            .unwrap();
        run(&mut s, "m=(a b c)");
        // associative `m` should be unchanged; positional literal is rejected.
        assert_eq!(
            s.lookup_associative_element("m", "preexisting"),
            Some("x".into())
        );
    }

    #[test]
    fn scalar_rhs_on_associative_rejects() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k".into(), "v".into())
            .unwrap();
        run(&mut s, "m=newscalar");
        // associative `m` should be unchanged.
        assert_eq!(s.lookup_associative_element("m", "k"), Some("v".into()));
    }

    #[test]
    fn unset_associative_element_removes_one_key() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[a]=1");
        run(&mut s, "m[b]=2");
        run(&mut s, "m[c]=3");
        run(&mut s, "unset m[b]");
        let pairs = s.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 2);
        assert!(s.lookup_associative_element("m", "b").is_none());
        assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
        assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
    }

    #[test]
    fn unset_whole_associative_removes_variable() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[a]=1");
        run(&mut s, "unset m");
        assert!(s.get_associative("m").is_none());
        assert!(s.get("m").is_none());
    }

    #[test]
    fn readonly_blocks_element_write_on_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "a".into(), "1".into())
            .unwrap();
        s.mark_readonly("m");
        run(&mut s, "m[b]=2");
        assert!(s.lookup_associative_element("m", "b").is_none());
    }

    #[test]
    fn unset_name_with_separate_assoc_still_creates_indexed() {
        // The gotcha is name-specific: having declared `foo` as associative
        // should not influence routing for an UNSET `bar`. `bar[baz]=v`
        // should still create indexed `bar[0]=v`.
        let mut s = Shell::new();
        s.declare_associative("foo").unwrap();
        s.set_associative_element("foo", "k".into(), "v".into())
            .unwrap();
        run(&mut s, "bar[baz]=value");
        assert!(s.get_indexed("bar").is_some(), "bar should be indexed");
        assert!(
            s.get_associative("bar").is_none(),
            "bar should NOT be associative"
        );
        // foo should be unaffected.
        assert_eq!(s.lookup_associative_element("foo", "k"), Some("v".into()));
    }
}

#[cfg(test)]
mod arith_for_tests {
    use crate::builtins::ExecOutcome;
    use crate::shell_state::Shell;

    #[test]
    fn arith_command_nonzero_exits_0() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((1+2))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(0)), "got {outcome:?}");
    }

    #[test]
    fn arith_command_zero_exits_1() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((0))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(1)), "got {outcome:?}");
    }

    #[test]
    fn arith_command_division_by_zero_exits_1() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((1/0))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(1)), "got {outcome:?}");
    }

    #[test]
    fn arith_for_counter_loop_sets_var() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<3;i++)) do :; done",
            &mut sh,
            false,
        );
        // After the loop, i should be 3 (the value at which cond failed).
        assert_eq!(sh.lookup_var("i").as_deref(), Some("3"));
    }

    #[test]
    fn arith_for_break_stops_at_value() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<10;i++)) do if [ $i -eq 5 ]; then break; fi; done",
            &mut sh,
            false,
        );
        // i was 5 when break fired; step does NOT run after break.
        assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
    }

    #[test]
    fn arith_for_continue_evaluates_step() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<5;i++)) do continue; done",
            &mut sh,
            false,
        );
        // i should reach 5 (cond fails) — step runs after continue.
        assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
    }
}

#[cfg(test)]
mod loop_levels_executor_tests {
    use crate::shell_state::Shell;

    #[test]
    fn break_in_inner_loop_exits_inner_only() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2; do for j in a b; do if [ \"$j\" = \"b\" ]; then break; fi; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // Outer loop ran both i=1 and i=2 (inner break only exits inner).
        assert_eq!(sh.lookup_var("x").as_deref(), Some("2"), "outer loop should run twice");
        assert_eq!(sh.loop_depth, 0, "loop_depth not restored after nested-for break");
    }

    #[test]
    fn break_2_in_inner_loop_exits_both() {
        let mut sh = Shell::new();
        // Counter to verify outer loop didn't iterate again.
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2; do for j in a b; do break 2; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // x should still be 0 — break 2 exits before x=$((x+1)) runs.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn break_999_caps_in_two_loops() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2; do for j in a b; do break 999; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // Same as break 2 — cap to depth=2.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn continue_2_in_inner_loop_runs_outer_step() {
        let mut sh = Shell::new();
        // continue 2 from inner: skip rest of inner, advance outer
        let _ = crate::shell::process_line(
            "x=0; for i in 1 2 3; do for j in a; do continue 2; done; x=$((x+1)); done",
            &mut sh,
            false,
        );
        // x should be 0 — `continue 2` skips the x=... line each outer iteration.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    }

    #[test]
    fn break_inside_function_called_from_loop_errors() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "f() { break; }; for i in 1 2; do f; done; echo done",
            &mut sh,
            false,
        );
        // The break inside f errors (loop_depth=0 inside the function);
        // for-loop continues; loop_depth is back to 0 afterward.
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_zero_after_loop_exits() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2 3; do :; done",
            &mut sh,
            false,
        );
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_zero_after_nested_loop_exits() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2; do for j in a b; do :; done; done",
            &mut sh,
            false,
        );
        assert_eq!(sh.loop_depth, 0);
    }

    #[test]
    fn loop_depth_restored_after_function_return() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "f() { for j in a b; do :; done; }; for i in 1 2; do f; done",
            &mut sh,
            false,
        );
        // Both outer for-loop (depth +1) and inner function-then-for
        // should leave loop_depth at 0.
        assert_eq!(sh.loop_depth, 0);
    }

    // ----- malformed-arg break/continue: break ALL loops, terminal $? = 1 -----

    #[test]
    fn break_zero_breaks_all_loops_and_status_1() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; o=0; for i in 1 2; do for j in a b; do break 0; x=$((x+1)); done; o=$((o+1)); done",
            &mut sh,
            false,
        );
        // break 0 breaks ALL loops: neither the inner body after it (x) nor the
        // outer body after the inner loop (o) runs again.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"), "inner body must not run after break 0");
        assert_eq!(sh.lookup_var("o").as_deref(), Some("0"), "outer body must not run after break 0");
        // The loop nest leaves $? = 1.
        assert_eq!(sh.last_status(), 1, "break 0 leaves $? = 1");
    }

    #[test]
    fn continue_zero_breaks_all_loops_and_status_1() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; o=0; for i in 1 2; do for j in a b; do continue 0; x=$((x+1)); done; o=$((o+1)); done",
            &mut sh,
            false,
        );
        // continue 0 behaves like break-all (out-of-range), same as bash.
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
        assert_eq!(sh.lookup_var("o").as_deref(), Some("0"));
        assert_eq!(sh.last_status(), 1, "continue 0 leaves $? = 1");
    }

    #[test]
    fn break_too_many_args_breaks_all_loops_and_status_1() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "x=0; o=0; for i in 1 2; do for j in a b; do break 1 2 3; x=$((x+1)); done; o=$((o+1)); done",
            &mut sh,
            false,
        );
        assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
        assert_eq!(sh.lookup_var("o").as_deref(), Some("0"));
        assert_eq!(sh.last_status(), 1, "break with too many args leaves $? = 1");
    }

    #[test]
    fn normal_break_leaves_status_0() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for i in 1 2; do break; done",
            &mut sh,
            false,
        );
        // Normal break leaves $? = 0 (no regression from the status-carrying change).
        assert_eq!(sh.last_status(), 0);
    }
}

#[cfg(test)]
mod select_menu_tests {
    use super::{format_select_menu, number_len, select_indent};

    fn items(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn number_len_digit_counts() {
        assert_eq!(number_len(1), 1);
        assert_eq!(number_len(9), 1);
        assert_eq!(number_len(10), 2);
        assert_eq!(number_len(99), 2);
        assert_eq!(number_len(100), 3);
    }

    #[test]
    fn indent_emits_tab_across_stop_else_space() {
        let mut s = String::new();
        select_indent(&mut s, 6, 11); // crosses the 8-boundary once → tab + 3 spaces
        assert_eq!(s, "\t   ");
        let mut s2 = String::new();
        select_indent(&mut s2, 20, 22); // same tab block → 2 spaces
        assert_eq!(s2, "  ");
        let mut s3 = String::new();
        select_indent(&mut s3, 8, 11); // from is exactly on a tab stop → no tab emitted, 3 spaces
        assert_eq!(s3, "   ");
    }

    #[test]
    fn single_item() {
        assert_eq!(format_select_menu(&items(&["only"]), 80), "1) only\n");
    }

    #[test]
    fn three_items_single_column() {
        // 3 items: max_elem_len=6, cols=80/6=13, rows=ceil(3/13)=1 → flip → 3 rows × 1 col.
        assert_eq!(
            format_select_menu(&items(&["a", "b", "c"]), 80),
            "1) a\n2) b\n3) c\n"
        );
    }

    #[test]
    fn ten_items_cols80_multicolumn() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            80,
        );
        // Verified byte-for-byte against bash 5.2 (COLUMNS=80, cat -A):
        let expected = "1) one\t    3) three   5) five\t  7) seven   9) nine\n\
                        2) two\t    4) four    6) six\t  8) eight  10) ten\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn ten_items_cols40() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            40,
        );
        let expected = "1) one\t    5) five    9) nine\n\
                        2) two\t    6) six    10) ten\n\
                        3) three    7) seven\n\
                        4) four\t    8) eight\n";
        assert_eq!(got, expected);
    }

    #[test]
    fn ten_items_cols110_single_column_flip() {
        let got = format_select_menu(
            &items(&["one", "two", "three", "four", "five",
                     "six", "seven", "eight", "nine", "ten"]),
            110,
        );
        // Wide COLS → rows==1 flip → single column, numbers right-justified to 2.
        // (Verified byte-for-byte against bash 5.2 COLUMNS=110 via cat -A.)
        let expected = concat!(
            " 1) one\n",
            " 2) two\n",
            " 3) three\n",
            " 4) four\n",
            " 5) five\n",
            " 6) six\n",
            " 7) seven\n",
            " 8) eight\n",
            " 9) nine\n",
            "10) ten\n",
        );
        assert_eq!(got, expected);
    }
}
