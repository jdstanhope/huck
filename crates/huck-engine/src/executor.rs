use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::io::RawFd;
use std::process::{Command as ProcessCommand, Stdio};

use crate::builtins::{self, ExecOutcome, InterruptReason};
use crate::child_fd::{ChildFd, ChildStdio};
use crate::command::{
    CaseClause, CaseItem, CaseTerminator, Command, Connector, ExecCommand, FileMode, ForClause,
    IfClause, Pipeline, RedirFd, RedirOp, RedirectSlot, Redirection, Sequence, SimpleCommand,
    TestBinaryOp, TestExpr, TestUnaryOp, WhileClause,
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
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
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

/// Emit a path-bearing redirect-open failure in bash's format:
/// `<prologue>{path}: {strerror}` via `sh_error_to!` — the non-interactive
/// prologue (`<src>: line N: `) or interactive `huck: `, the offending path,
/// and `bash_io_error` (strerror with no Rust `(os error N)` suffix). Single
/// home for every `File::open`/`open_resolved`/`OpenOptions` redirect-open
/// error so the format stays identical across execution contexts. Routes
/// through the CALLER's redirect-aware writer (built from `err_sink`/`out_sink`)
/// rather than the thread-local sink, so an inner `2>&1`/capture on the
/// surrounding command is honored (v269 T4fix).
pub(crate) fn redir_open_error(
    shell: &Shell,
    err_sink: &mut StderrSink,
    out_sink: &mut StdoutSink,
    path: &str,
    e: &std::io::Error,
) {
    let mut err = err_writer(err_sink, out_sink);
    crate::sh_error_to!(
        shell,
        &mut *err,
        None,
        "{path}: {}",
        crate::bash_io_error(e)
    );
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
    if shell.shell_options.errexit && shell.err_suppressed_depth == 0 && status != 0 {
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
                return run_background_subshell(&seq.first, shell, sink, err_sink, false, source);
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
            let subshell = Command::Subshell {
                body: Box::new(inner),
            };
            return run_background_subshell(&subshell, shell, sink, err_sink, false, source);
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
    let sanitized = if seq.background || seq.rest.iter().any(|(c, _)| matches!(c, Connector::Amp)) {
        Sequence {
            first: seq.first.clone(),
            rest: seq
                .rest
                .iter()
                .map(|(c, cmd)| {
                    let c = if matches!(c, Connector::Amp) {
                        Connector::Semi
                    } else {
                        *c
                    };
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
        ExecOutcome::Exit(_)
            | ExecOutcome::LoopBreak(_, _)
            | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_)
            | ExecOutcome::Interrupted(_)
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
        // errexit / ERR fire for first's failure only when it is the
        // SYNTACTICALLY LAST command in this and-or list. bash exempts every
        // and-or-list command except the last, regardless of whether the
        // following connector is `&&` or `||` (a command followed by either is
        // "part of a list being tested", not a standalone failure). `first`
        // is last iff there is no `rest`.
        let is_last = rest.is_empty();
        if c != 0 && shell.err_suppressed_depth == 0 && is_last && !is_negated_pipeline(first) {
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
                ExecOutcome::Exit(_)
                    | ExecOutcome::LoopBreak(_, _)
                    | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_)
                    | ExecOutcome::Interrupted(_)
            ) {
                return status;
            }
            if let ExecOutcome::Continue(c) = status {
                shell.set_last_status(c);
                if shell.pending_fatal_status.is_some() {
                    return ExecOutcome::Continue(c);
                }
                crate::traps::dispatch_pending_traps(shell);
                // errexit / ERR fire only when this failing command is the
                // SYNTACTICALLY LAST in the and-or list. A command followed by
                // `&&` OR `||` is exempt (bash rule) — the two differ from the
                // old `!next_is_or` gate only in the `&&`-next case, which was
                // the bug. This command is last iff there is no rest[i+1].
                let is_last = i + 1 == rest.len();
                if c != 0
                    && shell.err_suppressed_depth == 0
                    && is_last
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
            // bash: a bare multi-stage pipeline backgrounded via `&` keeps the
            // shell's stdin on stage 0; every other async unit gets /dev/null.
            let inherit_stdin = group.rest.is_empty()
                && matches!(group.first, Command::Pipeline(p) if p.commands.len() > 1);
            let source = group_display_label(group.first);
            let subshell = Command::Subshell {
                body: Box::new(inner),
            };
            // Launch; ignore the Continue(0) it returns — the foreground status
            // is unchanged by a background launch.
            run_background_subshell(&subshell, shell, sink, err_sink, inherit_stdin, &source);
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        if let Some(r) = capture_read_fd {
                            unsafe {
                                libc::close(r);
                            }
                        }
                        if stdout_fd != libc::STDOUT_FILENO {
                            unsafe {
                                libc::close(stdout_fd);
                            }
                        }
                        return ExecOutcome::Continue(1);
                    }
                },
            };

            // Build the child's fd environment. stdin inherits; stdout/stderr are
            // the shell's real streams (Inherit) or freshly-made capture-pipe
            // write ends (Owned). Merged stderr dups whatever stdout resolves to.
            let child_stdout = if stdout_fd == libc::STDOUT_FILENO {
                ChildFd::Inherit
            } else {
                unsafe { ChildFd::owned_raw(stdout_fd) }
            };
            let child_stderr = match err_sink {
                StderrSink::Terminal => ChildFd::Inherit,
                // SAFETY: the slot (STDOUT_FILENO) is always a live shell std fd.
                StderrSink::Merged => match child_stdout.try_clone_resolving(libc::STDOUT_FILENO) {
                    Ok(c) => c,
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "fork: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        if let Some(r) = capture_read_fd {
                            unsafe {
                                libc::close(r);
                            }
                        }
                        if let Some(r) = capture_err_read_fd {
                            unsafe {
                                libc::close(r);
                            }
                        }
                        return ExecOutcome::Continue(1);
                    }
                },
                StderrSink::Capture(_) => unsafe { ChildFd::owned_raw(stderr_fd) },
            };
            let child_stdio = ChildStdio::new(ChildFd::Inherit, child_stdout, child_stderr);

            let pid = match fork_and_run_in_subshell(
                cmd,
                shell,
                child_stdio,
                if interactive { 0 } else { NO_PGROUP },
                &[],
                None, // no Dup redirect at this call site
                None,
            ) {
                Ok(p) => p,
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "fork: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    if let Some(r) = capture_read_fd {
                        unsafe {
                            libc::close(r);
                        }
                    }
                    if let Some(r) = capture_err_read_fd {
                        unsafe {
                            libc::close(r);
                        }
                    }
                    // child_stdio (with any owned stdout/stderr write end) was
                    // consumed by the failed call and already dropped.
                    return ExecOutcome::Continue(1);
                }
            };

            // Register the subshell child in the live-children registry so the
            // timeout timer thread can SIGTERM it if the deadline fires. The
            // guard pops on every exit path (including early returns / panics).
            let live_pids = shell.live_external_children.clone();
            live_pids.lock().unwrap().push(pid as libc::pid_t);
            let _pid_guard = LiveChildGuard {
                pids: &live_pids,
                pid: pid as libc::pid_t,
            };

            // The child's stdout/stderr pipe write ends were owned by the moved
            // ChildStdio and closed in the parent by the call — the read ends
            // (capture_read_fd / capture_err_read_fd) stay open for draining.

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
                setpgid_self(pid);
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
                        let line = shell
                            .jobs
                            .iter()
                            .find(|j| j.id == job_id)
                            .map(|j| crate::jobs::notification_line(j, '+'))
                            .unwrap_or_default();
                        {
                            let mut err = err_writer(err_sink, sink);
                            e!(&mut *err, "\n{line}");
                        }
                        128 + sig
                    }
                    Ok((raw_status, false)) => raw_status_to_exit_code(raw_status, shell),
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
                let stderr_sink_buf: Option<&mut Vec<u8>> =
                    if matches!(err_sink, StderrSink::Capture(_)) {
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
                    unsafe {
                        libc::close(pipe_out);
                    }
                }
                if pipe_err >= 0 {
                    unsafe {
                        libc::close(pipe_err);
                    }
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
                let code = raw_status_to_exit_code(raw_status, shell);
                shell.set_pipestatus(&[code]);
                ExecOutcome::Continue(code)
            }
        }
        Command::FunctionDef { name, body } => {
            // POSIX: a function may not be named after a special builtin; a
            // non-interactive posix shell errors and exits (default mode allows it).
            if shell.shell_options.posix && builtins::is_special_builtin(name) {
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(shell, &mut *err, None, "{name}: is a special builtin");
                }
                shell.posix_fatal(2);
                return ExecOutcome::Continue(2);
            }
            shell.define_function(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
        Command::DoubleBracket {
            expr,
            inline_assignments,
        } => run_double_bracket(expr, inline_assignments, shell, sink, err_sink),
        Command::ArithFor(clause) => run_arith_for(clause, shell, sink, err_sink),
        Command::Arith(expr) => run_arith(expr, shell, sink, err_sink),
        Command::Select(clause) => run_select(clause, shell, sink, err_sink),
        Command::Redirected { inner, redirects } => {
            run_redirected(inner, redirects, shell, sink, err_sink)
        }
        Command::Coproc { name, body } => run_coproc(name, body, shell, sink, err_sink),
        _ => {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "unsupported command variant");
            }
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
        RedirectScope {
            saved: Vec::new(),
            heredoc_writers: Vec::new(),
        }
    }

    /// Replace `target_fd` with a dup of `new_fd`, saving the original so Drop
    /// can restore it. `new_fd` is NOT consumed (caller closes it). If
    /// `target_fd` is not currently open, the saved slot is recorded as `-1`
    /// (Drop closes it back to unopened) — bash leaves a fresh high fd open
    /// only for the command's duration.
    fn redirect(
        &mut self,
        shell: &Shell,
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
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "dup2: {}",
                        io::Error::last_os_error()
                    );
                }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "ambiguous redirect");
            }
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
                            redir_open_error(shell, err_sink, sink, &path, &e);
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
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        // `<>`: O_RDWR|O_CREAT — open in place, do NOT truncate
                        // (bash keeps existing content for read-write access).
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
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
                    if self
                        .redirect(shell, new_fd, target, sink, err_sink)
                        .is_err()
                    {
                        unsafe { libc::close(new_fd) };
                        return Err(ExecOutcome::Continue(1));
                    }
                    unsafe { libc::close(new_fd) };
                }
                Ok(())
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                // `>&w` / `<&w` (dup), and `>&w-` / `<&w-` (move = dup then close
                // the source). Resolved AFTER earlier swaps so e.g. `>file 2>&1`
                // makes stderr follow the already-redirected stdout.
                let is_move = matches!(&redir.op, RedirOp::Move { .. });
                let src = resolve_dup_source(source, shell, sink, err_sink)
                    .map_err(|()| ExecOutcome::Continue(1))?;
                // bash (redir.c do_redirection_internal): a degenerate move
                // `N>&N-` (source == target) is guarded by `redir_fd != redirector`
                // — a pure no-op with no fd validation, dup2, or close.
                if is_move && src == target {
                    return Ok(());
                }
                // Validate the source fd is open before dup2 (bash: bad fd error).
                validate_fd_open(src, shell, sink, err_sink)
                    .map_err(|()| ExecOutcome::Continue(1))?;
                if self.redirect(shell, src, target, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
                if is_move {
                    // The "move": close the source fd (save/restore via
                    // close_target so a command-scoped move restores it; `exec`
                    // persists).
                    self.close_target(src);
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
                        if self.redirect(shell, rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
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
                        if self.redirect(shell, rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
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
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(shell, &mut *err, None, "{name}: ambiguous redirect");
                    }
                    return Err(ExecOutcome::Continue(1));
                }
            };
            // Save prior state so Drop restores (saved == -1 if it was unopened),
            // then close. EBADF (already closed) is lenient per bash.
            self.close_target(fd);
            return Ok(());
        }
        // A Move mirrors the Dup arm to resolve `src`, but the source fd must be
        // closed (save/restore aware, via `close_target`) after the dup — this
        // flag is checked once `high` is allocated, below.
        let is_move = matches!(&redir.op, RedirOp::Move { .. });
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
                            redir_open_error(shell, err_sink, sink, &path, &e);
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
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                };
                (fd, true)
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                // Dup and move resolve the source fd identically here; a move's
                // extra source-close happens below via `close_target` (save/
                // restore) once `high` is allocated, gated on `is_move`. The
                // source is not "owned" (the shell's fd, not a temp we opened).
                let src = resolve_dup_source(source, shell, sink, err_sink)
                    .map_err(|()| ExecOutcome::Continue(1))?;
                validate_fd_open(src, shell, sink, err_sink)
                    .map_err(|()| ExecOutcome::Continue(1))?;
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
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
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "{name}: {}",
                        crate::bash_io_error(&e)
                    );
                }
                return Err(ExecOutcome::Continue(1));
            }
        };
        if owns_src {
            // The opened file / heredoc read-end was only a temp to dup from.
            unsafe { libc::close(src) };
        }
        if is_move {
            // The "move": close the source fd, save/restore aware.
            self.close_target(src);
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
            unsafe {
                libc::waitpid(pid, &mut st, 0);
            }
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
/// Any output File, a Dup (`>&N`), a Move (`>&N-`), OR a Close (`>&-`) on fd 1
/// qualifies: in all cases the command's real fd 1 is redirected (to a file,
/// another fd, or closed), so an in-process builtin must write through
/// `io::stdout()` (= fd 1 =
/// the redirect target) rather than into the capture buffer — otherwise `>&-`'s
/// discard / `>&N`'s dup would be silently ignored by the buffer. A stdin-only
/// redirect does not force Terminal. `RedirFd::Var` (target_fd None) is ignored.
fn redirs_write_stdout(redirs: &[Redirection]) -> bool {
    redirs.iter().any(|r| {
        r.target_fd() == Some(1)
            && matches!(
                &r.op,
                RedirOp::File {
                    mode: FileMode::Truncate
                        | FileMode::Append
                        | FileMode::Clobber
                        | FileMode::ReadWrite,
                    ..
                } | RedirOp::Dup { .. }
                    | RedirOp::Move { .. }
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
fn final_dests_for_1_2(redirs: &[Redirection], shell: &mut Shell) -> (RedirectDest, RedirectDest) {
    let mut fd1 = RedirectDest::Sink;
    let mut fd2 = RedirectDest::Sink;
    for r in redirs {
        let Some(fd) = r.target_fd() else { continue };
        if fd != 1 && fd != 2 {
            continue;
        }
        let dest = match &r.op {
            RedirOp::Dup {
                source: src_word,
                output: true,
            } => match resolve_fd_target(src_word, shell) {
                Ok(n) if n >= 0 => RedirectDest::Follows(n as u32),
                _ => RedirectDest::External,
            },
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
            RedirOp::File {
                mode: FileMode::ReadOnly,
                ..
            } | RedirOp::Heredoc { .. }
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
        shell.procsub_pending.len(),
        procsub_base,
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
    let write_to_fd1 =
        !route_out_to_err && (redirs_write_stdout(redirs) || matches!(sink, StdoutSink::Terminal));
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
            (_, StderrSink::Terminal) => {
                unreachable!("route_out_to_err requires non-Terminal err_sink")
            }
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
    with_redirect_scope(
        redirects,
        shell,
        sink,
        err_sink,
        |shell, inner_sink, inner_err_sink| run_command(inner, shell, inner_sink, inner_err_sink),
    )
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
            ExecOutcome::Exit(_)
            | ExecOutcome::LoopBreak(_, _)
            | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_)
            | ExecOutcome::Interrupted(_) => {
                return cond;
            }
            ExecOutcome::Continue(c) => {
                if clause.until {
                    c != 0
                } else {
                    c == 0
                }
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
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "`{}': not a valid identifier",
                clause.var
            );
        }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{}: readonly variable", clause.var);
            }
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
    xtrace_compound(
        shell,
        &format!(
            "(( {} ))",
            crate::expand::reconstruct_word_source_inner(body)
        ),
    );
    let (src, res) = crate::expand::eval_arith_word_src(body, shell);
    match res {
        Ok(0) => ExecOutcome::Continue(1),
        Ok(_) => ExecOutcome::Continue(0),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                Some("(("),
                "{}",
                crate::arith::render_error_body(&src, &e)
            );
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
        xtrace_compound(
            shell,
            &format!(
                "(( {} ))",
                crate::expand::reconstruct_word_source_inner(init)
            ),
        );
    }
    if let Some(init) = &clause.init
        && let Err(e) = crate::expand::eval_arith_word(init, shell)
    {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "((: {e}");
        }
        return ExecOutcome::Continue(1);
    }

    let mut last = ExecOutcome::Continue(0);
    loop {
        // SIGINT check (mirrors run_for).
        if let Some(o) = check_interrupt(shell) {
            return o;
        }

        if let Some(c) = &clause.cond {
            xtrace_compound(
                shell,
                &format!("(( {} ))", crate::expand::reconstruct_word_source_inner(c)),
            );
        }
        // 2. Eval cond. Empty cond = always true (matches bash).
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::expand::eval_arith_word(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(shell, &mut *err, None, "((: {e}");
                    }
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
            xtrace_compound(
                shell,
                &format!(
                    "(( {} ))",
                    crate::expand::reconstruct_word_source_inner(step)
                ),
            );
        }
        if let Some(step) = &clause.step
            && let Err(e) = crate::expand::eval_arith_word(step, shell)
        {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "((: {e}");
            }
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
                words
                    .iter()
                    .map(crate::expand::reconstruct_word_source)
                    .collect::<Vec<_>>()
                    .join(" ")
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{}: readonly variable", clause.var);
            }
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
        &format!(
            "case {} in",
            crate::expand::reconstruct_word_source(&clause.subject)
        ),
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
        ExecOutcome::Exit(_)
            | ExecOutcome::LoopBreak(_, _)
            | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_)
            | ExecOutcome::Interrupted(_)
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
            ExecOutcome::Exit(_)
                | ExecOutcome::LoopBreak(_, _)
                | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_)
                | ExecOutcome::Interrupted(_)
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
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "[[: {msg}");
            }
            ExecOutcome::Continue(2)
        }
    };
    restore_inline_assignments(snap, shell);
    result
}

fn test_unary_op_str(op: crate::command::TestUnaryOp) -> &'static str {
    use crate::command::TestUnaryOp as U;
    match op {
        U::FileExists => "-e",
        U::IsRegFile => "-f",
        U::IsDir => "-d",
        U::IsReadable => "-r",
        U::IsWritable => "-w",
        U::IsExecutable => "-x",
        U::IsNonEmpty => "-s",
        U::IsSymlink => "-L",
        U::StringNonEmpty => "-n",
        U::StringEmpty => "-z",
        U::VarSet => "-v",
        U::OptEnabled => "-o",
        U::IsFifo => "-p",
        U::IsSocket => "-S",
        U::IsBlockDev => "-b",
        U::IsCharDev => "-c",
        U::OwnedByEuid => "-O",
        U::OwnedByEgid => "-G",
        U::NewerThanRead => "-N",
        U::IsSticky => "-k",
        U::IsSetuid => "-u",
        U::IsSetgid => "-g",
        U::IsTerminal => "-t",
    }
}

fn test_binary_op_str(op: crate::command::TestBinaryOp) -> &'static str {
    use crate::command::TestBinaryOp as B;
    match op {
        B::StringEq => "==",
        B::StringNe => "!=",
        B::StringLt => "<",
        B::StringGt => ">",
        B::IntEq => "-eq",
        B::IntNe => "-ne",
        B::IntLt => "-lt",
        B::IntGt => "-gt",
        B::IntLe => "-le",
        B::IntGe => "-ge",
        B::NewerThan => "-nt",
        B::OlderThan => "-ot",
        B::SameFile => "-ef",
    }
}

/// bash shows an empty `[[ ]]` operand as `''` and a non-empty one raw.
fn xtrace_operand(s: &str) -> String {
    if s.is_empty() {
        "''".to_string()
    } else {
        s.to_string()
    }
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
            format!(
                "{} {} {}",
                xtrace_operand(&l),
                test_binary_op_str(*op),
                xtrace_operand(&r)
            )
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

fn eval_test_expr_traced(
    expr: &TestExpr,
    shell: &mut Shell,
    suppress: bool,
) -> Result<bool, String> {
    if !suppress
        && shell.shell_options.xtrace
        && matches!(
            expr,
            TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. }
        )
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
            let p = if shell.nocasematch() {
                format!("(?i){p}")
            } else {
                p
            };
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
                    let _ =
                        shell.replace_indexed("BASH_REMATCH", std::collections::BTreeMap::new());
                    Ok(false)
                }
            }
        }
        TestExpr::Not(inner) => {
            if !suppress
                && shell.shell_options.xtrace
                && matches!(
                    **inner,
                    TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. }
                )
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
        TestUnaryOp::StringEmpty => s.is_empty(),
        // Delegate all file tests to the shared test_builtin logic.
        TestUnaryOp::FileExists => {
            test_builtin::evaluate(&["-e".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsRegFile => {
            test_builtin::evaluate(&["-f".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsDir => {
            test_builtin::evaluate(&["-d".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsReadable => {
            test_builtin::evaluate(&["-r".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsWritable => {
            test_builtin::evaluate(&["-w".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsExecutable => {
            test_builtin::evaluate(&["-x".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsNonEmpty => {
            test_builtin::evaluate(&["-s".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsSymlink => {
            test_builtin::evaluate(&["-L".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsFifo => {
            test_builtin::evaluate(&["-p".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsSocket => {
            test_builtin::evaluate(&["-S".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsBlockDev => {
            test_builtin::evaluate(&["-b".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsCharDev => {
            test_builtin::evaluate(&["-c".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::OwnedByEuid => {
            test_builtin::evaluate(&["-O".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::OwnedByEgid => {
            test_builtin::evaluate(&["-G".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::NewerThanRead => {
            test_builtin::evaluate(&["-N".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsSticky => {
            test_builtin::evaluate(&["-k".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsSetuid => {
            test_builtin::evaluate(&["-u".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsSetgid => {
            test_builtin::evaluate(&["-g".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::IsTerminal => {
            test_builtin::evaluate(&["-t".to_string(), s.to_string()]).unwrap_or(false)
        }
        TestUnaryOp::VarSet => unreachable!("VarSet handled in eval_test_expr"),
        TestUnaryOp::OptEnabled => unreachable!("OptEnabled handled in eval_test_expr"),
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
            // G3: the `==`/`!=` RHS inside `[[ … ]]` is ALWAYS an extended
            // pattern in bash — an `@(a|b)`/`!(x)`-shaped group matches as extglob
            // regardless of `shopt extglob` (the parser likewise force-recognizes
            // it). So gate ONLY on the pattern SHAPE here, not the runtime option
            // (unlike `case`/globbing, which honor the shopt).
            let matched = if crate::glob_match::has_extglob(&pattern_str)
                || crate::glob_match::has_posix_class(&pattern_str)
            {
                crate::glob_match::extglob_match(&pattern_str, lhs, nocase)
            } else {
                let npat = crate::glob_match::translate_bracket_negation(&pattern_str);
                let pat = glob::Pattern::new(&npat).map_err(|e| format!("bad pattern: {e}"))?;
                pat.matches_with(
                    lhs,
                    glob::MatchOptions {
                        case_sensitive: !nocase,
                        require_literal_separator: false,
                        require_literal_leading_dot: false,
                    },
                )
            };
            Ok(if matches!(op, TestBinaryOp::StringEq) {
                matched
            } else {
                !matched
            })
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

/// The stdin an async (`&`) child should start with. bash defaults async stdin
/// to `/dev/null` when the shell is non-interactive and the unit is not a bare
/// multi-stage pipeline; otherwise the child inherits the shell's stdin.
/// Decide an async child's default stdin as a `ChildFd`. `inherit` is true when
/// the unit must keep the shell's stdin regardless of interactivity (a bare
/// multi-stage pipeline). Interactive always inherits. Otherwise stdin defaults
/// to `/dev/null` (`Owned`); on open failure emit `/dev/null: <error>` + Err.
fn async_default_stdin(
    inherit: bool,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<ChildFd, ()> {
    if inherit || shell.is_interactive {
        return Ok(ChildFd::Inherit);
    }
    match File::open("/dev/null") {
        Ok(f) => Ok(ChildFd::from(f)),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "/dev/null: {}",
                crate::bash_io_error(&e)
            );
            Err(())
        }
    }
}

/// Backgrounds a `Command::Subshell` via fork + job registration.
/// Used by `execute()` when `seq.background` is set and `seq.first` is
/// `Command::Subshell`.  Does NOT waitpid — returns immediately after
/// registering the job.
fn run_background_subshell(
    cmd: &Command,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    inherit_stdin: bool,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);
    let job_control = shell.job_control_active();
    // bash: an async command's stdin defaults to /dev/null (non-interactive, no
    // explicit input redirect) so it can't steal the terminal; a bare
    // multi-stage pipeline inherits instead (async_default_stdin, #126).
    let stdin = match async_default_stdin(inherit_stdin, shell, sink, err_sink) {
        Ok(c) => c,
        Err(()) => return ExecOutcome::Continue(1),
    };
    let fork_result = fork_and_run_in_subshell(
        cmd,
        shell,
        ChildStdio::new(stdin, ChildFd::Inherit, ChildFd::Inherit),
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
        /*parent_fds_to_close=*/ &[],
        None, // no Dup redirect at this call site
        None,
    );
    // The parent's /dev/null copy (if any) was consumed + dropped by the call.
    match fork_result {
        Ok(pid) => {
            shell.last_bg_pid = Some(pid);
            let id = shell
                .jobs
                .add_with_pgroup(pid, vec![pid], display, job_control);
            // bash suppresses automatic job notices inside a subshell environment / completion funcs
            if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
                {
                    let mut err = err_writer(err_sink, sink);
                    e!(&mut *err, "[{id}] {pid}");
                }
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "fork: {}", crate::bash_io_error(&e));
            }
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

    // Stage-0 stdin default (async rule, shared with run_background_subshell): a
    // bare multi-stage pipeline's stage 0 inherits the shell's stdin; a single
    // async command gets /dev/null when non-interactive; interactive always
    // inherits. (#129)
    let stage0_default: ChildFd =
        match async_default_stdin(pipeline.commands.len() > 1, shell, sink, err_sink) {
            Ok(c) => c,
            Err(()) => return ExecOutcome::Continue(1),
        };

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
                unsafe {
                    libc::close(r);
                }
            }
            // Run via fork so it's isolated (assignments don't affect parent).
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone(), *aline));
            let pgid_target = if job_control {
                first_pid.unwrap_or(0)
            } else {
                NO_PGROUP
            };
            // stage 0 default (a distinct clone of the shared /dev/null / inherit).
            let stdin: ChildFd = match stage0_default.try_clone() {
                Ok(c) => c,
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "dup: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return bail_teardown_bg(
                        shell,
                        procsub_base,
                        first_pid,
                        &spawned_pids,
                        &mut parent_held,
                    );
                }
            };
            // For a no-op assign stage, stdout is irrelevant but we still need
            // to either pipe or close it for downstream stages.
            let stdout: ChildFd = if !is_last {
                match make_pipe() {
                    Ok((r, w)) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                        unsafe { ChildFd::owned_raw(w) }
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        return bail_teardown_bg(
                            shell,
                            procsub_base,
                            first_pid,
                            &spawned_pids,
                            &mut parent_held,
                        );
                    }
                }
            } else {
                ChildFd::Inherit
            };
            let mut fds_to_close: Vec<RawFd> = parent_held
                .iter()
                .copied()
                .filter(|&fd| Some(fd) != stdout.raw() && Some(fd) != stdin.raw())
                .collect();
            // The parent's shared /dev/null original is inherited by the child;
            // close it there (the child's own stdin is a distinct clone).
            if let Some(d) = stage0_default.raw() {
                fds_to_close.push(d);
            }
            let child_stdio = ChildStdio::new(stdin, stdout, ChildFd::Inherit);
            match fork_and_run_in_subshell(
                &assign_cmd,
                shell,
                child_stdio,
                pgid_target,
                &fds_to_close,
                None,
                None,
            ) {
                Ok(pid) => {
                    // The pipe write end (if any) was owned by the moved
                    // child_stdio and closed in the parent by the call.
                    if first_pid.is_none() {
                        first_pid = Some(pid);
                        if job_control {
                            unsafe {
                                if libc::setpgid(pid, pid) != 0 {
                                    let errno =
                                        std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
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
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "fork: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    // child_stdio (with any owned write end) was consumed by the
                    // failed call and already dropped.
                    return bail_teardown_bg(
                        shell,
                        procsub_base,
                        first_pid,
                        &spawned_pids,
                        &mut parent_held,
                    );
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
                return bail_teardown_bg(
                    shell,
                    procsub_base,
                    first_pid,
                    &spawned_pids,
                    &mut parent_held,
                );
            }
        };

        // ---- Stdin fd ---------------------------------------------------------
        // A heredoc/herestring stdin is fed by a forked writer process (M-120);
        // the read end becomes this stage's stdin and the parent holds no write
        // end.
        let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.slot_stdin() {
                Some(RedirectSlot::Read(word)) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_bg(
                                shell,
                                procsub_base,
                                first_pid,
                                &spawned_pids,
                                &mut parent_held,
                            );
                        }
                    };
                    match File::open(&path) {
                        Ok(f) => ChildFd::from(f),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_bg(
                                shell,
                                procsub_base,
                                first_pid,
                                &spawned_pids,
                                &mut parent_held,
                            );
                        }
                    }
                }
                Some(RedirectSlot::Heredoc { body, .. }) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    // Forked writer (M-120): the read end is this stage's stdin;
                    // the writer process is an internal helper collected by the
                    // existing SIGCHLD reaper — never a job, never $!, so it is
                    // NOT added to spawned_pids/first_pid.
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, _pid)) => unsafe { ChildFd::owned_raw(r) },
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_bg(
                                shell,
                                procsub_base,
                                first_pid,
                                &spawned_pids,
                                &mut parent_held,
                            );
                        }
                    }
                }
                Some(RedirectSlot::HereString(body)) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    // Here-string: expand with no split/glob + trailing newline,
                    // then feed via a forked writer (M-120; see Heredoc above).
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, _pid)) => unsafe { ChildFd::owned_raw(r) },
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_bg(
                                shell,
                                procsub_base,
                                first_pid,
                                &spawned_pids,
                                &mut parent_held,
                            );
                        }
                    }
                }
                _ => match prev_pipe_read.take() {
                    Some(r) => {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { ChildFd::owned_raw(r) }
                    }
                    None => match stage0_default.try_clone() {
                        Ok(c) => c,
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "dup: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_bg(
                                shell,
                                procsub_base,
                                first_pid,
                                &spawned_pids,
                                &mut parent_held,
                            );
                        }
                    },
                },
            }
        } else {
            // Compound stage: use prev pipe or /dev/null for stage 0.
            match prev_pipe_read.take() {
                Some(r) => {
                    parent_held.retain(|&fd| fd != r);
                    unsafe { ChildFd::owned_raw(r) }
                }
                None => match stage0_default.try_clone() {
                    Ok(c) => c,
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "dup: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        return bail_teardown_bg(
                            shell,
                            procsub_base,
                            first_pid,
                            &spawned_pids,
                            &mut parent_held,
                        );
                    }
                },
            }
        };

        // ---- Stdout redirect (ExecCommand only) ------------------------------
        let explicit_stdout: Option<ChildFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stdout() {
                    Some(r @ (RedirectSlot::Truncate(w) | RedirectSlot::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        };
                        let guard =
                            shell.shell_options.noclobber && !matches!(r, RedirectSlot::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    Some(RedirectSlot::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        };
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Stderr redirect (ExecCommand only) ------------------------------
        let explicit_stderr: Option<ChildFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stderr() {
                    Some(r @ (RedirectSlot::Truncate(w) | RedirectSlot::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        };
                        let guard =
                            shell.shell_options.noclobber && !matches!(r, RedirectSlot::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    Some(RedirectSlot::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        };
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Stdout fd -------------------------------------------------------
        let stdout: ChildFd = if let Some(cf) = explicit_stdout {
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        // stdin / cf (the open file) / explicit_stderr all drop here.
                        drain_procsubs(shell, procsub_base);
                        cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                        for pfd in parent_held.drain(..) {
                            unsafe {
                                libc::close(pfd);
                            }
                        }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            cf
        } else if !is_last {
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    unsafe { ChildFd::owned_raw(w) }
                }
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "pipe: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    restore_inline_assignments(snap, shell);
                    // stdin / explicit_stderr drop here.
                    return bail_teardown_bg(
                        shell,
                        procsub_base,
                        first_pid,
                        &spawned_pids,
                        &mut parent_held,
                    );
                }
            }
        } else {
            ChildFd::Inherit
        };

        let stderr: ChildFd = explicit_stderr.unwrap_or(ChildFd::Inherit);

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if job_control {
            first_pid.unwrap_or(0)
        } else {
            NO_PGROUP
        };

        let mut fds_to_close_in_child: Vec<RawFd> = parent_held
            .iter()
            .copied()
            .filter(|&fd| {
                Some(fd) != stdin.raw() && Some(fd) != stdout.raw() && Some(fd) != stderr.raw()
            })
            .collect();
        // The parent's shared /dev/null original (stage0_default) is inherited by
        // every child; close it there. Each stage's own stdin is a distinct clone
        // or pipe end, so this never closes a fd the child needs.
        if let Some(d) = stage0_default.raw() {
            fds_to_close_in_child.push(d);
        }
        // (Any heredoc pipe write end lives in the forked writer process, not
        // here, so there is nothing extra to add to fds_to_close_in_child.)

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.slot_stdout() {
                    Some(RedirectSlot::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                {
                                    let mut err = err_writer(err_sink, sink);
                                    crate::sh_error_to!(
                                        shell,
                                        &mut *err,
                                        None,
                                        "{}",
                                        crate::bash_io_error(&e)
                                    );
                                }
                                restore_inline_assignments(snap, shell);
                                // stdin / stdout / stderr ChildFds drop on return.
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.slot_stderr() {
                    Some(RedirectSlot::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                {
                                    let mut err = err_writer(err_sink, sink);
                                    crate::sh_error_to!(
                                        shell,
                                        &mut *err,
                                        None,
                                        "{}",
                                        crate::bash_io_error(&e)
                                    );
                                }
                                restore_inline_assignments(snap, shell);
                                // stdin / stdout / stderr ChildFds drop on return.
                                return bail_teardown_bg(
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &spawned_pids,
                                    &mut parent_held,
                                );
                            }
                        }
                    }
                    _ => None,
                };
                (sdt, sedt)
            } else {
                (None, None)
            };

        // Build the child's fd environment ONCE; move it into whichever spawner.
        // Both spawners consume it and close the parent's owned copies (on both
        // the success and error paths — RAII), so there is no post-spawn parent
        // close bookkeeping to do.
        let child_stdio = ChildStdio::new(stdin, stdout, stderr);
        let spawn_result = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => spawn_external_with_fds(
                simple,
                shell,
                sink,
                err_sink,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
            ),
            StageKind::InProcess(cmd) => fork_and_run_in_subshell(
                cmd,
                shell,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
                stdout_dup_target,
                stderr_dup_target,
            ),
        };

        restore_inline_assignments(snap, shell);

        let pid = match spawn_result {
            Ok(p) => p,
            Err(e) => {
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(shell, &mut *err, None, "{}", crate::bash_io_error(&e));
                }
                // child_stdio (with all owned stdio fds) was consumed by the
                // failed call and already dropped; the pipe read end (if any)
                // stays in parent_held and is closed by bail_teardown_bg.
                return bail_teardown_bg(
                    shell,
                    procsub_base,
                    first_pid,
                    &spawned_pids,
                    &mut parent_held,
                );
            }
        };

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
        unsafe {
            libc::close(fd);
        }
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
    let id = shell
        .jobs
        .add_with_pgroup(pgid, spawned_pids, display, job_control);
    // bash suppresses automatic job notices inside a subshell environment / completion funcs
    if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
        {
            let mut err = err_writer(err_sink, sink);
            e!(&mut *err, "[{id}] {last_pid}");
        }
    }
    // Non-blocking drain: close parent fds and attempt WNOHANG reap of inner
    // procsub children. We don't block here because a long-running inner producer
    // (e.g. `cmd < <(long-running-gen) &`) should not make the background job
    // synchronous. Any child not yet exited is left to SIGCHLD/exit cleanup.
    drain_procsubs_nonblocking(shell, procsub_base);
    ExecOutcome::Continue(0)
}

/// Pipeline-spawn error teardown for `run_background_sequence`: reap process
/// substitutions started this pipeline, kill+reap already-spawned stages, close
/// every parent-held pipe fd, and return failure. Extracted from the ~19
/// byte-identical bail sites in that function.
fn bail_teardown_bg(
    shell: &mut Shell,
    procsub_base: usize,
    first_pid: Option<i32>,
    spawned_pids: &[i32],
    parent_held: &mut Vec<RawFd>,
) -> ExecOutcome {
    drain_procsubs(shell, procsub_base);
    cleanup_partial_pipeline_raw(first_pid, spawned_pids);
    for fd in parent_held.drain(..) {
        unsafe {
            libc::close(fd);
        }
    }
    ExecOutcome::Continue(1)
}

/// Pipeline-spawn error teardown for `run_multi_stage`: reap process
/// substitutions and close every parent-held pipe fd, then return failure. The
/// foreground path tracks live pids separately (`live_pids_arc`/`pipeline_stages`),
/// so there is no `cleanup_partial_pipeline_raw` here — that is the only
/// difference from `bail_teardown_bg`. Extracted from the ~21 byte-identical sites.
fn bail_teardown_stage(
    shell: &mut Shell,
    procsub_base: usize,
    parent_held: &mut Vec<RawFd>,
) -> ExecOutcome {
    drain_procsubs(shell, procsub_base);
    for fd in parent_held.drain(..) {
        unsafe {
            libc::close(fd);
        }
    }
    ExecOutcome::Continue(1)
}

/// Best-effort `setpgid(pid, pid)` to guarantee the child's process group
/// exists before `give_terminal_to`/`tcsetpgrp` (a race-close mirror of what
/// `fork_and_run_in_subshell` already does in the parent). A failure here is
/// expected only as ESRCH/EACCES (the child already exec'd or changed its pgrp).
fn setpgid_self(pid: i32) {
    unsafe {
        if libc::setpgid(pid, pid) != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            debug_assert!(
                errno == libc::ESRCH || errno == libc::EACCES,
                "setpgid({pid}, {pid}) failed with unexpected errno {errno}"
            );
        }
    }
}

/// Decodes a raw `waitpid` status into a shell exit code: `WEXITSTATUS` for a
/// normal exit, `128 + signal` for a signal death (recording a pending SIGINT
/// so the interactive loop can react), and `1` otherwise. Extracted from the
/// four byte-identical decode sites.
fn raw_status_to_exit_code(raw_status: libc::c_int, shell: &Shell) -> i32 {
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
        unsafe {
            libc::waitpid(pid, &mut raw, 0);
        }
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
            if let Command::Simple(s) = &p.commands[0] {
                Some(s)
            } else {
                None
            }
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
        crate::sh_error_to!(shell, err, None, "ambiguous redirect");
        Err(())
    }
}

/// Expands `source` to a string and parses it as an fd number (e.g. "1" or "2").
/// Used for `RedirectSlot::Dup { source }` to obtain the target fd pre-fork.
/// Errors with "bad fd: ..." if the expansion is not a valid non-negative integer.
fn resolve_fd_target(source: &crate::lexer::Word, shell: &mut Shell) -> Result<i32, io::Error> {
    let expanded = expand_assignment(source, shell);
    expanded
        .parse::<i32>()
        .map_err(|_| io::Error::other(format!("bad fd: {expanded}")))
}

/// Resolve a `>&w` / `<&w` / move (`>&w-`) source word to an fd, writing bash's
/// error to the redirect-aware writer on failure. Shared by every dup/move
/// redirect-apply site (in-process `apply`/`apply_var` and the child-plan
/// builders). Does NOT check the fd is currently open — the in-process sites
/// add that via `validate_fd_open` (they perform the `dup2` in the parent);
/// the child-plan sites defer the check to child replay. Returns `Err(())`
/// after emitting the error.
fn resolve_dup_source(
    source: &crate::lexer::Word,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<RawFd, ()> {
    match resolve_fd_target(source, shell) {
        Ok(fd) => Ok(fd),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "{}", crate::bash_io_error(&e));
            Err(())
        }
    }
}

/// Check that `src` is currently an open fd; on failure write bash's
/// `N: Bad file descriptor` and return `Err(())`. Used by the in-process
/// dup/move apply sites before the parent-side `dup2`.
fn validate_fd_open(
    src: RawFd,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<(), ()> {
    if unsafe { libc::fcntl(src, libc::F_GETFD) } < 0 {
        let mut err = err_writer(err_sink, sink);
        crate::sh_error_to!(shell, &mut *err, None, "{src}: Bad file descriptor");
        return Err(());
    }
    Ok(())
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
    let exp = glob_expand_fields_opts(fields, opts, shell);
    if !exp.failglob_unmatched.is_empty() {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "no match: {}",
            exp.failglob_unmatched.join(" ")
        );
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
        crate::sh_error_to!(shell, err, None, "command not found:");
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
    Ok(ResolvedCommand {
        program,
        args,
        decl_args,
    })
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
        unsafe {
            libc::close(r);
            libc::close(w);
        }
        return Err(e);
    }
    if pid == 0 {
        // CHILD: async-signal-safe only. Close read end; write the body; _exit.
        // v137: keep SIGPIPE ignored here (the process is otherwise SIG_DFL now)
        // so the writer retains its manual EPIPE handling and closes cleanly,
        // preserving v134 large-heredoc behavior exactly.
        unsafe {
            libc::close(r);
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        }
        let mut off = 0usize;
        while off < bytes.len() {
            let n = unsafe {
                libc::write(
                    w,
                    bytes[off..].as_ptr() as *const libc::c_void,
                    bytes.len() - off,
                )
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
                if errno == libc::EINTR {
                    continue;
                }
                break;
            }
            if n == 0 {
                break;
            }
            off += n as usize;
        }
        unsafe {
            libc::close(w);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(w);
    }
    Ok((r, pid))
}

fn open_resolved(redirect: &ResolvedRedirect) -> io::Result<File> {
    match redirect {
        ResolvedRedirect::Truncate(path) => open_writable(path, false),
        ResolvedRedirect::NoclobberTruncate(path) => open_writable(path, true),
        ResolvedRedirect::Append(path) => OpenOptions::new().create(true).append(true).open(path),
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
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                Some(name),
                "maximum function nesting level exceeded ({limit})"
            );
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
        source: shell
            .function_source
            .get(name)
            .cloned()
            .unwrap_or_else(|| "environment".to_string()),
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
pub(crate) fn call_function_body(name: &str, args: Vec<String>, shell: &mut Shell) -> ExecOutcome {
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
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest: String = chars.collect();
    let level = shell.xtrace_depth + 1;
    let mut out = String::with_capacity(level * first.len_utf8() + rest.len());
    for _ in 0..level {
        out.push(first);
    }
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
                unsafe {
                    libc::close(ps.parent_fd);
                }
            }
            // Remove the FIFO file if present.
            if let Some(ref p) = ps.fifo_path {
                let _ = std::fs::remove_file(p);
            }
            // Best-effort non-blocking reap. If the inner child is still running
            // (e.g. a long-running producer used with a background consumer),
            // skip the wait — it will be reaped by SIGCHLD handling or shell exit.
            let mut status = 0;
            unsafe {
                libc::waitpid(ps.pid, &mut status, libc::WNOHANG);
            }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{name}: readonly variable");
            }
            shell.posix_fatal(127);
            st = 1;
            break;
        }
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
            shell.posix_fatal(127);
            st = 1;
            break;
        }
        // `set -a` / `-o allexport`: a variable assigned here is auto-exported.
        if shell.shell_options.allexport {
            shell.export(name);
        }
        if shell.shell_options.xtrace {
            let val = shell.lookup_var(name).unwrap_or_default();
            let p4 = ps4(shell);
            xtrace_emit(&format!(
                "{p4}{name}={}",
                crate::param_expansion::xtrace_quote(&val)
            ));
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
            .map(|s| builtins::is_declaration_command(&s) || s == "builtin" || s == "command")
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
            if let RedirOp::File {
                mode: crate::command::FileMode::ReadOnly,
                target,
            } = &redir.op
                && redir.target_fd() == Some(0)
            {
                let path = match expand_single(target, shell, &mut *err_writer(err_sink, sink)) {
                    Ok(p) => p,
                    Err(()) => {
                        drain_procsubs(shell, procsub_base);
                        return ExecOutcome::Continue(1);
                    }
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
                        redir_open_error(shell, err_sink, sink, &path, &e);
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
        Err(code) => {
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(code);
        }
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
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(shell, &mut *err, None, "command: {s}: invalid option");
                    }
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
            None => {
                drain_procsubs(shell, procsub_base);
                return ExecOutcome::Continue(0);
            } // `command` / `command -p` alone
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
            None => {
                drain_procsubs(shell, procsub_base);
                return ExecOutcome::Continue(0);
            } // `builtin` alone
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
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "builtin: {}: declaration builtins must not be wrapped by `command builtin`",
                resolved.program
            );
        }
        drain_procsubs(shell, procsub_base);
        return ExecOutcome::Continue(1);
    }
    if require_builtin && !builtins::is_builtin(&resolved.program) {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "builtin: {}: not a shell builtin",
                resolved.program
            );
        }
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
            xtrace_emit(&format!(
                "{p4}{name}={}",
                crate::param_expansion::xtrace_quote(&val)
            ));
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{msg}");
            }
            // exec's variable assignments persist (special builtin), but the
            // #28 child-env scalar overlay must NOT — pop it so a later command
            // doesn't inherit this inline scalar. (inline_scopes isn't pushed on
            // this early-return path, so only the overlay is popped.)
            pop_inline_scalar_overlay(snap.overlay_pushed, shell);
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
        let outcome = run_exec_builtin(&resolved, cmd, shell, sink, err_sink);
        // POSIX case #1: `exec -z` (bad option) exits a non-interactive posix
        // shell. A `command exec`/`builtin exec` wrapper strips it (matches bash).
        // exec returns early here (it never reaches the post-dispatch consume), so
        // mirror that consume on this path, gated identically on a BARE invocation.
        if !wrapped
            && command_prefix.is_empty()
            && !require_builtin
            && let Some(st) = shell.builtin_usage_error.take()
        {
            shell.posix_fatal(st);
        }
        // exec's variable assignments persist, but pop the #28 child-env scalar
        // overlay so it doesn't leak into subsequent commands (redirection-only
        // `exec` continues the shell). See the early-return above.
        pop_inline_scalar_overlay(snap.overlay_pushed, shell);
        drain_procsubs(shell, procsub_base);
        return outcome;
    }

    // Section 3: track this command's snapshotted names on a shell-managed stack
    // so a nested posix special-builtin persist can delete them from enclosing
    // scopes. finalize_inline_scope (every exit path below) pops exactly this.
    shell
        .inline_scopes
        .push(snap.vars.iter().map(|(n, _)| n.clone()).collect());

    if crate::restricted::is_restricted(shell)
        && let Err(msg) = crate::restricted::check_command_name(&resolved.program)
    {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        }
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
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned()
    {
        let name = resolved.program.clone();
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(
                &cmd.redirects,
                shell,
                sink,
                err_sink,
                move |shell, inner_sink, inner_err_sink| {
                    call_function(&name, body, args, shell, inner_sink, inner_err_sink)
                },
            )
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
            with_redirect_scope(
                &cmd.redirects,
                shell,
                sink,
                err_sink,
                move |shell, inner_sink, inner_err_sink| {
                    builtins::eval_in_sink(&args, shell, inner_sink, inner_err_sink)
                },
            )
        } else {
            builtins::eval_in_sink(&args, shell, sink, err_sink)
        }
    } else if resolved.program == "source" || resolved.program == "." {
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(
                &cmd.redirects,
                shell,
                sink,
                err_sink,
                move |shell, inner_sink, inner_err_sink| {
                    builtins::source_in_sink(&args, shell, inner_sink, inner_err_sink)
                },
            )
        } else {
            builtins::source_in_sink(&args, shell, sink, err_sink)
        }
    } else if builtins::builtin_active(&resolved.program, shell) {
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
    if !wrapped
        && command_prefix.is_empty()
        && !require_builtin
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
        shell.procsub_pending.len(),
        procsub_base,
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
    let mut f = ExecFlags {
        clear_env: false,
        login: false,
        argv0: None,
        operand_start: args.len(),
    };
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
const EXEC_RESET_SIGNALS: [libc::c_int; 3] = [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU];

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
        unsafe {
            libc::signal(sig, prev[i]);
        }
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
            unsafe {
                libc::close(saved);
            }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{msg}");
            }
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
        if shell.is_interactive {
            ExecOutcome::Continue(code)
        } else {
            ExecOutcome::Exit(code)
        }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "exec: {name}: {msg}");
            }
            return exit_or_continue(code, shell);
        }
    };

    flush_stdout();
    use std::os::unix::process::CommandExt;
    let mut process = ProcessCommand::new(&prog_path);
    process.args(&operands[1..]);
    // argv[0]: `-a NAME` overrides; `-l` prepends `-`; default is the name as given.
    let base0 = flags.argv0.clone().unwrap_or_else(|| name.clone());
    process.arg0(if flags.login {
        format!("-{base0}")
    } else {
        base0
    });
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
    unsafe {
        restore_exec_signals(saved);
    }

    let code = match err.raw_os_error() {
        Some(libc::ENOENT) => 127,
        _ => 126, // EACCES / ENOEXEC / EISDIR / etc.: "cannot execute".
    };
    {
        let mut errw = err_writer(err_sink, sink);
        crate::sh_error_to!(shell, &mut *errw, None, "exec: {name}: {err}");
    }
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
                    if flags < 0
                        || unsafe { libc::fcntl(target, libc::F_SETFD, flags & !libc::FD_CLOEXEC) }
                            < 0
                    {
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
    let mut plan = ChildRedirPlan {
        ops: Vec::new(),
        held: Vec::new(),
        heredoc_writers: Vec::new(),
    };
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "{name}: ambiguous redirect"
                            );
                        }
                        return Err(1);
                    }
                };
                plan.ops.push(ChildRedirOp::Close { target: fd });
                continue;
            }
            // A Move mirrors the Dup arm to resolve `src`, but the source fd
            // must be closed in the CHILD (a replayed Close op) after the dup
            // lands on `high` — checked once `high` is allocated, below.
            let is_move = matches!(&redir.op, RedirOp::Move { .. });
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
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(1);
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
                                Ok(f) => f,
                                Err(e) => {
                                    redir_open_error(
                                        shell,
                                        err_sink,
                                        sink,
                                        &resolved_path(&resolved),
                                        &e,
                                    );
                                    return Err(1);
                                }
                            }
                        }
                        FileMode::ReadWrite => {
                            match OpenOptions::new()
                                .read(true)
                                .write(true)
                                .create(true)
                                .truncate(false)
                                .open(&path)
                            {
                                Ok(f) => f,
                                Err(e) => {
                                    redir_open_error(shell, err_sink, sink, &path, &e);
                                    return Err(1);
                                }
                            }
                        }
                    };
                    (file.into_raw_fd(), true)
                }
                RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                    // Dup and move resolve the source identically; a move's extra
                    // child-side close is replayed below, gated on `is_move`. Not
                    // "owned" (the shell's fd, not a temp we opened).
                    let src = resolve_dup_source(source, shell, sink, err_sink).map_err(|()| 1)?;
                    (src, false)
                }
                RedirOp::Heredoc { body, .. } => {
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((rfd, pid)) => {
                            plan.heredoc_writers.push(pid);
                            (rfd, true)
                        }
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            return Err(1);
                        }
                    }
                }
                RedirOp::HereString(w) => {
                    let mut bytes = expand_assignment(w, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((rfd, pid)) => {
                            plan.heredoc_writers.push(pid);
                            (rfd, true)
                        }
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            return Err(1);
                        }
                    }
                }
                RedirOp::Close => unreachable!("Close handled above"),
            };
            let high = match alloc_high_fd(src) {
                Ok(h) => h,
                Err(e) => {
                    if owns_src {
                        unsafe { libc::close(src) };
                    }
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "{name}: {}",
                            crate::bash_io_error(&e)
                        );
                    }
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
            plan.ops.push(ChildRedirOp::Dup {
                target: high,
                source: high,
            });
            if is_move {
                // The "move": close the original source fd in the CHILD replay
                // (it has already landed on `high`, which the child inherits).
                plan.ops.push(ChildRedirOp::Close { target: src });
            }
            plan.held.push(unsafe { OwnedFd::from_raw_fd(high) });
            continue;
        }
        let Some(target) = redir.target_fd() else {
            // RedirFd::Var is handled above; any other None is unexpected.
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "ambiguous redirect");
            }
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
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            return Err(1);
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
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(1);
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(1);
                            }
                        }
                    }
                };
                // Relocate above fd 9 so the source never collides with a low
                // explicit-redirect target (e.g. `2>file 3>&2`).
                let raw = relocate_high_cloexec(file.into_raw_fd());
                let owned = unsafe { OwnedFd::from_raw_fd(raw) };
                plan.ops.push(ChildRedirOp::Dup {
                    target,
                    source: raw,
                });
                plan.held.push(owned);
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                // `>&w` / `<&w` (dup), `>&w-` / `<&w-` (move). Resolve the source
                // in the parent; the fd refers to a descriptor the child inherits
                // (e.g. `&1`), so the number is valid in the child after fork. A
                // move also replays a `Close` of the source in the child, except
                // for the degenerate `N>&N-` (source == target), which bash treats
                // as a pure no-op (redir.c's `redir_fd != redirector` guard).
                let is_move = matches!(&redir.op, RedirOp::Move { .. });
                let src = resolve_dup_source(source, shell, sink, err_sink).map_err(|()| 1)?;
                // Degenerate `N>&N-` (source == target) contributes nothing.
                if !(is_move && src == target) {
                    plan.ops.push(ChildRedirOp::Dup {
                        target,
                        source: src,
                    });
                    if is_move {
                        plan.ops.push(ChildRedirOp::Close { target: src });
                    }
                }
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
                        plan.ops.push(ChildRedirOp::Dup {
                            target,
                            source: rfd,
                        });
                        plan.held.push(owned);
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        return Err(1);
                    }
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
                        plan.ops.push(ChildRedirOp::Dup {
                            target,
                            source: rfd,
                        });
                        plan.held.push(owned);
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "heredoc: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        return Err(1);
                    }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "ambiguous redirect");
            }
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
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            return Err(1);
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
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(1);
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(1);
                            }
                        }
                    }
                };
                let raw = relocate_high_cloexec(file.into_raw_fd());
                ops.push(ChildRedirOp::Dup {
                    target,
                    source: raw,
                });
                held.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                // `>&w` (dup) / `>&w-` (move): resolve in the parent (the fd is
                // valid in the child after fork). A move also replays a `Close` of
                // the source in the child, except for the degenerate `N>&N-`
                // (source == target), which bash treats as a pure no-op.
                let is_move = matches!(&redir.op, RedirOp::Move { .. });
                let src = resolve_dup_source(source, shell, sink, err_sink).map_err(|()| 1)?;
                if !(is_move && src == target) {
                    ops.push(ChildRedirOp::Dup {
                        target,
                        source: src,
                    });
                    if is_move {
                        ops.push(ChildRedirOp::Close { target: src });
                    }
                }
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

/// Emit a spawn-failure diagnostic (command-not-found / exec error) for an
/// external command whose redirects were lowered into a CHILD-only replay
/// plan (`ChildRedirPlan`) that never ran (the fork never happened). Since
/// the real fds were never touched, route through the OUTER sink as usual —
/// UNLESS `stderr_follows_stdout` (this command's own trailing `2>&1`, with
/// no later fd-1 override) is set while stdout is captured, in which case
/// write into the stdout capture buffer instead, so a `$(cmd 2>&1)` still
/// sees the diagnostic. Mirrors `run_builtin_with_redirects`'s in-memory
/// `route_err_to_out` (v205/L-25) for this one pre-fork external-command site.
fn emit_exec_spawn_diag(
    shell: &Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    stderr_follows_stdout: bool,
    body: std::fmt::Arguments,
) {
    match sink {
        StdoutSink::Capture(obuf) if stderr_follows_stdout => {
            let mut w = LineDispatchWriter {
                inner: obuf,
                stream: LineStream::Stdout,
            };
            crate::emit_error_to(shell, &mut w, None, body);
        }
        _ => {
            let mut err = err_writer(err_sink, sink);
            crate::emit_error_to(shell, &mut *err, None, body);
        }
    }
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
    unsafe {
        process.pre_exec(reset_job_control_signals_in_child);
    }

    // Replay the ordered redirect ops in the child (AFTER the signal-reset
    // pre_exec). All ops are pure dup2/close (async-signal-safe). On any failure
    // return Err so spawn() fails cleanly.
    let ops = std::mem::take(&mut plan.ops);
    // Detect whether this command's OWN `2>&1` (with no later fd-1 override)
    // would, once replayed, route the child's stderr back onto its stdout.
    // Needed for the spawn-failure diagnostics below: those fire BEFORE any
    // fork happens, so `replay_redir_ops` never runs and the real fds are
    // never touched — a `$(cmd 2>&1)` capture would otherwise miss a
    // command-not-found/exec diagnostic entirely. Mirrors
    // `run_builtin_with_redirects`'s in-memory `route_err_to_out` (v205/L-25).
    let stderr_follows_stdout = {
        let last_2_from_1 = ops
            .iter()
            .rev()
            .find(|op| {
                matches!(
                    op,
                    ChildRedirOp::Dup { target: 2, .. } | ChildRedirOp::Close { target: 2 }
                )
            })
            .is_some_and(|op| matches!(op, ChildRedirOp::Dup { source: 1, .. }));
        let fd1_overridden = ops.iter().any(|op| {
            matches!(
                op,
                ChildRedirOp::Dup { target: 1, .. } | ChildRedirOp::Close { target: 1 }
            )
        });
        last_2_from_1 && !fd1_overridden
    };
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
            let _pid_guard = LiveChildGuard {
                pids: &live_pids,
                pid: pid as libc::pid_t,
            };

            let outcome = if interactive {
                // Race-close: also setpgid in the parent so the child's pgrp
                // is guaranteed to exist before we call tcsetpgrp.
                setpgid_self(pid);
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
                        let line = shell
                            .jobs
                            .iter()
                            .find(|j| j.id == job_id)
                            .map(|j| crate::jobs::notification_line(j, '+'))
                            .unwrap_or_default();
                        {
                            let mut err = err_writer(err_sink, sink);
                            e!(&mut *err, "\n{line}");
                        }
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
                        let code = raw_status_to_exit_code(raw_status, shell);
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
                let pipe_out: RawFd = child.stdout.take().map(|cs| cs.into_raw_fd()).unwrap_or(-1);
                let pipe_err: RawFd = child.stderr.take().map(|cs| cs.into_raw_fd()).unwrap_or(-1);
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
                    unsafe {
                        libc::close(pipe_out);
                    }
                }
                if pipe_err >= 0 {
                    unsafe {
                        libc::close(pipe_err);
                    }
                }
                // We have already reaped the child via waitpid(WNOHANG) inside
                // the loop. Tell `Child` not to reap (or wait on) again — the
                // pid has been collected and would otherwise be -ECHILD.
                std::mem::forget(child);
                match loop_result {
                    Ok(raw_status) => {
                        // Fold the separately-captured stderr bytes into err_sink
                        // now that the stdout &mut borrow is released.
                        if want_capture_err && let StderrSink::Capture(buf) = err_sink {
                            buf.extend_from_slice(&stderr_capture);
                        }
                        let code = raw_status_to_exit_code(raw_status, shell);
                        ExecOutcome::Continue(code)
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "{}: {}",
                                cmd.program,
                                crate::bash_io_error(&e)
                            );
                        }
                        ExecOutcome::Continue(1)
                    }
                }
            };
            // Reap the forked heredoc/herestring writers now the consumer has
            // exited (M-120). They are internal helpers — not jobs, not $!.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe {
                    libc::waitpid(wpid, &mut st, 0);
                }
            }
            outcome
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Spawn failed: reap any heredoc writers so they don't linger.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe {
                    libc::waitpid(wpid, &mut st, 0);
                }
            }
            // bash format: `<src>: line N: <name>: command not found` (the name
            // precedes the phrase; error_prefix supplies the prologue + mode split).
            emit_exec_spawn_diag(
                shell,
                sink,
                err_sink,
                stderr_follows_stdout,
                format_args!("{}: command not found", cmd.program),
            );
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe {
                    libc::waitpid(wpid, &mut st, 0);
                }
            }
            emit_exec_spawn_diag(
                shell,
                sink,
                err_sink,
                stderr_follows_stdout,
                format_args!("{}: {}", cmd.program, crate::bash_io_error(&e)),
            );
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
    // The parser accepts ANY single non-keyword word as the coproc NAME (bash's
    // grammar `coproc WORD compound-command`) and defers the valid-identifier
    // check to here (RUNTIME), matching bash: `coproc @ { :; }` parses, then at
    // runtime prints `` `@': not a valid identifier `` and does NOT start the
    // coprocess (exit status 1, body not run).
    if !crate::builtins::is_valid_name(name) {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "`{}': not a valid identifier", name);
        }
        return ExecOutcome::Continue(1);
    }
    // v157 single-active: warn (but proceed) if a coproc is already live.
    if let Some(existing) = shell.coprocs.first() {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "warning: execute_coproc: coproc [{}:{}] still exists",
                existing.pid,
                existing.name
            );
        }
    }
    // make_pipe() returns (read_end, write_end).
    // pipe_in: shell writes in_w -> coproc reads in_r (its stdin).
    // pipe_out: coproc writes out_w (its stdout) -> shell reads out_r.
    let (in_r, in_w) = match make_pipe() {
        Ok(p) => p,
        Err(e) => {
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
    flush_stdout();
    // Fork the body: child stdin = in_r, stdout = out_w, stderr inherited; its
    // own process group (pgid_target 0); the child must NOT keep the shell ends.
    let pid = match fork_and_run_in_subshell(
        body,
        shell,
        ChildStdio::new(
            unsafe { ChildFd::owned_raw(in_r) },
            unsafe { ChildFd::owned_raw(out_w) },
            ChildFd::Inherit,
        ),
        0,
        &[in_w, out_r],
        None,
        None,
    ) {
        Ok(pid) => pid,
        Err(e) => {
            // in_r / out_w were owned by the moved ChildStdio and already
            // dropped; close only the parent-kept ends here.
            unsafe {
                libc::close(in_w);
                libc::close(out_r);
            }
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
    // Parent: the child ends (in_r/out_w) were closed by the call; relocate the
    // shell ends high + cloexec.
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
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

/// Move `fd` above the stdio range (>= 3) so a freed 0/1/2 (e.g. after
/// `exec <&-`) is never silently reused as a pipeline pipe end, which would
/// alias a stage's std fd onto the pipe (issue #130). Returns `fd` unchanged
/// when it is already >= 3 (the common case). Uses `F_DUPFD` (NOT
/// `F_DUPFD_CLOEXEC`) to keep the moved fd's non-close-on-exec semantics
/// identical to the raw `libc::pipe()` ends the callers dup2/close by hand,
/// then closes the original low fd.
fn move_fd_above_stdio(fd: RawFd) -> io::Result<RawFd> {
    if fd > 2 {
        return Ok(fd);
    }
    let newfd = unsafe { libc::fcntl(fd, libc::F_DUPFD, 3) };
    if newfd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        libc::close(fd);
    }
    Ok(newfd)
}

/// Opens a `libc::pipe()` and returns `(read_end, write_end)` as raw fds, both
/// guaranteed >= 3 so a freed std fd cannot be aliased into a pipeline stage's
/// std fd (issue #130).
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let (r0, w0) = (fds[0], fds[1]);
    let r = match move_fd_above_stdio(r0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r0);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    let w = match move_fd_above_stdio(w0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    Ok((r, w))
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
    unsafe {
        libc::close(w);
    }
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
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "pipe: {}",
                            crate::bash_io_error(&e)
                        );
                    }
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
                if let Some(p) = pos {
                    parent_held.remove(p);
                }
                unsafe {
                    libc::close(r);
                }
            }
            // Run the assignments via fork so they're isolated.
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone(), *aline));
            let pgid_target = if interactive {
                first_pid.unwrap_or(0)
            } else {
                NO_PGROUP
            };
            let stdout_child: ChildFd = if !is_last {
                // Create a pipe; next stage reads from it (will be empty).
                match make_pipe() {
                    Ok((r, w)) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                        unsafe { ChildFd::owned_raw(w) }
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        // Clean up all held fds.
                        return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                    }
                }
            } else {
                match sink {
                    StdoutSink::Capture(_) => match make_pipe() {
                        Ok((r, w)) => {
                            capture_read_fd = Some(r);
                            parent_held.push(r);
                            unsafe { ChildFd::owned_raw(w) }
                        }
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "pipe: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                        }
                    },
                    StdoutSink::Terminal => ChildFd::Inherit,
                }
            };
            let fds_to_close: Vec<RawFd> = parent_held
                .iter()
                .copied()
                .filter(|&fd| Some(fd) != stdout_child.raw())
                .collect();
            match fork_and_run_in_subshell(
                &assign_cmd,
                shell,
                ChildStdio::new(ChildFd::Inherit, stdout_child, ChildFd::Inherit),
                pgid_target,
                &fds_to_close,
                None,
                None,
            ) {
                Ok(pid) => {
                    // The pipe/capture write end was owned by the moved
                    // ChildStdio and closed in the parent by the call.
                    if interactive && first_pid.is_none() {
                        first_pid = Some(pid);
                    }
                    stage_pids.push(pid);
                    live_pids_arc.lock().unwrap().push(pid as libc::pid_t);
                    pipeline_stages.push(PipelineStage::Forked(pid));
                }
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "fork: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return bail_teardown_stage(shell, procsub_base, &mut parent_held);
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
                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
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
        let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.slot_stdin() {
                Some(RedirectSlot::Read(word)) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin, so that pipe would otherwise be leaked
                    // into parent_held, keeping the write-end alive and
                    // deadlocking the previous stage's writer.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    let path = match expand_single(word, shell, &mut *err_writer(err_sink, sink)) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                        }
                    };
                    match File::open(&path) {
                        Ok(f) => ChildFd::from(f),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                        }
                    }
                }
                Some(RedirectSlot::Heredoc { body, .. }) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin via heredoc, so that pipe would otherwise
                    // be leaked into parent_held, deadlocking the previous
                    // stage's writer once the pipe buffer fills.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    // Expand the body NOW while inline assignments are still applied,
                    // then hand it to a forked writer process (M-120): a body larger
                    // than the pipe buffer must not block the parent before the
                    // consumer drains. The parent never holds the write end.
                    let bytes = expand_assignment(body, shell).into_bytes();
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, pid)) => {
                            heredoc_writers.push(pid);
                            unsafe { ChildFd::owned_raw(r) }
                        }
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                        }
                    }
                }
                Some(RedirectSlot::HereString(body)) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin via here-string.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe {
                            libc::close(r);
                        }
                    }
                    // Expand NOW (inline assignments still applied) + trailing newline,
                    // then feed via a forked writer (M-120).
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    match spawn_heredoc_writer(&bytes) {
                        Ok((r, pid)) => {
                            heredoc_writers.push(pid);
                            unsafe { ChildFd::owned_raw(r) }
                        }
                        Err(e) => {
                            {
                                let mut err = err_writer(err_sink, sink);
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                        }
                    }
                }
                _ => {
                    // No explicit stdin redirect: use prev_pipe_read or inherit.
                    match prev_pipe_read.take() {
                        Some(r) => {
                            parent_held.retain(|&fd| fd != r);
                            unsafe { ChildFd::owned_raw(r) }
                        }
                        None => ChildFd::Inherit,
                    }
                }
            }
        } else {
            // Compound command: no explicit stdin at stage level.
            match prev_pipe_read.take() {
                Some(r) => {
                    parent_held.retain(|&fd| fd != r);
                    unsafe { ChildFd::owned_raw(r) }
                }
                None => ChildFd::Inherit,
            }
        };

        // ---- Determine stdout redirect (from ExecCommand if Simple) ----------
        let explicit_stdout: Option<ChildFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stdout() {
                    Some(r @ (RedirectSlot::Truncate(w) | RedirectSlot::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        };
                        let guard =
                            shell.shell_options.noclobber && !matches!(r, RedirectSlot::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        }
                    }
                    Some(RedirectSlot::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        };
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

        // ---- Determine stderr redirect (from ExecCommand if Simple) ----------
        let explicit_stderr: Option<ChildFd> =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                match &exec.slot_stderr() {
                    Some(r @ (RedirectSlot::Truncate(w) | RedirectSlot::Clobber(w))) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        };
                        let guard =
                            shell.shell_options.noclobber && !matches!(r, RedirectSlot::Clobber(_));
                        match open_writable(&path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        }
                    }
                    Some(RedirectSlot::Append(w)) => {
                        let path = match expand_single(w, shell, &mut *err_writer(err_sink, sink)) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        };
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                restore_inline_assignments(snap, shell);
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
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
        let stdout: ChildFd = if let Some(cf) = explicit_stdout {
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
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        // stdin / cf (the open file) / explicit_stderr drop here.
                        drain_procsubs(shell, procsub_base);
                        for pfd in parent_held.drain(..) {
                            unsafe {
                                libc::close(pfd);
                            }
                        }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            cf
        } else if !is_last {
            // Create the inter-stage pipe.
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    // w is owned by the child's stdout ChildFd (not parent_held).
                    unsafe { ChildFd::owned_raw(w) }
                }
                Err(e) => {
                    {
                        let mut err = err_writer(err_sink, sink);
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "pipe: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    restore_inline_assignments(snap, shell);
                    // stdin / explicit_stderr drop here.
                    return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                }
            }
        } else {
            match sink {
                StdoutSink::Capture(_) => match make_pipe() {
                    Ok((r, w)) => {
                        capture_read_fd = Some(r);
                        parent_held.push(r);
                        unsafe { ChildFd::owned_raw(w) }
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        // stdin / explicit_stderr drop here.
                        return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                    }
                },
                StdoutSink::Terminal => ChildFd::Inherit,
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
        let stderr: ChildFd = if let Some(cf) = explicit_stderr {
            cf
        } else {
            match err_sink {
                StderrSink::Terminal => ChildFd::Inherit,
                // Kernel-level 2>&1: stderr := a distinct dup of whatever stdout
                // resolves to (a real fd 1 when stdout inherits, else a clone of
                // the owned pipe/file). A clone — never an alias of the same fd —
                // so no fd is double-owned (the §5 double-OwnedFd fix).
                // SAFETY: STDOUT_FILENO is always a live shell std fd.
                StderrSink::Merged => match stdout.try_clone_resolving(libc::STDOUT_FILENO) {
                    Ok(c) => c,
                    Err(e) => {
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "dup: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        // stdin / stdout drop here.
                        return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                    }
                },
                StderrSink::Capture(_) => {
                    // dup the shared capture_err write-end PER STAGE (each stage
                    // owns its own copy; the shared original stays open until the
                    // pipeline finishes).
                    let shared = capture_err_pipe_write_fd
                        .expect("capture_err_pipe_write_fd set when err_sink is Capture");
                    let fd = unsafe { libc::dup(shared) };
                    if fd < 0 {
                        let e = io::Error::last_os_error();
                        {
                            let mut err = err_writer(err_sink, sink);
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "dup: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        // stdin / stdout drop here; capture_read_fd stays in
                        // parent_held and is closed by bail_teardown_stage.
                        return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                    }
                    unsafe { ChildFd::owned_raw(fd) }
                }
            }
        };

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if interactive {
            first_pid.unwrap_or(0)
        } else {
            NO_PGROUP
        };

        // parent_fds_to_close: all fds the parent currently holds that the
        // child must close (so it doesn't hold downstream pipe write-ends open,
        // which would prevent EOF propagation). We exclude the fds being passed
        // to this stage as stdio (those are the child's to keep). The heredoc
        // pipe's write end lives in the forked writer process, not here, so
        // there is nothing extra to add.
        let fds_to_close_in_child: Vec<RawFd> = parent_held
            .iter()
            .copied()
            .filter(|&fd| {
                Some(fd) != stdin.raw() && Some(fd) != stdout.raw() && Some(fd) != stderr.raw()
            })
            .collect();

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.slot_stdout() {
                    Some(RedirectSlot::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                {
                                    let mut err = err_writer(err_sink, sink);
                                    crate::sh_error_to!(
                                        shell,
                                        &mut *err,
                                        None,
                                        "{}",
                                        crate::bash_io_error(&e)
                                    );
                                }
                                restore_inline_assignments(snap, shell);
                                // stdin / stdout / stderr ChildFds drop on return;
                                // capture_read_fd stays in parent_held (drained).
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.slot_stderr() {
                    Some(RedirectSlot::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                {
                                    let mut err = err_writer(err_sink, sink);
                                    crate::sh_error_to!(
                                        shell,
                                        &mut *err,
                                        None,
                                        "{}",
                                        crate::bash_io_error(&e)
                                    );
                                }
                                restore_inline_assignments(snap, shell);
                                // stdin / stdout / stderr ChildFds drop on return;
                                // capture_read_fd stays in parent_held (drained).
                                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
                            }
                        }
                    }
                    _ => None,
                };
                (sdt, sedt)
            } else {
                (None, None)
            };

        // Build the child's fd environment ONCE; move it into whichever spawner.
        // Both spawners consume the ChildStdio and close the parent's owned
        // copies (on success AND error — RAII), so there is no post-spawn parent
        // close bookkeeping. The inter-stage pipe WRITE end is owned by `stdout`
        // (never entered parent_held); the READ end stays in parent_held for the
        // next stage to consume.
        let child_stdio = ChildStdio::new(stdin, stdout, stderr);
        let spawn_result = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => spawn_external_with_fds(
                simple,
                shell,
                sink,
                err_sink,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
            ),
            StageKind::InProcess(cmd) => fork_and_run_in_subshell(
                cmd,
                shell,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
                stdout_dup_target,
                stderr_dup_target,
            ),
        };

        // ---- Restore inline assignments (v23 scoping) -----------------------
        restore_inline_assignments(snap, shell);

        let pid = match spawn_result {
            Ok(p) => p,
            Err(e) => {
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(shell, &mut *err, None, "{}", crate::bash_io_error(&e));
                }
                // child_stdio (with all owned stdio fds) was consumed by the
                // failed call and already dropped. Close every remaining
                // parent-held fd (inter-stage read ends + capture read ends).
                drain_procsubs(shell, procsub_base);
                for fd in parent_held.drain(..) {
                    unsafe {
                        libc::close(fd);
                    }
                }
                return ExecOutcome::Continue(1);
            }
        };

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
            unsafe {
                libc::close(fd);
            }
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
        unsafe {
            libc::close(pipe_out);
        }
    }
    if pipe_err >= 0 {
        unsafe {
            libc::close(pipe_err);
        }
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
    let last_status = wait_pipeline_raw(
        &pipeline_stages,
        &stage_pids,
        first_pid,
        shell,
        sink,
        err_sink,
        interactive,
    );

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
        unsafe {
            libc::waitpid(wpid, &mut st, 0);
        }
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
                        if slot.is_none() {
                            *slot = Some(1);
                        }
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
                    {
                        let mut err = err_writer(err_sink, sink);
                        e!(&mut *err, "\n{line}");
                    }
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
                    if errno == libc::EINTR {
                        continue;
                    }
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
/// Snapshot of the variable state an inline-prefix scope must restore, plus
/// bookkeeping for the `#28` child-env scalar overlay.
#[derive(Debug)]
struct AssignmentSnapshot {
    /// `(name, prior)` per snapshotted variable, restored LIFO.
    vars: Vec<(String, Option<crate::shell_state::Variable>)>,
    /// How many entries this apply pushed onto `shell.inline_scalar_export`,
    /// truncated (LIFO) by restore/finalize. See #28.
    overlay_pushed: usize,
}

/// Expands and applies `assignments` left-to-right, exporting each, and
/// returns a snapshot the caller can pass to `restore_inline_assignments`
/// (for temporary-scope targets) or discard (for persistent-scope targets).
fn apply_inline_assignments(
    assignments: &[crate::command::Assignment],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<AssignmentSnapshot, AssignmentSnapshot> {
    let mut snap = AssignmentSnapshot {
        vars: Vec::with_capacity(assignments.len()),
        overlay_pushed: 0,
    };
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
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(shell, &mut *err, None, "{name}: readonly variable");
            }
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
            // #28: `exported_env` omits persistent exported arrays, but an
            // inline-prefix SCALAR assignment must still reach the child as that
            // scalar even when the target variable is array-typed (bash exports
            // the inline scalar). Record its scalar view so `exported_env`
            // re-adds it; the overlay is popped when this scope is restored.
            if shell.get_indexed(&snap_name).is_some()
                || shell.get_associative(&snap_name).is_some()
            {
                let sv = shell.get(&snap_name).unwrap_or_default().to_string();
                shell.inline_scalar_export.push((snap_name.clone(), sv));
                snap.overlay_pushed += 1;
            }
        }
        snap.vars.push((snap_name, prior));
    }
    Ok(snap)
}

/// Restores each snapshot entry in reverse order, so repeated names
/// unwind LIFO and end up at their pre-prefix value.
fn restore_inline_assignments(snap: AssignmentSnapshot, shell: &mut Shell) {
    pop_inline_scalar_overlay(snap.overlay_pushed, shell);
    for (name, prior) in snap.vars.into_iter().rev() {
        shell.restore_var(&name, prior);
    }
}

/// Truncate the `#28` inline-scalar child-env overlay by the `count` entries a
/// paired `apply_inline_assignments` pushed (LIFO).
fn pop_inline_scalar_overlay(count: usize, shell: &mut Shell) {
    let new_len = shell.inline_scalar_export.len().saturating_sub(count);
    shell.inline_scalar_export.truncate(new_len);
}

/// Pops the top `inline_scopes` entry pushed by `run_exec_single` and finalizes
/// this command's inline assignments. NON-persistent: restore LIFO, but skip any
/// name a nested posix special-builtin persist deleted from this scope.
/// PERSISTENT (posix special builtin / export / readonly): keep the live values
/// and delete these names from every enclosing scope so their restores skip them.
fn finalize_inline_scope(snap: AssignmentSnapshot, persistent: bool, shell: &mut Shell) {
    pop_inline_scalar_overlay(snap.overlay_pushed, shell);
    let kept = shell.inline_scopes.pop().unwrap_or_default();
    if persistent {
        // Only POSIX mode propagates a persist THROUGH an enclosing
        // temp-assignment scope. In default mode export/readonly keep their
        // value at their own level (snap not restored here) but must NOT
        // survive an enclosing same-name restore — so skip the deletion.
        if shell.shell_options.posix {
            for (name, _) in &snap.vars {
                for scope in shell.inline_scopes.iter_mut() {
                    scope.remove(name);
                }
            }
        }
    } else {
        for (name, prior) in snap.vars.into_iter().rev() {
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
        && let crate::lexer::WordPart::Literal {
            text,
            quoted: false,
        } = &word.0[0]
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
    matches!(word.0.last(), Some(crate::lexer::WordPart::ArrayLiteral(_)))
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
                        crate::sh_error_to!(shell, err, None, "{target_name}: readonly variable");
                        return Err(());
                    }
                    let new_pairs = build_associative_map(elements, shell, err)?;
                    for (k, v, append) in new_pairs {
                        if append {
                            // `a+=([k]+=v)`: concatenate onto the existing
                            // element (bash: `[k]=base` + `[k]+=y` → `basey`).
                            shell
                                .append_associative_element(name, &k, &v)
                                .map_err(|_| ())?;
                        } else {
                            shell.set_associative_element(name, k, v).map_err(|_| ())?;
                        }
                    }
                    return Ok(());
                } else {
                    let pairs = build_associative_map(elements, shell, err)?
                        .into_iter()
                        .map(|(k, v, _)| (k, v))
                        .collect();
                    return shell.replace_associative(name, pairs).map_err(|_| ());
                }
            }
            (AssignTarget::Bare(name), None) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "{name}: {} not valid on associative array",
                    if a.append {
                        "scalar append"
                    } else {
                        "scalar assignment"
                    }
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
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "{name}: cannot assign array literal to associative array element"
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
                    crate::sh_error_to!(shell, err, None, "{name}: readonly variable");
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
                let map = expand_array_elements(elements, name, shell, start, true, err)?;
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
                    Some(_) => shell.append_indexed_element(name, 0, &s).map_err(|_| ()),
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
                    crate::sh_error_to!(shell, err, None, "{msg}");
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
                    shell
                        .set_indexed_element(name, idx, (cur + add).to_string())
                        .map_err(|_| ())
                } else {
                    shell.append_indexed_element(name, idx, &v).map_err(|_| ())
                }
            } else {
                shell.set_indexed_element(name, idx, v).map_err(|_| ())
            }
        }
        // Subscripted lvalue + compound array RHS: bash rejects this.
        (AssignTarget::Indexed { name, .. }, Some(_)) => {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "{name}: cannot assign array literal to array element"
            );
            Err(())
        }
    }
}

/// Builds an associative-array initializer from the compound literal's
/// elements. Each element MUST have an explicit subscript ([key]=value);
/// positional elements (no subscript) are an error.
/// Each returned triple is `(key, value, append)`. `append` is `true` for a
/// `[key]+=value` element and is honored ONLY by the `a+=(…)` append context
/// (append to the pre-existing element). In a fresh `declare -A a=(…)` replace,
/// bash treats `[k]+=v` like `[k]=v` — no concat with an earlier same-key
/// element in the literal (e.g. `([k]=x [k]+=y)` → `y`, not `xy`) — which the
/// key-dedup below already reproduces.
fn build_associative_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<Vec<(String, String, bool)>, ()> {
    let mut out: Vec<(String, String, bool)> = Vec::new();
    for elem in elements {
        let key = match &elem.subscript {
            Some(sw) => crate::expand::eval_subscript_key(sw, shell),
            None => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "associative array initializer requires [key]=value form"
                );
                return Err(());
            }
        };
        let val = crate::param_expansion::expand_word_to_string(&elem.value, shell);
        if let Some(slot) = out.iter_mut().find(|(k, _, _)| k == &key) {
            slot.1 = val;
            slot.2 = elem.append;
        } else {
            out.push((key, val, elem.append));
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
    consult_existing: bool,
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
                        crate::sh_error_to!(shell, err, None, "{msg}");
                        return Err(());
                    }
                };
                let add = expand_assignment(&elem.value, shell);
                let value = if elem.append {
                    // `[i]+=v`: append to the current value of element `i` —
                    // whatever an earlier element in THIS literal set (map),
                    // or (only for `a+=(…)` append context) the pre-existing
                    // array element. A plain `a=(…)` replace discards the old
                    // array, so it never consults it. Integer-flagged arrays
                    // use arithmetic `+=`, matching a standalone `a[i]+=v`.
                    let base = map
                        .get(&idx)
                        .cloned()
                        .or_else(|| {
                            if consult_existing {
                                shell.lookup_indexed_element(name, idx)
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    if shell.is_integer(name) {
                        let cur = arith_eval_operand(&base, shell).unwrap_or(0);
                        let addv = arith_eval_operand(&add, shell).unwrap_or(0);
                        (cur + addv).to_string()
                    } else {
                        base + &add
                    }
                } else {
                    add
                };
                map.insert(idx, value);
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
    expand_array_elements(elements, name, shell, 0, false, err)
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
/// child. Used for `RedirectSlot::Dup` (e.g. `2>&1`). Resolution happens in
/// the parent (pre-fork) so this is always an i32, never a Word.
#[allow(clippy::too_many_arguments)]
pub fn fork_and_run_in_subshell(
    cmd: &Command,
    shell: &mut Shell,
    stdio: ChildStdio,
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
            // 3-5. Install stdio from ChildStdio. Convert to raw NOW so no
            // OwnedFd destructor can run in the forked child (Drop safety).
            let ChildStdio {
                stdin,
                stdout,
                stderr,
            } = stdio;
            let mut plan: [(Option<RawFd>, RawFd); 3] = [
                (stdin.into_raw(), 0),
                (stdout.into_raw(), 1),
                (stderr.into_raw(), 2),
            ];
            let original_raws: [RawFd; 3] = {
                // fd numbers this child owns as stdio sources, -1 for Inherit.
                [
                    plan[0].0.unwrap_or(-1),
                    plan[1].0.unwrap_or(-1),
                    plan[2].0.unwrap_or(-1),
                ]
            };
            // Pass 1 (PRE-MOVE): move any owned source in 0..=2 up to >=3, so
            // pass 2's dup2 always has source != target (clears FD_CLOEXEC ->
            // the §H2b fix) and installs are order-independent. F_DUPFD (not
            // _CLOEXEC): the moved copy must survive exec if its install no-ops.
            for (src, _) in plan.iter_mut() {
                if let Some(s) = *src
                    && s <= 2
                {
                    let moved = libc::fcntl(s, libc::F_DUPFD, 3);
                    if moved >= 0 {
                        libc::close(s);
                        *src = Some(moved);
                    }
                    // On failure keep s: degraded to old behavior, never worse.
                }
            }
            // Pass 2 (INSTALL): sources now all >=3 and pairwise distinct.
            for (src, slot) in plan {
                if let Some(s) = src {
                    libc::dup2(s, slot);
                    libc::close(s);
                }
            }
            // Pass 3: close every parent-held pipe fd, skipping this child's own
            // stdio sources by their ORIGINAL numbers.
            for &fd in parent_fds_to_close {
                if fd != original_raws[0] && fd != original_raws[1] && fd != original_raws[2] {
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
///   - AND NOT an active builtin (`builtins::builtin_active`).
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
        && !builtins::builtin_active(&prog, shell)
    {
        return StageKind::External(simple);
    }
    StageKind::InProcess(cmd)
}

/// Spawns an external command with a pre-built `ChildStdio`.
///
/// Consumes `stdio`: each `ChildFd::Owned` slot becomes a `Stdio` (ownership
/// transferred — `std::process::Command` closes it in the parent after fork),
/// and each `ChildFd::Inherit` slot uses `Stdio::inherit()`. On every early
/// return the un-consumed `stdio` drops, closing the parent's owned fds (#78).
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
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    // Flush pending parent stdout before spawning an external stage so its output
    // does not race ahead of buffered parent bytes (M-118 sibling: ordering).
    flush_stdout();
    use std::os::unix::process::CommandExt;

    let SimpleCommand::Exec(exec) = cmd else {
        // Assign-only stages are classified as InProcess by classify_stage;
        // reaching here is a caller bug.
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "spawn_external_with_fds called on Assign stage",
        ));
    };

    // Resolve (expand) the command — same path as run_exec_single / run_multi_stage.
    let resolved = resolve(exec, shell, &mut *err_writer(err_sink, sink))
        .map_err(|code| io::Error::other(format!("resolve failed with code {code}")))?;

    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(&format!(
            "{p4}{}",
            xtrace_command_line(&[], &resolved.program, &resolved.args)
        ));
    }

    // Resolve Dup targets pre-fork (Word expansion may allocate; not async-signal-safe).
    // stdout-dup BEFORE stderr-dup matches canonical `>file 2>&1` semantics.
    let stdout_dup_target: Option<i32> = match &exec.slot_stdout() {
        Some(RedirectSlot::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
        _ => None,
    };
    let stderr_dup_target: Option<i32> = match &exec.slot_stderr() {
        Some(RedirectSlot::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
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
    unsafe {
        process.pre_exec(reset_job_control_signals_in_child);
    }

    // If there are Dup redirects, chain a second pre_exec to apply dup2 in the
    // child. This runs AFTER the signal-reset pre_exec (registration order).
    // stdout-dup BEFORE stderr-dup matches canonical `>file 2>&1` semantics.
    if stdout_dup_target.is_some() || stderr_dup_target.is_some() {
        unsafe {
            process.pre_exec(move || {
                if let Some(fd) = stdout_dup_target
                    && libc::dup2(fd, 1) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                if let Some(fd) = stderr_dup_target
                    && libc::dup2(fd, 2) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    // Collect the target fds that extra_ops will set up in the child, so we
    // can exclude them from fds_to_close below (Fix B: a dup2 then close on
    // the same fd would silently defeat the redirect).
    let extra_targets: Vec<RawFd> = extra_ops
        .iter()
        .map(|op| match *op {
            ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target,
        })
        .collect();

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

    // Convert the ChildStdio to std Stdio. An `Inherit` slot leaves the shell's
    // real fd alone; an `Owned` slot transfers the OwnedFd to Stdio (std closes
    // it in the parent after fork). For a Dup-redirect slot, use Stdio::inherit()
    // and drop the owned fd (if any) — the dup2 pre_exec applies the real
    // redirect in the child; dropping closes the parent's copy.
    let ChildStdio {
        stdin,
        stdout,
        stderr,
    } = stdio;
    let stdin_stdio = match stdin {
        ChildFd::Inherit => Stdio::inherit(),
        ChildFd::Owned(fd) => Stdio::from(fd),
    };
    let stdout_stdio = if stdout_dup_target.is_some() {
        // Dup on stdout: inherit so the dup2 pre_exec redirects to target.
        // Dropping the owned fd (if any) closes the parent's copy.
        drop(stdout);
        Stdio::inherit()
    } else {
        match stdout {
            ChildFd::Inherit => Stdio::inherit(),
            ChildFd::Owned(fd) => Stdio::from(fd),
        }
    };
    let stderr_stdio = if stderr_dup_target.is_some() {
        drop(stderr);
        Stdio::inherit()
    } else {
        match stderr {
            ChildFd::Inherit => Stdio::inherit(),
            ChildFd::Owned(fd) => Stdio::from(fd),
        }
    };

    process.stdin(stdin_stdio);
    process.stdout(stdout_stdio);
    process.stderr(stderr_stdio);

    // In the child's pre_exec, close every parent-held pipe fd that this
    // child shouldn't inherit (so downstream readers see EOF).
    // Exclude any fd that extra_ops already claimed as a redirect target: a
    // dup2 into that fd followed by close(fd) would silently defeat the redirect.
    // The closure must be async-signal-safe; libc::close is.
    let fds_to_close: Vec<RawFd> = parent_fds_to_close
        .iter()
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
mod tests;

#[cfg(test)]
mod array_assign_tests;

#[cfg(test)]
mod assoc_assign_tests;

#[cfg(test)]
mod coproc_name_tests;

#[cfg(test)]
mod arith_for_tests;

#[cfg(test)]
mod loop_levels_executor_tests;

#[cfg(test)]
mod select_menu_tests;

#[cfg(test)]
mod g3_dbracket_extglob_noshopt_tests;

#[cfg(test)]
mod errexit_andor_tests;
