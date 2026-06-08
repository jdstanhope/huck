use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::io::RawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{
    CaseClause, CaseItem, CaseTerminator, Command, Connector, ExecCommand, ForClause, IfClause,
    Pipeline, Redirect, Sequence, SimpleCommand, TestBinaryOp, TestExpr, TestUnaryOp, WhileClause,
};
use crate::expand::{expand, expand_assignment, expand_pattern, glob_expand_fields_opts};
use crate::shell_state::Shell;

/// Where the terminal stage of a top-level pipeline sends its stdout when
/// there's no explicit `> file` redirect.
pub enum StdoutSink<'a> {
    Terminal,
    Capture(&'a mut Vec<u8>),
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

pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
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
                return run_background_sequence(p, shell, &mut sink, source);
            }
            if let Command::Subshell { .. } = &seq.first {
                return run_background_subshell(&seq.first, shell, &mut sink, source);
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
            return run_background_subshell(&subshell, shell, &mut sink, source);
        }
    }
    execute_sequence_body(seq, shell, &mut sink)
}

/// Runs a sequence with stdout captured to a buffer. Used by command
/// substitution; the trailing `&` is ignored here because substitutions
/// must complete before their output is interpolated. Spawning real
/// background children whose pids the parent's JobTable doesn't track
/// would let them escape `wait`/`jobs` and litter the terminal.
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
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
        execute_sequence_body(&sanitized, shell, &mut sink)
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::FunctionReturn(n) => n,
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
) -> ExecOutcome {
    let mut status = run_command(first, shell, sink);
    if matches!(
        status,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_)
    ) {
        return status;
    }
    // B-11: propagate `$?` across sequence connectors. The top-level loop
    // in shell.rs only refreshes `shell.last_status` after `process_line`
    // returns, so without this update the second command in `false; echo $?`
    // would see a stale value.
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_pe_error.is_some() {
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
            status = run_command(command, shell, sink);
            if matches!(
                status,
                ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_)
            ) {
                return status;
            }
            if let ExecOutcome::Continue(c) = status {
                shell.set_last_status(c);
                if shell.pending_fatal_pe_error.is_some() {
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

fn execute_sequence_body(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
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
            run_background_subshell(&subshell, shell, sink, &source);
        } else {
            last_status = run_andor_group(group.first, &group.rest, shell, sink);
            // Propagate control-flow outcomes immediately.
            if matches!(
                last_status,
                ExecOutcome::Exit(_)
                    | ExecOutcome::LoopBreak(_, _)
                    | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_)
            ) {
                return last_status;
            }
        }
    }
    last_status
}

/// Dispatches a single sequence element.
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::Simple(s) => run_single(s, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
        Command::For(clause) => run_for(clause, shell, sink),
        Command::Case(clause) => run_case(clause, shell, sink),
        Command::BraceGroup(seq) => execute_sequence_body(seq, shell, sink),
        Command::Subshell { .. } => {
            // Determine stdout fd for the child.  For Terminal (the common
            // case) we pass STDOUT_FILENO directly.  For Capture we create a
            // pipe so the parent can read the child's output back into the
            // capture buffer after the child exits.
            let (stdout_fd, capture_read_fd): (RawFd, Option<RawFd>) = match sink {
                StdoutSink::Terminal => (libc::STDOUT_FILENO, None),
                StdoutSink::Capture(_) => match make_pipe() {
                    Ok((r, w)) => (w, Some(r)),
                    Err(e) => {
                        eprintln!("huck: pipe: {e}");
                        return ExecOutcome::Continue(1);
                    }
                },
            };

            let pid = match fork_and_run_in_subshell(
                cmd,
                shell,
                libc::STDIN_FILENO,
                stdout_fd,
                libc::STDERR_FILENO,
                0,
                &[],
                None, // no Dup redirect at this call site
                None,
            ) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("huck: fork: {e}");
                    if let Some(r) = capture_read_fd {
                        unsafe { libc::close(r); }
                    }
                    if stdout_fd != libc::STDOUT_FILENO {
                        unsafe { libc::close(stdout_fd); }
                    }
                    return ExecOutcome::Continue(1);
                }
            };

            // Close the write-end in the parent so the child's write-end is
            // the only writer; once the child exits, the read-end sees EOF.
            if stdout_fd != libc::STDOUT_FILENO {
                unsafe { libc::close(stdout_fd); }
            }

            // Drain capture pipe before waitpid to avoid deadlock.
            if let (Some(r), StdoutSink::Capture(buf)) = (capture_read_fd, &mut *sink) {
                use std::os::fd::FromRawFd;
                let mut f = unsafe { File::from_raw_fd(r) };
                let _ = io::copy(&mut f, *buf);
                // f is dropped here, closing r.
            }

            // Wait for the child.
            let mut raw_status: libc::c_int = 0;
            let r = unsafe { libc::waitpid(pid, &mut raw_status, 0) };
            if r < 0 {
                return ExecOutcome::Continue(1);
            }
            let code = if libc::WIFEXITED(raw_status) {
                libc::WEXITSTATUS(raw_status)
            } else if libc::WIFSIGNALED(raw_status) {
                128 + libc::WTERMSIG(raw_status)
            } else {
                1
            };
            // A subshell is one forked unit → 1-element PIPESTATUS.
            shell.set_pipestatus(&[code]);
            ExecOutcome::Continue(code)
        }
        Command::FunctionDef { name, body } => {
            shell.functions.insert(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
        Command::DoubleBracket { expr, inline_assignments } => {
            run_double_bracket(expr, inline_assignments, shell)
        }
        Command::ArithFor(clause) => run_arith_for(clause, shell, sink),
        Command::Arith(expr) => run_arith(expr, shell),
        Command::Select(clause) => run_select(clause, shell, sink),
        Command::Redirected { inner, stdin, stdout, stderr } => {
            run_redirected(inner, stdin, stdout, stderr, shell, sink)
        }
    }
}

/// RAII guard that records fds saved (via `dup`) before they were replaced by
/// `dup2`, and restores each on Drop. `saved` holds `(target_fd, saved_dup_fd)`
/// pairs; restoration runs in reverse to undo overlapping swaps cleanly.
struct CompoundRedirectScope {
    saved: Vec<(RawFd, RawFd)>,
}

impl CompoundRedirectScope {
    fn new() -> Self {
        CompoundRedirectScope { saved: Vec::new() }
    }

    /// Replace `target_fd` with a dup of `new_fd`, saving the original so Drop
    /// can restore it. `new_fd` is NOT consumed (caller closes it).
    fn redirect(&mut self, new_fd: RawFd, target_fd: RawFd) -> Result<(), ()> {
        unsafe {
            let saved = libc::dup(target_fd);
            if saved < 0 {
                eprintln!("huck: dup: {}", io::Error::last_os_error());
                return Err(());
            }
            if libc::dup2(new_fd, target_fd) < 0 {
                eprintln!("huck: dup2: {}", io::Error::last_os_error());
                libc::close(saved);
                return Err(());
            }
            self.saved.push((target_fd, saved));
        }
        Ok(())
    }
}

impl Drop for CompoundRedirectScope {
    fn drop(&mut self) {
        // Flush any buffered stdout written through the redirected fd before we
        // swap the original fd back, so it lands in the redirect target.
        let _ = io::stdout().flush();
        // Restore in reverse application order.
        while let Some((target_fd, saved)) = self.saved.pop() {
            unsafe {
                libc::dup2(saved, target_fd);
                libc::close(saved);
            }
        }
    }
}

/// Runs a compound command with trailing redirections applied at the real fd
/// level: each present redirect is `dup2`'d onto fd 0/1/2 (originals saved and
/// restored on scope exit), `inner` runs through the existing `sink`, and its
/// status is returned. A redirect-open failure prints `huck: <target>: <err>`
/// and returns `Continue(1)` WITHOUT running `inner`.
fn run_redirected(
    inner: &Command,
    stdin: &Option<Redirect>,
    stdout: &Option<Redirect>,
    stderr: &Option<Redirect>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    use std::os::unix::io::IntoRawFd;

    // Flush buffered terminal/builtin output BEFORE swapping fds so prior
    // output is not diverted into the redirect target.
    let _ = io::stdout().flush();

    let mut scope = CompoundRedirectScope::new();

    // --- stdin (fd 0) ---
    if let Some(r) = stdin {
        let new_fd: RawFd = match r {
            Redirect::Read(word) => {
                let path = match expand_single(word, shell) {
                    Ok(p) => p,
                    Err(()) => return ExecOutcome::Continue(1),
                };
                match File::open(&path) {
                    Ok(f) => f.into_raw_fd(),
                    Err(e) => {
                        eprintln!("huck: {path}: {e}");
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            Redirect::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                match write_pipe_for_stdin(&bytes) {
                    Ok(fd) => fd,
                    Err(()) => return ExecOutcome::Continue(1),
                }
            }
            Redirect::HereString(body) => {
                let mut bytes = expand_assignment(body, shell).into_bytes();
                bytes.push(b'\n');
                match write_pipe_for_stdin(&bytes) {
                    Ok(fd) => fd,
                    Err(()) => return ExecOutcome::Continue(1),
                }
            }
            // `<&N` on a compound is out of scope; only `<file`/heredoc/
            // here-string reach the stdin slot from the parser.
            Redirect::Truncate(_) | Redirect::Append(_) | Redirect::Dup { .. } => {
                eprintln!("huck: unsupported stdin redirect on compound");
                return ExecOutcome::Continue(1);
            }
        };
        if scope.redirect(new_fd, libc::STDIN_FILENO).is_err() {
            unsafe { libc::close(new_fd) };
            return ExecOutcome::Continue(1);
        }
        unsafe { libc::close(new_fd) };
    }

    // --- stdout (fd 1) ---
    if let Some(r) = stdout {
        match apply_out_redirect(r, libc::STDOUT_FILENO, &mut scope, shell) {
            Ok(()) => {}
            Err(outcome) => return outcome,
        }
    }

    // --- stderr (fd 2) ---
    if let Some(r) = stderr {
        match apply_out_redirect(r, libc::STDERR_FILENO, &mut scope, shell) {
            Ok(()) => {}
            Err(outcome) => return outcome,
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
    let inner_sink: &mut StdoutSink = if stdout.is_some() {
        &mut terminal_sink
    } else {
        sink
    };
    let outcome = run_command(inner, shell, inner_sink);
    let _ = io::stdout().flush();
    drop(scope);
    outcome
}

/// Apply a stdout/stderr-class redirect (`>`/`>>`/`>&N`/`2>&N`) onto
/// `target_fd`, recording the swap in `scope`. On open/resolve failure prints
/// the bash-style error and returns `Err(Continue(1))`.
fn apply_out_redirect(
    r: &Redirect,
    target_fd: RawFd,
    scope: &mut CompoundRedirectScope,
    shell: &mut Shell,
) -> Result<(), ExecOutcome> {
    use std::os::unix::io::IntoRawFd;
    match r {
        Redirect::Truncate(word) | Redirect::Append(word) => {
            let path = match expand_single(word, shell) {
                Ok(p) => p,
                Err(()) => return Err(ExecOutcome::Continue(1)),
            };
            let resolved = if matches!(r, Redirect::Append(_)) {
                ResolvedRedirect::Append(path)
            } else {
                ResolvedRedirect::Truncate(path)
            };
            let file = match open_resolved(&resolved) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("huck: {}: {e}", resolved_path(&resolved));
                    return Err(ExecOutcome::Continue(1));
                }
            };
            let new_fd = file.into_raw_fd();
            if scope.redirect(new_fd, target_fd).is_err() {
                unsafe { libc::close(new_fd) };
                return Err(ExecOutcome::Continue(1));
            }
            unsafe { libc::close(new_fd) };
            Ok(())
        }
        Redirect::Dup { source, .. } => {
            // `>&N` / `2>&N`: duplicate the source fd onto target_fd. Resolve
            // the source AFTER any earlier fd swaps so e.g. `>file 2>&1` makes
            // stderr follow the already-redirected stdout (last-wins ordering
            // matches the parser slot fill).
            let src = match resolve_fd_target(source, shell) {
                Ok(fd) => fd,
                Err(e) => {
                    eprintln!("huck: {e}");
                    return Err(ExecOutcome::Continue(1));
                }
            };
            if scope.redirect(src, target_fd).is_err() {
                return Err(ExecOutcome::Continue(1));
            }
            Ok(())
        }
        Redirect::Read(_) | Redirect::Heredoc { .. } | Redirect::HereString(_) => {
            // The parser never routes these to the stdout/stderr slots.
            eprintln!("huck: unsupported output redirect on compound");
            Err(ExecOutcome::Continue(1))
        }
    }
}

/// Runs a `while`/`until` loop. The body runs while the condition's
/// exit status satisfies the loop's polarity. `break` ends the loop;
/// `continue` jumps to the next condition test; `exit` propagates; a
/// pending SIGINT (Ctrl-C) ends the loop with status 130.
fn run_while(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_while_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_while_inner(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;
    let mut last = ExecOutcome::Continue(0);
    loop {
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }
        shell.err_suppressed_depth += 1;
        let cond = execute_sequence_body(&clause.condition, shell, sink);
        shell.err_suppressed_depth -= 1;
        let keep_going = match cond {
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_) => {
                return cond;
            }
            ExecOutcome::Continue(c) => {
                if clause.until { c != 0 } else { c == 0 }
            }
        };
        if !keep_going {
            break;
        }
        match execute_sequence_body(&clause.body, shell, sink) {
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
fn run_for(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_for_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_for_inner(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // Expand the word list once — the same path command arguments take.
    // The no-`in` form (`has_in == false`) iterates the positional
    // parameters ("$@"); an explicit empty `in` (`has_in == true`, empty
    // `words`) iterates nothing (M-24a, matching bash).
    let mut values: Vec<String> = Vec::new();
    if clause.has_in {
        for word in &clause.words {
            match glob_expand_word(word, shell) {
                Ok(v) => values.extend(v),
                Err(()) => return ExecOutcome::Continue(1),
            }
        }
    } else {
        values = shell.positional_args.clone();
    }

    let mut last = ExecOutcome::Continue(0);
    for value in values {
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }
        if shell.try_set(&clause.var, value).is_err() {
            eprintln!("huck: {}: readonly variable", clause.var);
            return ExecOutcome::Continue(1);
        }
        match execute_sequence_body(&clause.body, shell, sink) {
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
fn run_arith(body: &crate::lexer::Word, shell: &mut Shell) -> ExecOutcome {
    match crate::expand::eval_arith_word(body, shell) {
        Ok(0) => ExecOutcome::Continue(1),
        Ok(_) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("huck: ((: {e}");
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
) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_arith_for_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_arith_for_inner(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // 1. Eval init once (if present).
    if let Some(init) = &clause.init
        && let Err(e) = crate::expand::eval_arith_word(init, shell)
    {
        eprintln!("huck: ((: {e}");
        return ExecOutcome::Continue(1);
    }

    let mut last = ExecOutcome::Continue(0);
    loop {
        // SIGINT check (mirrors run_for).
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }

        // 2. Eval cond. Empty cond = always true (matches bash).
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::expand::eval_arith_word(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("huck: ((: {e}");
                    return ExecOutcome::Continue(1);
                }
            },
        };
        if cond_value == 0 {
            break;
        }

        // 3. Execute body.
        match execute_sequence_body(&clause.body, shell, sink) {
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
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }

        // 4. Eval step (if present).
        if let Some(step) = &clause.step
            && let Err(e) = crate::expand::eval_arith_word(step, shell)
        {
            eprintln!("huck: ((: {e}");
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
    crate::builtins::run_builtin("read", &[], &mut devnull, shell)
}

/// Runs a `select NAME [in WORDS]; do BODY; done` menu loop, mirroring
/// bash 5.2's `execute_select_command`/`select_query`. The numbered menu
/// (via `format_select_menu`) and the `PS3` prompt go to stderr; one line
/// is read into `REPLY` per prompt via the `read` builtin. An empty list
/// runs the body zero times; `break`/`continue N` bubble via the v79
/// loop infrastructure. Wrapped to keep a single `loop_depth` return path.
fn run_select(clause: &crate::command::SelectClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_select_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_select_inner(clause: &crate::command::SelectClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // 1. Build the item list: expand `in WORDS` (Some), or "$@" (None).
    let items: Vec<String> = match &clause.words {
        Some(words) => {
            let mut v = Vec::new();
            for w in words {
                match glob_expand_word(w, shell) {
                    Ok(g) => v.extend(g),
                    Err(()) => return ExecOutcome::Continue(1),
                }
            }
            v
        }
        None => shell.positional_args.clone(),
    };

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
            if show_menu {
                eprint!("{}", format_select_menu(&items, cols_width));
            }
            eprint!("{ps3}");
            let _ = std::io::stderr().flush();

            let r = read_line_into_reply(shell);
            if !matches!(r, ExecOutcome::Continue(0)) {
                // EOF / read failure → write a newline to stdout (bash) and
                // terminate the loop with the last status (read failure if no
                // body ran).
                match sink {
                    StdoutSink::Terminal => {
                        let _ = writeln!(io::stdout());
                    }
                    StdoutSink::Capture(buf) => buf.push(b'\n'),
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
            eprintln!("huck: {}: readonly variable", clause.var);
            return ExecOutcome::Continue(1);
        }

        // 3d. SIGINT check (mirror run_for).
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }

        // 3e. Run the body; bubble flow with the v79 decrement-and-bubble pattern.
        match execute_sequence_body(&clause.body, shell, sink) {
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
        let hit = if extglob && crate::glob_match::has_extglob(&pattern) {
            crate::glob_match::extglob_match(&pattern, subject, nocase)
        } else {
            glob::Pattern::new(&pattern)
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
fn run_case(clause: &CaseClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let subject = expand_assignment(&clause.subject, shell);
    let mut last = ExecOutcome::Continue(0);
    let mut i = 0;
    let mut fall_through = false;
    while i < clause.items.len() {
        let item = &clause.items[i];
        let run_this = fall_through || case_item_matches(item, &subject, shell);
        if let Some(status) = shell.pending_fatal_pe_error {
            return ExecOutcome::Continue(status);
        }
        if !run_this {
            i += 1;
            continue;
        }
        match &item.body {
            None => last = ExecOutcome::Continue(0),
            Some(body) => match execute_sequence_body(body, shell, sink) {
                ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
                ExecOutcome::LoopBreak(n, st) => return ExecOutcome::LoopBreak(n, st),
                ExecOutcome::LoopContinue(n) => return ExecOutcome::LoopContinue(n),
                ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
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
fn run_if(clause: &IfClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.err_suppressed_depth += 1;
    let cond = execute_sequence_body(&clause.condition, shell, sink);
    shell.err_suppressed_depth -= 1;
    if matches!(
        cond,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_)
    ) {
        return cond;
    }
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink);
    }
    for elif in &clause.elif_branches {
        shell.err_suppressed_depth += 1;
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink);
        shell.err_suppressed_depth -= 1;
        if matches!(
            elif_cond,
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_)
        ) {
            return elif_cond;
        }
        if matches!(elif_cond, ExecOutcome::Continue(0)) {
            return execute_sequence_body(&elif.body, shell, sink);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell, sink);
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
) -> ExecOutcome {
    let snap = match apply_inline_assignments(inline_assignments, shell) {
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
            eprintln!("huck: [[: {msg}");
            ExecOutcome::Continue(2)
        }
    };
    restore_inline_assignments(snap, shell);
    result
}

fn eval_test_expr(expr: &TestExpr, shell: &mut Shell) -> Result<bool, String> {
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            if matches!(op, TestUnaryOp::VarSet) {
                return Ok(shell.is_set(&s));
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
            let p = expand_assignment(pattern, shell);
            let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            Ok(re.is_match(&l))
        }
        TestExpr::Not(inner) => eval_test_expr(inner, shell).map(|b| !b),
        TestExpr::And(a, b) => {
            if eval_test_expr(a, shell)? {
                eval_test_expr(b, shell)
            } else {
                Ok(false)
            }
        }
        TestExpr::Or(a, b) => {
            if eval_test_expr(a, shell)? {
                Ok(true)
            } else {
                eval_test_expr(b, shell)
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
        TestUnaryOp::VarSet       => unreachable!("VarSet handled in eval_test_expr"),
        TestUnaryOp::OptEnabled   => unreachable!("OptEnabled handled in eval_test_expr"),
    }
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
            let matched = if extglob && crate::glob_match::has_extglob(&pattern_str) {
                crate::glob_match::extglob_match(&pattern_str, lhs, nocase)
            } else {
                let pat = glob::Pattern::new(&pattern_str)
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
            let parse_int = |s: &str| -> Result<i64, String> {
                let t = s.trim();
                if t.is_empty() {
                    return Ok(0);
                }
                t.parse().map_err(|_| format!("bad integer: {s}"))
            };
            let l: i64 = parse_int(lhs)?;
            let r: i64 = parse_int(&rhs)?;
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

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage pipeline: run directly in the parent shell (no fork needed).
        // This covers both Simple commands and compound commands as single stages.
        run_command(&pipeline.commands[0], shell, sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink)
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
    _sink: &mut StdoutSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);
    // Inherit stdin from the terminal (unlike pipeline backgrounds that use
    // /dev/null) — match bash/dash behaviour for `(cmd) &`.
    match fork_and_run_in_subshell(
        cmd,
        shell,
        libc::STDIN_FILENO,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        /*pgid_target=*/ 0,
        /*parent_fds_to_close=*/ &[],
        None, // no Dup redirect at this call site
        None,
    ) {
        Ok(pid) => {
            shell.last_bg_pid = Some(pid);
            let id = shell.jobs.add(pid, vec![pid], display);
            if shell.is_interactive {
                eprintln!("[{id}] {pid}");
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("huck: fork: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn run_background_sequence(
    pipeline: &Pipeline,
    shell: &mut Shell,
    _sink: &mut StdoutSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);

    // Spawn each stage using the same per-stage fork dispatch as run_multi_stage
    // (classify_stage → External via spawn_external_with_fds, or InProcess via
    // fork_and_run_in_subshell). This handles all Command variants including
    // compound commands (if/while/for/case/brace-group), so there are no
    // unreachable! arms. After all stages are spawned, register the job and
    // return immediately (no wait) — that's what makes this "background".
    //
    // Background stdin default: /dev/null for stage 0 (no explicit redirect,
    // no previous pipe) so the job doesn't compete for the terminal.
    use std::os::fd::FromRawFd;

    let n = pipeline.commands.len();
    let mut spawned_pids: Vec<i32> = Vec::with_capacity(n);
    let mut first_pid: Option<i32> = None;
    let mut prev_pipe_read: Option<RawFd> = None;
    let mut parent_held: Vec<RawFd> = Vec::new();
    let mut heredoc_pairs: Vec<(RawFd, Vec<u8>)> = Vec::new(); // (write_fd, bytes)

    // Open /dev/null once for the first stage's default stdin.
    let devnull_fd: RawFd = {
        use std::os::unix::io::IntoRawFd;
        match File::open("/dev/null") {
            Ok(f) => f.into_raw_fd(),
            Err(e) => {
                eprintln!("huck: /dev/null: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    };
    parent_held.push(devnull_fd);

    for (i, stage_cmd) in pipeline.commands.iter().enumerate() {
        let is_last = i == n - 1;

        // ---- Assign-only stages: no-op ----------------------------------------
        if let Command::Simple(SimpleCommand::Assign(items)) = stage_cmd {
            // Drop incoming pipe (no-op stage produces no output).
            if let Some(r) = prev_pipe_read.take() {
                parent_held.retain(|&fd| fd != r);
                unsafe { libc::close(r); }
            }
            // Run via fork so it's isolated (assignments don't affect parent).
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone()));
            let pgid_target = first_pid.unwrap_or(0);
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
                        eprintln!("huck: pipe: {e}");
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
                        unsafe {
                            if libc::setpgid(pid, pid) != 0 {
                                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                                debug_assert!(errno == libc::ESRCH || errno == libc::EACCES,
                                    "setpgid({pid},{pid}) failed errno {errno}");
                            }
                        }
                    }
                    spawned_pids.push(pid);
                }
                Err(e) => {
                    eprintln!("huck: fork: {e}");
                    if stdout_fd > 2 { unsafe { libc::close(stdout_fd); } }
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
        let snap = match apply_inline_assignments(inline_assignments, shell) {
            Ok(s) => s,
            Err(s) => {
                restore_inline_assignments(s, shell);
                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                return ExecOutcome::Continue(1);
            }
        };

        // ---- Stdin fd ---------------------------------------------------------
        let mut heredoc_write_fd: Option<RawFd> = None;
        let mut heredoc_body_bytes: Option<Vec<u8>> = None;

        let stdin_fd: RawFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.stdin {
                Some(Redirect::Read(word)) => {
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    let path = match expand_single(word, shell) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    };
                    use std::os::unix::io::IntoRawFd;
                    match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            eprintln!("huck: {path}: {e}");
                            restore_inline_assignments(snap, shell);
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
                    heredoc_body_bytes = Some(expand_assignment(body, shell).into_bytes());
                    match make_pipe() {
                        Ok((r, w)) => {
                            heredoc_write_fd = Some(w);
                            r
                        }
                        Err(e) => {
                            eprintln!("huck: pipe: {e}");
                            restore_inline_assignments(snap, shell);
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
                    // Here-string: expand with no split/glob + trailing newline.
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    heredoc_body_bytes = Some(bytes);
                    match make_pipe() {
                        Ok((r, w)) => {
                            heredoc_write_fd = Some(w);
                            r
                        }
                        Err(e) => {
                            eprintln!("huck: pipe: {e}");
                            restore_inline_assignments(snap, shell);
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
                match &exec.stdout {
                    Some(Redirect::Truncate(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
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
                match &exec.stderr {
                    Some(Redirect::Truncate(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
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
                    eprintln!("huck: pipe: {e}");
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
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
        let pgid_target = first_pid.unwrap_or(0);

        let mut fds_to_close_in_child: Vec<RawFd> = parent_held.iter().copied()
            .filter(|&fd| fd != stdout_fd && fd != stdin_fd && fd != stderr_fd)
            .collect();
        if let Some(w) = heredoc_write_fd {
            fds_to_close_in_child.push(w);
        }

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.stdout {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                eprintln!("huck: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.stderr {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                eprintln!("huck: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
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
                spawn_external_with_fds(simple, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &fds_to_close_in_child)
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
                eprintln!("huck: {e}");
                if !went_external {
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if stdout_fd > 2 { unsafe { libc::close(stdout_fd); } }
                    if stderr_fd > 2 { unsafe { libc::close(stderr_fd); } }
                }
                for fd in [stdout_fd, stdin_fd, stderr_fd] {
                    if fd > 2 { parent_held.retain(|&x| x != fd); }
                }
                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
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

        // Write heredoc body after child is spawned.
        if let (Some(w), Some(bytes)) = (heredoc_write_fd.take(), heredoc_body_bytes.take()) {
            heredoc_pairs.push((w, bytes));
        }

        // Track pgrp + pid.
        if first_pid.is_none() {
            first_pid = Some(pid);
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
        spawned_pids.push(pid);
    }

    // Close all remaining parent-held fds (inter-stage pipe read-ends that
    // weren't consumed, and the /dev/null fd).
    for fd in parent_held.drain(..) {
        unsafe { libc::close(fd); }
    }

    // Write heredoc bodies and close write-ends so children see EOF.
    for (w, bytes) in heredoc_pairs {
        let mut write_file = unsafe { File::from_raw_fd(w) };
        let _ = write_file.write_all(&bytes);
        // write_file drops here, closing w.
    }

    let Some(pgid) = first_pid else {
        // No actual children spawned (all-Assign pipeline). Treat as
        // synthetic Done. This shouldn't happen in practice — the parser
        // doesn't produce all-Assign backgrounded pipelines as a typical
        // user input shape, but we handle it defensively.
        shell.jobs.add_synthetic_done(display, 0);
        crate::jobs::reap_and_notify(shell);
        return ExecOutcome::Continue(0);
    };

    let last_pid = *spawned_pids.last().unwrap();
    shell.last_bg_pid = Some(last_pid);
    let id = shell.jobs.add(pgid, spawned_pids, display);
    if shell.is_interactive {
        eprintln!("[{id}] {last_pid}");
    }
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

/// Resolved stdin source for a command — either a file path or a heredoc body
/// Word that will be expanded just before the child is spawned (so that inline
/// assignments applied between resolve-time and spawn-time are visible).
enum ResolvedStdin {
    /// `< file` — path to open for reading.
    File(String),
    /// `<< EOF` — body Word to be expanded after inline assignments are applied.
    /// Storing the Word (rather than pre-expanded bytes) ensures that
    /// `FOO=hi cat <<EOF\n$FOO\nEOF` sees FOO=hi in the body expansion.
    Heredoc(crate::lexer::Word),
    /// `<<< word` — here-string body Word to be expanded just before spawning.
    /// Expansion uses expand_assignment (no split/glob); a trailing `\n` is
    /// appended per bash semantics.
    HereString(crate::lexer::Word),
}

struct ResolvedCommand {
    program: String,
    args: Vec<String>,
    /// Populated only when `program` is a declaration command
    /// (`declare`, `typeset`, `local`, `readonly`, `export`). Carries
    /// the per-arg `DeclArg` shape so the builtin can route compound-RHS
    /// assignments through `apply_one_assignment` while still handling
    /// flags and scalar names. `args` is left empty in that case.
    decl_args: Option<Vec<crate::command::DeclArg>>,
    stdin: Option<ResolvedStdin>,
    stdout: Option<ResolvedRedirect>,
    stderr: Option<ResolvedRedirect>,
}

enum ResolvedRedirect {
    Truncate(String),
    Append(String),
}

fn expand_single(word: &crate::lexer::Word, shell: &mut Shell) -> Result<String, ()> {
    // Redirect targets do NOT undergo pathname expansion in v10 (per spec).
    // We call `expand` directly and require exactly one field, preserving the
    // ambiguous-redirect contract for word-splitting that produces 0 or >1.
    let fields = expand(word, shell);
    if fields.len() == 1 {
        Ok(fields.into_iter().next().unwrap().chars)
    } else {
        eprintln!("huck: ambiguous redirect");
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
fn glob_expand_word(word: &crate::lexer::Word, shell: &mut Shell) -> Result<Vec<String>, ()> {
    // Borrow note: take the owned `Copy` opts before the mutable `expand`
    // borrow, so the immutable borrow ends first.
    let opts = shell.glob_opts();
    let fields = expand(word, shell);
    let exp = glob_expand_fields_opts(fields, opts);
    if !exp.failglob_unmatched.is_empty() {
        eprintln!("huck: no match: {}", exp.failglob_unmatched.join(" "));
        return Err(());
    }
    Ok(exp.words)
}

fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = match glob_expand_word(&cmd.program, shell) {
        Ok(v) => v,
        Err(()) => return Err(1),
    };
    if let Some(status) = shell.pending_fatal_pe_error {
        return Err(status);
    }
    if prog_fields.is_empty() {
        eprintln!("huck: command not found:");
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
        let fields = match glob_expand_word(word, shell) {
            Ok(v) => v,
            Err(()) => return Err(1),
        };
        if let Some(status) = shell.pending_fatal_pe_error {
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
    let stdin = match &cmd.stdin {
        Some(Redirect::Read(word)) => {
            let path = expand_single(word, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error {
                return Err(status);
            }
            Some(ResolvedStdin::File(path))
        }
        Some(Redirect::Heredoc { body, .. }) => {
            // Store the body Word to be expanded later (after inline
            // assignments have been applied). This ensures `FOO=hi cat <<EOF`
            // body expansion sees FOO=hi.
            Some(ResolvedStdin::Heredoc(body.clone()))
        }
        Some(Redirect::HereString(w)) => Some(ResolvedStdin::HereString(w.clone())),
        Some(Redirect::Truncate(_) | Redirect::Append(_)) => {
            unreachable!("parser never produces Truncate/Append for stdin")
        }
        Some(Redirect::Dup { .. }) => unreachable!(
            "Redirect::Dup on stdin (<&n) is out of scope for v29"
        ),
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(Redirect::Truncate(w)) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error {
                return Err(status);
            }
            Some(ResolvedRedirect::Truncate(path))
        }
        Some(Redirect::Append(w)) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error {
                return Err(status);
            }
            Some(ResolvedRedirect::Append(path))
        }
        Some(Redirect::Read(_) | Redirect::Heredoc { .. } | Redirect::HereString(_)) => {
            unreachable!("parser never produces Read/Heredoc/HereString for stdout")
        }
        Some(Redirect::Dup { .. }) => {
            // Dup is handled via dup2 (pre_exec or direct), not by opening a file.
            // Return None here; callers check exec.stdout for Dup and apply dup2.
            None
        }
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(Redirect::Truncate(w)) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error {
                return Err(status);
            }
            Some(ResolvedRedirect::Truncate(path))
        }
        Some(Redirect::Append(w)) => {
            let path = expand_single(w, shell).map_err(|()| 1)?;
            if let Some(status) = shell.pending_fatal_pe_error {
                return Err(status);
            }
            Some(ResolvedRedirect::Append(path))
        }
        Some(Redirect::Read(_) | Redirect::Heredoc { .. } | Redirect::HereString(_)) => {
            unreachable!("parser never produces Read/Heredoc/HereString for stderr")
        }
        Some(Redirect::Dup { .. }) => {
            // Dup is handled via dup2 (pre_exec or direct), not by opening a file.
            // Return None here; callers check exec.stderr for Dup and apply dup2.
            None
        }
        None => None,
    };
    Ok(ResolvedCommand { program, args, decl_args, stdin, stdout, stderr })
}

// ----- redirect file handling -----------------------------------------------

/// Resolved stdin for a spawned subprocess — either an open file or a
/// heredoc body Word whose expansion is deferred until after per-stage inline
/// assignments are applied, so that `$var` references in the body see the
/// stage's own inline assignments.
enum StdinInput {
    File(File),
    /// Heredoc body to be expanded just before spawning the child, after
    /// `apply_inline_assignments` has been called for this stage.
    DeferredHeredoc(crate::lexer::Word),
    /// Here-string body to be expanded just before spawning the child.
    /// Expansion via expand_assignment (no split/glob) + trailing `\n`.
    DeferredHereString(crate::lexer::Word),
}

struct StageFiles {
    stdin: Option<StdinInput>,
    stdout: Option<File>,
    stderr: Option<File>,
}

/// RAII guard that restores STDIN_FILENO from a saved dup'd fd on drop.
/// Used to apply stdin redirection around an in-process builtin call.
struct BuiltinStdinGuard {
    saved_fd: RawFd,
}

impl Drop for BuiltinStdinGuard {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_fd, libc::STDIN_FILENO);
            libc::close(self.saved_fd);
        }
    }
}

/// Apply `stdin` to STDIN_FILENO for the duration of an in-process builtin
/// call. Returns an RAII guard whose Drop restores the original stdin.
/// Returns Ok(None) when there is no stdin redirection (no save needed).
///
/// For `File`: dup2 the file's fd to 0.
/// For `DeferredHeredoc` / `DeferredHereString`: build a pipe, write the
/// expanded body to the write end (close it), dup2 the read end to 0. Bodies
/// are bounded by the pipe buffer (~64K on Linux); larger bodies would block.
fn prepare_builtin_stdin(
    stdin: Option<StdinInput>,
    shell: &mut Shell,
) -> Result<Option<BuiltinStdinGuard>, ()> {
    use std::os::unix::io::IntoRawFd;
    let new_fd: RawFd = match stdin {
        None => return Ok(None),
        Some(StdinInput::File(file)) => file.into_raw_fd(),
        Some(StdinInput::DeferredHeredoc(body)) => {
            let bytes = expand_assignment(&body, shell).into_bytes();
            write_pipe_for_stdin(&bytes)?
        }
        Some(StdinInput::DeferredHereString(body)) => {
            let mut bytes = expand_assignment(&body, shell).into_bytes();
            bytes.push(b'\n');
            write_pipe_for_stdin(&bytes)?
        }
    };
    unsafe {
        let saved = libc::dup(libc::STDIN_FILENO);
        if saved < 0 {
            let e = io::Error::last_os_error();
            eprintln!("huck: dup: {e}");
            libc::close(new_fd);
            return Err(());
        }
        if libc::dup2(new_fd, libc::STDIN_FILENO) < 0 {
            let e = io::Error::last_os_error();
            eprintln!("huck: dup2: {e}");
            libc::close(saved);
            libc::close(new_fd);
            return Err(());
        }
        libc::close(new_fd);
        Ok(Some(BuiltinStdinGuard { saved_fd: saved }))
    }
}

/// RAII guard that restores STDERR_FILENO from a saved dup'd fd on drop.
/// Used to apply a `2>`/`2>>`/`2>&1` redirection around an in-process builtin
/// call so the builtin's `eprintln!` diagnostics honor the redirect. Drop
/// flushes Rust's stderr buffer first so any buffered text lands on the
/// redirect target before fd 2 is restored.
struct BuiltinStderrGuard {
    saved_fd: RawFd,
}

impl Drop for BuiltinStderrGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = std::io::Write::flush(&mut std::io::stderr());
            libc::dup2(self.saved_fd, libc::STDERR_FILENO);
            libc::close(self.saved_fd);
        }
    }
}

/// Apply a stderr redirection to STDERR_FILENO for the duration of an
/// in-process builtin call. Returns an RAII guard whose Drop restores the
/// original stderr. Returns None when there is no stderr redirection.
///
/// `stderr` is the opened `2>file`/`2>>file` target (mirrors
/// `prepare_builtin_stdin`'s `File` handling via `into_raw_fd` + close-after).
/// `dup_target` is `Some(fd)` for `2>&1` (a `Redirect::Dup{fd:2,source:1}`
/// resolves to no File): the fd that `2>&1` should follow — the real fd 1 for a
/// Terminal sink, or a redirected stdout file's fd for `>file 2>&1`. fd 2 is
/// dup2'd onto a copy of that target, applied AFTER any stdout setup so
/// `>out 2>&1` / pipeline ordering holds. `None` means no in-process dup.
fn prepare_builtin_stderr(stderr: Option<File>, dup_target: Option<RawFd>) -> Option<BuiltinStderrGuard> {
    use std::os::unix::io::IntoRawFd;
    let new_fd: RawFd = match stderr {
        Some(file) => file.into_raw_fd(),
        None => match dup_target {
            // `2>&1`: duplicate the target fd (the real fd 1, or a redirected
            // stdout file's fd) so we dup2 a copy onto fd 2 and can close it.
            Some(target) => {
                let d = unsafe { libc::dup(target) };
                if d < 0 {
                    return None;
                }
                d
            }
            None => return None,
        },
    };
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 {
            let e = io::Error::last_os_error();
            eprintln!("huck: dup: {e}");
            libc::close(new_fd);
            return None;
        }
        let _ = std::io::Write::flush(&mut std::io::stderr());
        if libc::dup2(new_fd, libc::STDERR_FILENO) < 0 {
            let e = io::Error::last_os_error();
            eprintln!("huck: dup2: {e}");
            libc::close(saved);
            libc::close(new_fd);
            return None;
        }
        libc::close(new_fd);
        Some(BuiltinStderrGuard { saved_fd: saved })
    }
}

/// True when `cmd.stderr` is a `2>&1`-style dup whose source resolves to fd 1,
/// so an in-process builtin's stderr should follow fd 1 for the call's
/// duration. `2>&N` for other N is left to the subprocess path (rare on
/// builtins) and treated as no in-process dup.
fn stderr_dups_to_stdout(cmd: &ExecCommand, shell: &mut Shell) -> bool {
    match &cmd.stderr {
        Some(Redirect::Dup { source, .. }) => matches!(resolve_fd_target(source, shell), Ok(1)),
        _ => false,
    }
}

/// Create a pipe, write `bytes` to the write end, close it, return the
/// read end's raw fd. Used to feed a heredoc/here-string body to an
/// in-process builtin's stdin.
fn write_pipe_for_stdin(bytes: &[u8]) -> Result<RawFd, ()> {
    let mut fds: [libc::c_int; 2] = [-1, -1];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        let e = io::Error::last_os_error();
        eprintln!("huck: pipe: {e}");
        return Err(());
    }
    let r = fds[0];
    let w = fds[1];
    // Write may not complete if bytes exceed pipe buffer; for heredoc/
    // here-string bodies in v55 read tests this is well under 64K. A
    // future enhancement could fork a writer if needed.
    let mut written = 0usize;
    while written < bytes.len() {
        let n = unsafe {
            libc::write(
                w,
                bytes[written..].as_ptr() as *const libc::c_void,
                bytes.len() - written,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            // Retry on EINTR — a signal (e.g., SIGCHLD from a
            // background job completing) during the write must not
            // surface as "Interrupted system call".
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("huck: write: {e}");
            unsafe {
                libc::close(r);
                libc::close(w);
            }
            return Err(());
        }
        written += n as usize;
    }
    unsafe { libc::close(w) };
    Ok(r)
}

fn open_stage_files(cmd: &ResolvedCommand, _shell: &mut Shell) -> Result<StageFiles, ()> {
    let stdin = match &cmd.stdin {
        Some(ResolvedStdin::File(path)) => match File::open(path) {
            Ok(file) => Some(StdinInput::File(file)),
            Err(e) => {
                eprintln!("huck: {path}: {e}");
                return Err(());
            }
        },
        Some(ResolvedStdin::Heredoc(body)) => {
            // Defer body expansion: store the Word so the caller can expand it
            // after applying per-stage inline assignments. Callers that have
            // already applied inline assignments before calling open_stage_files
            // (run_exec_single, run_background_sequence) will see a
            // DeferredHeredoc and must expand it before spawning the child.
            Some(StdinInput::DeferredHeredoc(body.clone()))
        }
        Some(ResolvedStdin::HereString(body)) => {
            Some(StdinInput::DeferredHereString(body.clone()))
        }
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(redirect) => match open_resolved(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("huck: {}: {e}", resolved_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(redirect) => match open_resolved(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("huck: {}: {e}", resolved_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    Ok(StageFiles { stdin, stdout, stderr })
}

fn open_resolved(redirect: &ResolvedRedirect) -> io::Result<File> {
    match redirect {
        ResolvedRedirect::Truncate(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path),
        ResolvedRedirect::Append(path) => OpenOptions::new()
            .create(true)
            .append(true)
            .open(path),
    }
}

fn resolved_path(redirect: &ResolvedRedirect) -> &str {
    match redirect {
        ResolvedRedirect::Truncate(p) | ResolvedRedirect::Append(p) => p,
    }
}

fn status_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1))
}

// ----- single command -------------------------------------------------------

// $PIPESTATUS leaf-site rule (M-50, v83): ONLY `run_single`, `run_multi_stage`,
// and the foreground subshell arm write `$PIPESTATUS`. Compound runners
// (`run_if`/`run_while`/`run_for`/`run_case`/`run_select`/brace group) are
// deliberately PIPESTATUS-transparent — they never write it; their inner leaf
// commands do. This matches bash: after `if cond; then ...; fi`, `$PIPESTATUS`
// reflects the last inner pipeline (e.g. `cond`), not the `if` itself. Do NOT
// add a set_pipestatus call to a compound runner.
fn run_single(cmd: &SimpleCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let outcome = match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell, sink),
        SimpleCommand::Assign(items) => {
            let mut st = 0;
            for a in items {
                let name = a.target.name();
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    st = 1;
                    break;
                }
                if apply_one_assignment(a, shell).is_err() {
                    st = 1;
                    break;
                }
            }
            ExecOutcome::Continue(st)
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
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    let saved_loop_depth = std::mem::replace(&mut shell.loop_depth, 0);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    shell.local_scopes.push(std::collections::HashMap::new());

    let result = run_command(&body, shell, sink);

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

    shell.function_arg0.pop();
    shell.positional_args = saved;
    shell.loop_depth = saved_loop_depth;
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
    call_function(name, body, args, shell, &mut sink)
}

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
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
            stdin: cmd.stdin.clone(),
            stdout: cmd.stdout.clone(),
            stderr: cmd.stderr.clone(),
        };
        return run_exec_single(&inner, shell, sink);
    }

    let mut resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };

    // `command CMD args` (bare form): run CMD suppressing shell-FUNCTION lookup
    // (builtins + $PATH still resolve). `-v`/`-V` introspection is left to the
    // `command` builtin (not intercepted here).
    let mut bypass_functions = false;
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
                    eprintln!("huck: command: {s}: invalid option");
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
            None => return ExecOutcome::Continue(0), // `command` / `command -p` alone
            Some(_) => {
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

    // Apply inline assignments (e.g. FOO=bar in `FOO=bar cmd args`) before
    // dispatch. The snapshot is used to restore state for temporary-scope
    // targets (regular builtins and externals). Persistent-scope targets
    // (control builtins, special builtins per POSIX 2.14, and functions per
    // POSIX 2.9.1) skip the restore step.
    let snap = match apply_inline_assignments(&cmd.inline_assignments, shell) {
        Ok(s) => s,
        Err(s) => {
            restore_inline_assignments(s, shell);
            return ExecOutcome::Continue(1);
        }
    };

    // xtrace (`set -x`): print the expanded command to stderr, prefixed by
    // `$PS4` (default `+ `), BEFORE dispatch so a hanging command is traced
    // first. Use the already-expanded `resolved.program`/`resolved.args` (do
    // NOT re-expand). For a pure-assignment command (empty program) render
    // `name=value` from the just-applied values (read back via lookup_var). The
    // inline-assignment PREFIX on `VAR=v cmd` is omitted (minor divergence).
    if shell.shell_options.xtrace {
        let ps4 = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
        let mut line = String::new();
        if resolved.program.is_empty() {
            let mut first = true;
            for a in &cmd.inline_assignments {
                if !first {
                    line.push(' ');
                }
                first = false;
                let n = a.target.name();
                let v = shell.lookup_var(n).unwrap_or_default();
                line.push_str(&format!("{n}={v}"));
            }
        } else {
            line.push_str(&resolved.program);
            for a in &resolved.args {
                line.push(' ');
                line.push_str(a);
            }
        }
        eprintln!("{ps4}{line}");
    }

    // Determine whether the assignments should persist after the command.
    // Control builtins and special builtins: persistent.
    // User functions: persistent.
    // Regular builtins and external commands: temporary (restore after).
    // Note: is_control_builtin's set {break,continue,exit,return} is a strict
    // subset of is_special_builtin's set, so only the latter term is needed.
    let persistent = builtins::is_special_builtin(&resolved.program)
        || (!bypass_functions && shell.functions.contains_key(&resolved.program));

    // 1. Control builtins always win — they cannot be shadowed by functions.
    // 2. User-defined function lookup.
    // 3. Regular builtin.
    // 4. PATH-exec.
    let outcome = if is_control_builtin(&resolved.program) {
        let mut files = match open_stage_files(&resolved, shell) {
            Ok(f) => f,
            Err(()) => {
                // Control builtins always persist their inline assignments (POSIX
                // special-builtin semantics); no restore needed on the redirect-open
                // failure path.
                return ExecOutcome::Continue(1);
            }
        };
        // `2>&1` on a builtin: follow wherever the builtin's stdout actually
        // goes. With `>file` stdout the builtin writes to a Rust File (not fd 1),
        // so dup the FILE's fd onto fd 2; with a bare `2>&1` under a Terminal
        // sink, dup the real fd 1; a Capture sink has no fd (L-25 residual).
        let dup_target: Option<RawFd> = if files.stderr.is_none() && stderr_dups_to_stdout(cmd, shell) {
            if let Some(file) = files.stdout.as_ref() {
                use std::os::unix::io::AsRawFd;
                Some(file.as_raw_fd())
            } else if matches!(sink, StdoutSink::Terminal) {
                Some(libc::STDOUT_FILENO)
            } else {
                None
            }
        } else {
            None
        };
        let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_target);
        let outcome = match files.stdout {
            Some(mut file) => {
                builtins::run_builtin(&resolved.program, &resolved.args, &mut file, shell)
            }
            None => match sink {
                StdoutSink::Terminal => {
                    let mut out = io::stdout();
                    builtins::run_builtin(&resolved.program, &resolved.args, &mut out, shell)
                }
                StdoutSink::Capture(buf) => {
                    builtins::run_builtin(&resolved.program, &resolved.args, *buf, shell)
                }
            },
        };
        drop(stderr_guard);
        outcome
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
        call_function(&resolved.program.clone(), body, resolved.args, shell, sink)
    } else if builtins::is_builtin(&resolved.program) {
        let mut files = match open_stage_files(&resolved, shell) {
            Ok(f) => f,
            Err(()) => {
                if !persistent {
                    restore_inline_assignments(snap, shell);
                }
                return ExecOutcome::Continue(1);
            }
        };
        // Apply stdin redirection in-process for builtins: builtins that read
        // from stdin (e.g. `read`) need `<<<`, `<<`, and `<file` to actually
        // affect the fd they read from. Save+dup2 around the call.
        let stdin_guard = match prepare_builtin_stdin(files.stdin, shell) {
            Ok(g) => g,
            Err(()) => {
                if !persistent {
                    restore_inline_assignments(snap, shell);
                }
                return ExecOutcome::Continue(1);
            }
        };
        // `2>&1` on a builtin: follow wherever the builtin's stdout actually
        // goes. With `>file` stdout the builtin writes to a Rust File (not fd 1),
        // so dup the FILE's fd onto fd 2; with a bare `2>&1` under a Terminal
        // sink, dup the real fd 1; a Capture sink has no fd (L-25 residual).
        let dup_target: Option<RawFd> = if files.stderr.is_none() && stderr_dups_to_stdout(cmd, shell) {
            if let Some(file) = files.stdout.as_ref() {
                use std::os::unix::io::AsRawFd;
                Some(file.as_raw_fd())
            } else if matches!(sink, StdoutSink::Terminal) {
                Some(libc::STDOUT_FILENO)
            } else {
                None
            }
        } else {
            None
        };
        let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_target);
        let outcome = match files.stdout {
            Some(mut file) => {
                if let Some(da) = resolved.decl_args.as_deref() {
                    builtins::run_declaration_builtin(&resolved.program, da, &mut file, shell)
                } else {
                    builtins::run_builtin(&resolved.program, &resolved.args, &mut file, shell)
                }
            }
            None => match sink {
                StdoutSink::Terminal => {
                    let mut out = io::stdout();
                    if let Some(da) = resolved.decl_args.as_deref() {
                        builtins::run_declaration_builtin(&resolved.program, da, &mut out, shell)
                    } else {
                        builtins::run_builtin(&resolved.program, &resolved.args, &mut out, shell)
                    }
                }
                StdoutSink::Capture(buf) => {
                    if let Some(da) = resolved.decl_args.as_deref() {
                        builtins::run_declaration_builtin(&resolved.program, da, *buf, shell)
                    } else {
                        builtins::run_builtin(&resolved.program, &resolved.args, *buf, shell)
                    }
                }
            },
        };
        drop(stderr_guard);
        drop(stdin_guard);
        outcome
    } else {
        let files = match open_stage_files(&resolved, shell) {
            Ok(f) => f,
            Err(()) => {
                if !persistent {
                    restore_inline_assignments(snap, shell);
                }
                return ExecOutcome::Continue(1);
            }
        };
        // Resolve Dup targets pre-fork (expansion may allocate; not async-signal-safe).
        let stdout_dup_target = match &cmd.stdout {
            Some(Redirect::Dup { source, .. }) => {
                match resolve_fd_target(source, shell) {
                    Ok(fd) => Some(fd),
                    Err(e) => {
                        eprintln!("huck: {e}");
                        if !persistent { restore_inline_assignments(snap, shell); }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            _ => None,
        };
        let stderr_dup_target = match &cmd.stderr {
            Some(Redirect::Dup { source, .. }) => {
                match resolve_fd_target(source, shell) {
                    Ok(fd) => Some(fd),
                    Err(e) => {
                        eprintln!("huck: {e}");
                        if !persistent { restore_inline_assignments(snap, shell); }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            _ => None,
        };
        run_subprocess(&resolved, files, shell, sink, stdout_dup_target, stderr_dup_target)
    };

    if !persistent {
        restore_inline_assignments(snap, shell);
    }
    outcome
}

/// `stdout_dup_target` / `stderr_dup_target`: if `Some(fd)`, a pre_exec closure
/// applies `dup2(fd, 1)` and/or `dup2(fd, 2)` in the child after stdio setup.
/// Used for `Redirect::Dup` (e.g. `2>&1`). Resolution happens in the parent
/// (pre-fork) so these are always resolved i32, never a Word.
fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    stdout_dup_target: Option<i32>,
    stderr_dup_target: Option<i32>,
) -> ExecOutcome {
    let interactive = matches!(sink, StdoutSink::Terminal);

    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());

    // Reset job-control signals to SIG_DFL in every child (foreground and
    // background). The shell SIG_IGNs these, and SIG_IGN is inherited across
    // exec — without this, Ctrl-Z would never stop foreground children like
    // vim/less, and background readers could never receive SIGTTIN.
    use std::os::unix::process::CommandExt;
    unsafe { process.pre_exec(reset_job_control_signals_in_child); }

    // If there are Dup redirects, add a second pre_exec to apply dup2 in the
    // child after stdio is configured but before exec. stdout-dup BEFORE
    // stderr-dup matches canonical `>file 2>&1` semantics.
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

    if interactive {
        process.process_group(0);
    }

    let mut pending_stdin_bytes: Option<Vec<u8>> = None;
    match files.stdin {
        Some(StdinInput::File(file)) => {
            process.stdin(Stdio::from(file));
        }
        Some(StdinInput::DeferredHeredoc(body)) => {
            // Inline assignments were applied before open_stage_files in this
            // path (run_exec_single / run_subprocess), so expanding here is
            // correct: $var references see the stage's inline assignments.
            let bytes = expand_assignment(&body, shell).into_bytes();
            process.stdin(Stdio::piped());
            pending_stdin_bytes = Some(bytes);
        }
        Some(StdinInput::DeferredHereString(body)) => {
            // Here-string: expand with no split/glob, append trailing newline.
            let mut bytes = expand_assignment(&body, shell).into_bytes();
            bytes.push(b'\n');
            process.stdin(Stdio::piped());
            pending_stdin_bytes = Some(bytes);
        }
        None => {}
    }
    let want_capture = matches!(sink, StdoutSink::Capture(_));
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    } else if stdout_dup_target.is_some() {
        // Dup redirect on stdout: inherit the parent's stdout (the dup2 pre_exec
        // will redirect to the target fd in the child). Stdio::inherit() avoids
        // the close-on-drop trap of OwnedFd::from_raw_fd for the parent's fd 1.
        process.stdout(Stdio::inherit());
    } else if want_capture {
        process.stdout(Stdio::piped());
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    } else if stderr_dup_target.is_some() {
        // Dup redirect on stderr: inherit parent's stderr; dup2 applied in child.
        process.stderr(Stdio::inherit());
    }

    match process.spawn() {
        Ok(mut child) => {
            // Write heredoc bytes into the child's piped stdin, then drop
            // the handle so the child sees EOF and can proceed.
            if let Some(bytes) = pending_stdin_bytes
                && let Some(mut child_stdin) = child.stdin.take()
            {
                let _ = child_stdin.write_all(&bytes);
                // child_stdin drops here, closing the pipe.
            }

            let pid = child.id() as i32;

            if interactive {
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
                        eprintln!("\n{line}");
                        std::mem::forget(child);
                        give_terminal_to(shell.shell_pgid);
                        ExecOutcome::Continue(128 + sig)
                    }
                    Ok((raw_status, false)) => {
                        // Child exited or was killed by a signal.
                        let code = if libc::WIFEXITED(raw_status) {
                            libc::WEXITSTATUS(raw_status)
                        } else if libc::WIFSIGNALED(raw_status) {
                            128 + libc::WTERMSIG(raw_status)
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
                // Capture path: use existing child.wait() semantics.
                let mut copy_err: Option<io::Error> = None;
                if let StdoutSink::Capture(buf) = sink
                    && let Some(mut child_stdout) = child.stdout.take()
                    && let Err(e) = io::copy(&mut child_stdout, *buf)
                {
                    copy_err = Some(e);
                }
                match child.wait() {
                    Ok(status) => {
                        if let Some(e) = copy_err {
                            eprintln!("huck: {}: {e}", cmd.program);
                            ExecOutcome::Continue(1)
                        } else {
                            ExecOutcome::Continue(status_code(&status))
                        }
                    }
                    Err(e) => {
                        eprintln!("huck: {}: {e}", cmd.program);
                        ExecOutcome::Continue(1)
                    }
                }
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            eprintln!("huck: command not found: {}", cmd.program);
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            eprintln!("huck: {}: {e}", cmd.program);
            ExecOutcome::Continue(1)
        }
    }
}

// ----- multi-stage pipeline -------------------------------------------------

/// Per-stage outcome after spawning: a forked child pid to be waited for.
enum PipelineStage {
    Forked(i32),
}

/// Opens a `libc::pipe()` and returns `(read_end, write_end)` as raw fds.
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
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
) -> ExecOutcome {
    use std::os::fd::FromRawFd;

    // Job-control process-grouping is only correct in the top-level shell. Inside
    // a forked subshell it places the inner pipeline in a background process group
    // with default SIGTTOU/SIGTTIN handling, deadlocking the subshell's wait on a
    // controlling terminal (M-104). A subshell's inner pipeline uses the
    // non-job-control path (stages stay in the subshell's pgrp), matching bash.
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell;
    let n = commands.len();

    // Fd for the capture-sink case: last stage's stdout is piped back to parent.
    let mut capture_read_fd: Option<RawFd> = None;

    // Pid tracking.
    let mut first_pid: Option<i32> = None;
    let mut stage_pids: Vec<i32> = Vec::with_capacity(n);

    // All forked stages (pid + optional inline exit status for Done stages).
    let mut pipeline_stages: Vec<PipelineStage> = Vec::with_capacity(n);

    // Read-end of the pipe from the previous stage (None for stage 0).
    let mut prev_pipe_read: Option<RawFd> = None;

    // All raw fds the parent currently holds (for the child's
    // parent_fds_to_close list so it doesn't inherit stale pipe ends).
    let mut parent_held: Vec<RawFd> = Vec::new();

    for (i, stage_cmd) in commands.iter().enumerate() {
        let is_last = i == n - 1;

        // ---- Assign-only stages: no-op, just pass stdin through as empty ----
        if let Command::Simple(SimpleCommand::Assign(items)) = stage_cmd {
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
            let assign_cmd = Command::Simple(SimpleCommand::Assign(items.clone()));
            let pgid_target = if interactive { first_pid.unwrap_or(0) } else { 0 };
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
                        eprintln!("huck: pipe: {e}");
                        // Clean up all held fds.
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
                                eprintln!("huck: pipe: {e}");
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
                    pipeline_stages.push(PipelineStage::Forked(pid));
                }
                Err(e) => {
                    eprintln!("huck: fork: {e}");
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
        let snap = match apply_inline_assignments(inline_assignments, shell) {
            Ok(s) => s,
            Err(s) => {
                restore_inline_assignments(s, shell);
                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                return ExecOutcome::Continue(1);
            }
        };

        // ---- Build stdin fd --------------------------------------------------
        // Priority: explicit redirect on ExecCommand > prev_pipe_read > STDIN_FILENO.
        // For InProcess compound stages, there are no explicit redirects at the
        // stage level; the child handles them internally via run_command.

        // We may need to create a heredoc pipe and write its body after spawning.
        // The body is expanded NOW (while inline assignments are applied) so that
        // $var references in the body see the stage's own inline assignments (v24
        // deferred-heredoc contract). The bytes are stored here and written to the
        // pipe write-end after the child is spawned.
        let mut heredoc_write_fd: Option<RawFd> = None;
        let mut heredoc_body_bytes: Option<Vec<u8>> = None;

        let stdin_fd: RawFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
            match &exec.stdin {
                Some(Redirect::Read(word)) => {
                    // Discard the previous stage's pipe read-end: this stage
                    // overrides stdin, so that pipe would otherwise be leaked
                    // into parent_held, keeping the write-end alive and
                    // deadlocking the previous stage's writer.
                    if let Some(r) = prev_pipe_read.take() {
                        parent_held.retain(|&fd| fd != r);
                        unsafe { libc::close(r); }
                    }
                    let path = match expand_single(word, shell) {
                        Ok(p) => p,
                        Err(()) => {
                            restore_inline_assignments(snap, shell);
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    };
                    use std::os::unix::io::IntoRawFd;
                    match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            eprintln!("huck: {path}: {e}");
                            restore_inline_assignments(snap, shell);
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
                    // Expand the body NOW while inline assignments are still applied.
                    // Store the bytes; write them to the pipe after the child is spawned.
                    heredoc_body_bytes = Some(expand_assignment(body, shell).into_bytes());
                    // Create a pipe: child reads from r; parent writes body to w.
                    match make_pipe() {
                        Ok((r, w)) => {
                            heredoc_write_fd = Some(w);
                            r
                        }
                        Err(e) => {
                            eprintln!("huck: pipe: {e}");
                            restore_inline_assignments(snap, shell);
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
                    // Expand NOW (inline assignments still applied) + trailing newline.
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    heredoc_body_bytes = Some(bytes);
                    // Create a pipe: child reads from r; parent writes body to w.
                    match make_pipe() {
                        Ok((r, w)) => {
                            heredoc_write_fd = Some(w);
                            r
                        }
                        Err(e) => {
                            eprintln!("huck: pipe: {e}");
                            restore_inline_assignments(snap, shell);
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
                match &exec.stdout {
                    Some(Redirect::Truncate(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
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
                match &exec.stderr {
                    Some(Redirect::Truncate(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    Some(Redirect::Append(w)) => {
                        let path = match expand_single(w, shell) {
                            Ok(p) => p,
                            Err(()) => {
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        };
                        use std::os::unix::io::IntoRawFd;
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(f.into_raw_fd()),
                            Err(e) => {
                                eprintln!("huck: {path}: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(fd) = explicit_stdout_fd { unsafe { libc::close(fd); } }
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
                    eprintln!("huck: pipe: {e}");
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
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
                            eprintln!("huck: pipe: {e}");
                            restore_inline_assignments(snap, shell);
                            if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                            if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                            if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                            for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                StdoutSink::Terminal => libc::STDOUT_FILENO,
            }
        };

        let stderr_fd = explicit_stderr_fd.unwrap_or(libc::STDERR_FILENO);

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if interactive { first_pid.unwrap_or(0) } else { 0 };

        // parent_fds_to_close: all fds the parent currently holds that the
        // child must close (so it doesn't hold downstream pipe write-ends open,
        // which would prevent EOF propagation). We exclude the fds being passed
        // to this stage as stdio (those are the child's to keep). We also
        // include the heredoc_write_fd so the child doesn't hold its own
        // stdin-pipe write-end open.
        let mut fds_to_close_in_child: Vec<RawFd> = parent_held.iter().copied()
            .filter(|&fd| fd != stdout_fd && fd != stdin_fd && fd != stderr_fd)
            .collect();
        if let Some(w) = heredoc_write_fd {
            // The child must close the write-end of its own heredoc pipe.
            fds_to_close_in_child.push(w);
        }

        // Resolve Dup targets pre-fork for InProcess stages (Word expansion may
        // allocate; not async-signal-safe). External stages handle this inside
        // spawn_external_with_fds itself.
        let (stdout_dup_target, stderr_dup_target) =
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                let sdt = match &exec.stdout {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                eprintln!("huck: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(r) = capture_read_fd {
                                    parent_held.retain(|&fd| fd != r);
                                    unsafe { libc::close(r); }
                                }
                                for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => None,
                };
                let sedt = match &exec.stderr {
                    Some(Redirect::Dup { source, .. }) => {
                        match resolve_fd_target(source, shell) {
                            Ok(fd) => Some(fd),
                            Err(e) => {
                                eprintln!("huck: {e}");
                                restore_inline_assignments(snap, shell);
                                if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                                if let Some(r) = capture_read_fd {
                                    parent_held.retain(|&fd| fd != r);
                                    unsafe { libc::close(r); }
                                }
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
                eprintln!("huck: {e}");
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
                if let Some(w) = heredoc_write_fd { unsafe { libc::close(w); } }
                // Exclude capture_read_fd from the drain: it will be closed
                // explicitly below, avoiding a double-close.
                if let Some(r) = capture_read_fd {
                    parent_held.retain(|&fd| fd != r);
                }
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

        // ---- Write heredoc body to child's stdin pipe -----------------------
        // The body was pre-expanded (while inline assignments were applied) and
        // stored in `heredoc_body_bytes`. Write it now so the child's stdin
        // doesn't block. Dropping `write_file` closes the write-end, signalling
        // EOF to the child.
        if let (Some(w), Some(bytes)) = (heredoc_write_fd.take(), heredoc_body_bytes.take()) {
            let mut write_file = unsafe { File::from_raw_fd(w) };
            let _ = write_file.write_all(&bytes);
            // write_file drops here, closing w — child sees EOF.
        }

        // ---- Track pid -------------------------------------------------------
        if interactive && first_pid.is_none() {
            first_pid = Some(pid);
        }
        stage_pids.push(pid);
        pipeline_stages.push(PipelineStage::Forked(pid));
    }

    // Close any remaining parent-held fds that weren't consumed
    // (e.g., if the last stage had an explicit stdout redirect, prev_pipe_read
    // might still hold a stale value from a stage with a broken pipe — but that
    // shouldn't happen in a well-formed pipeline).
    // The capture_read_fd is intentionally kept open until after wait.
    for fd in parent_held.iter().copied() {
        // Don't close the capture_read_fd; we need it after wait.
        if Some(fd) != capture_read_fd {
            unsafe { libc::close(fd); }
        }
    }
    parent_held.retain(|&fd| Some(fd) == capture_read_fd);

    // Give the terminal to the pipeline's process group if interactive.
    if interactive && let Some(pgid) = first_pid {
        give_terminal_to(pgid);
    }

    // ---- Wait for all stages ------------------------------------------------
    let last_status = wait_pipeline_raw(&pipeline_stages, &stage_pids, first_pid, shell, interactive);

    if interactive {
        give_terminal_to(shell.shell_pgid);
        if let PipelineWaitResult::Stopped(sig) = &last_status {
            let sig = *sig;
            // Intentionally do NOT set $PIPESTATUS here: bash does not set it
            // for a stopped (Ctrl-Z) pipeline. Capture fd cleanup before the
            // early return.
            if let Some(r) = capture_read_fd { unsafe { libc::close(r); } }
            return ExecOutcome::Continue(128 + sig);
        }
    }

    // ---- Read capture sink --------------------------------------------------
    if let Some(r) = capture_read_fd.take() {
        if let StdoutSink::Capture(buf) = sink {
            let mut f = unsafe { File::from_raw_fd(r) };
            let _ = io::copy(&mut f, *buf);
            // f drops here, closing r.
        } else {
            unsafe { libc::close(r); }
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
fn wait_pipeline_raw(
    stages: &[PipelineStage],
    stage_pids: &[i32],
    first_pid: Option<i32>,
    shell: &mut Shell,
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
                    eprintln!("\n{line}");
                    return PipelineWaitResult::Stopped(sig);
                }
                if libc::WIFEXITED(raw) || libc::WIFSIGNALED(raw) {
                    let s = if libc::WIFEXITED(raw) {
                        libc::WEXITSTATUS(raw)
                    } else {
                        128 + libc::WTERMSIG(raw)
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
                    *slot = Some(128 + libc::WTERMSIG(raw));
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
) -> Result<AssignmentSnapshot, AssignmentSnapshot> {
    let mut snap: AssignmentSnapshot = Vec::with_capacity(assignments.len());
    for a in assignments {
        let name = a.target.name();
        let prior = shell.snapshot_var(name);
        if shell.is_readonly(name) {
            eprintln!("huck: {name}: readonly variable");
            return Err(snap);
        }
        if apply_one_assignment(a, shell).is_err() {
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
        if matches!(&a.target, crate::command::AssignTarget::Bare(_))
            && !is_array_value_word(&a.value)
        {
            shell.export(name);
        }
        snap.push((name.to_string(), prior));
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
///   1. Bare + compound array RHS  →  `replace_array` / `append_array`
///   2. Bare + scalar RHS          →  `try_set` / scalar+=value
///   3. Indexed + scalar RHS       →  `set_array_element` / `append_array_element`
///   4. Indexed + compound array   →  rejected (matches bash)
///
/// Returns `Err(())` on readonly violation or other write failure
/// (diagnostic printed by the mutator). On success returns `Ok(())`.
pub(crate) fn apply_one_assignment(
    a: &crate::command::Assignment,
    shell: &mut Shell,
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
    let target_name = a.target.name();
    if shell.get_associative(target_name).is_some() {
        match (&a.target, trailing_array_literal) {
            (AssignTarget::Bare(name), Some(elements)) => {
                if a.append {
                    // Pre-validate readonly so the loop below cannot partial-write.
                    if shell.is_readonly(target_name) {
                        eprintln!("huck: {target_name}: readonly variable");
                        return Err(());
                    }
                    let new_pairs = build_associative_map(elements, shell)?;
                    for (k, v) in new_pairs {
                        shell
                            .set_associative_element(name, k, v)
                            .map_err(|_| ())?;
                    }
                    return Ok(());
                } else {
                    let pairs = build_associative_map(elements, shell)?;
                    return shell.replace_associative(name, pairs).map_err(|_| ());
                }
            }
            (AssignTarget::Bare(name), None) => {
                eprintln!(
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
                eprintln!(
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
                // a+=(elements): append new keys after max_index.
                let values: Vec<String> = elements
                    .iter()
                    .map(|e| expand_assignment(&e.value, shell))
                    .collect();
                shell.append_array(name, &values).map_err(|_| ())
            } else {
                // a=(elements): replace whole array.
                let map = build_array_map(elements, name, shell)?;
                shell.replace_array(name, map).map_err(|_| ())
            }
        }
        // Bare name + scalar RHS.
        (AssignTarget::Bare(name), None) => {
            let s = expand_assignment(&a.value, shell);
            if a.append {
                // a+=v: on a scalar, concatenate; on an array, append to element 0
                // (bash: `a=(x y); a+=z; echo "${a[0]}"` → "xz").
                match shell.get_array(name) {
                    Some(_) => shell
                        .append_array_element(name, 0, &s)
                        .map_err(|_| ()),
                    None => {
                        let existing = shell.get(name).map(str::to_string).unwrap_or_default();
                        shell.try_set(name, existing + &s).map_err(|_| ())
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
                Err(e) => {
                    eprintln!("huck: {e}");
                    return Err(());
                }
            };
            let v = expand_assignment(&a.value, shell);
            if a.append {
                shell.append_array_element(name, idx, &v).map_err(|_| ())
            } else {
                shell.set_array_element(name, idx, v).map_err(|_| ())
            }
        }
        // Subscripted lvalue + compound array RHS: bash rejects this.
        (AssignTarget::Indexed { name, .. }, Some(_)) => {
            eprintln!("huck: {name}: cannot assign array literal to array element");
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
) -> Result<Vec<(String, String)>, ()> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in elements {
        let key = match &e.subscript {
            Some(sw) => crate::expand::eval_subscript_key(sw, shell),
            None => {
                eprintln!("huck: associative array initializer requires [key]=value form");
                return Err(());
            }
        };
        let val = crate::param_expansion::expand_word_to_string(&e.value, shell);
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
fn build_array_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    name: &str,
    shell: &mut Shell,
) -> Result<std::collections::BTreeMap<usize, String>, ()> {
    let mut map: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    let mut implicit: usize = 0;
    for e in elements {
        let val = expand_assignment(&e.value, shell);
        let idx = match &e.subscript {
            Some(sw) => match crate::expand::eval_subscript(sw, shell, name) {
                Ok(i) => i,
                Err(msg) => {
                    eprintln!("huck: {msg}");
                    return Err(());
                }
            },
            None => implicit,
        };
        map.insert(idx, val);
        implicit = idx + 1;
    }
    Ok(map)
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
            // 2. Join the pgrp (or become pgrp leader if pgid_target == 0).
            libc::setpgid(0, pgid_target);
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
        // Anti-recursion guard: when a Command::Subshell is used as a
        // pipeline stage, the pipeline forks via this helper.  If we called
        // run_command here, it would fork AGAIN.  Instead, dispatch via
        // execute() so that body.background is honoured — `(cmd &)` inside a
        // subshell must background the inner command and let the subshell exit.
        // execute() calls execute_sequence_body when background is false
        // (the common case), preserving the single-fork invariant.
        let outcome = match cmd {
            Command::Subshell { body } => execute(body, shell, "(subshell)"),
            other => run_command(other, shell, &mut sink),
        };
        // 9. Translate outcome to an 8-bit exit status.
        let status: i32 = match outcome {
            ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
            ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
            ExecOutcome::FunctionReturn(n) => n,
        };
        let status = status.rem_euclid(256);
        // _exit bypasses Drop and Rust's atexit/flush machinery, which is
        // exactly what we want: the parent's history.save() etc. must not run.
        unsafe { libc::_exit(status) };
    }
    // PARENT: defensive setpgid to close the race with the child's setpgid.
    unsafe {
        libc::setpgid(pid, pgid_target);
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
fn spawn_external_with_fds(
    cmd: &SimpleCommand,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let SimpleCommand::Exec(exec) = cmd else {
        // Assign-only stages are classified as InProcess by classify_stage;
        // reaching here is a caller bug.
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "spawn_external_with_fds called on Assign stage"));
    };

    // Resolve (expand) the command — same path as run_exec_single / run_multi_stage.
    let resolved = resolve(exec, shell)
        .map_err(|code| io::Error::other(format!("resolve failed with code {code}")))?;

    // Resolve Dup targets pre-fork (Word expansion may allocate; not async-signal-safe).
    // stdout-dup BEFORE stderr-dup matches canonical `>file 2>&1` semantics.
    let stdout_dup_target: Option<i32> = match &exec.stdout {
        Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
        _ => None,
    };
    let stderr_dup_target: Option<i32> = match &exec.stderr {
        Some(Redirect::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
        _ => None,
    };

    let mut process = ProcessCommand::new(&resolved.program);
    process.args(&resolved.args);
    process.env_clear();
    process.envs(shell.exported_env());

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

    // Join the pgrp (or become pgrp leader if pgid_target == 0).
    process.process_group(pgid_target);

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
    // The closure must be async-signal-safe; libc::close is.
    let fds_to_close: Vec<RawFd> = parent_fds_to_close.to_vec();
    unsafe {
        process.pre_exec(move || {
            for &fd in &fds_to_close {
                libc::close(fd);
            }
            Ok(())
        });
    }

    let child = process.spawn()?;
    let pid = child.id() as i32;

    // Defensive setpgid in parent to close the race with the child's setpgid
    // (set via process_group above, which runs pre-exec in the child).
    unsafe {
        let _ = libc::setpgid(pid, pgid_target);
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
            stdin: None,
            stdout: None,
            stderr: None,
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
                ]))],
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
                ]))],
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
        let result = expand_single(&word, &mut shell);

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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
                stdin: None,
                stdout: None,
                stderr: None,
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
                    stdin: None,
                    stdout: None,
                    stderr: None,
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
        let snap = apply_inline_assignments(&assigns, &mut shell).expect("ok");
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
        let snap = apply_inline_assignments(&assigns, &mut shell).expect("ok");
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
        let snap = apply_inline_assignments(&assigns, &mut shell).expect("ok");
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
        let snap = apply_inline_assignments(&assigns, &mut shell).expect("ok");
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
        let snap = apply_inline_assignments(&assigns, &mut shell).expect("ok");
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
            stdin: None,
            stdout: None,
            stderr: None,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=inner true");
        assert_eq!(shell.get("FOO"), Some("outer"));
        assert!(!shell.is_exported("FOO"));
    }

    #[test]
    fn run_exec_single_function_call_inline_assignment_persists() {
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
            stdin: None,
            stdout: None,
            stderr: None,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=val myfunc");
        assert_eq!(shell.get("FOO"), Some("val"));
    }

    #[test]
    fn run_exec_single_special_builtin_inline_assignment_persists() {
        let mut shell = Shell::new();
        let cmd = SimpleCommand::Exec(ExecCommand {
            inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
            program: lit_word("export"),
            args: vec![lit_word("FOO")],
            stdin: None,
            stdout: None,
            stderr: None,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=val export FOO");
        assert_eq!(shell.get("FOO"), Some("val"));
        assert!(shell.is_exported("FOO"));
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

    // ----- classify_stage unit tests (Task 4) ----------------------------------

    /// Helper: builds `Command::Simple(SimpleCommand::Exec(...))` for `program`.
    fn simple_exec_cmd(program: &str) -> Command {
        Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: lit_word(program),
            args: vec![],
            stdin: None,
            stdout: None,
            stderr: None,
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
            stdin: None,
            stdout: None,
            stderr: None,
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
        ]));
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
            stdin: None,
            stdout: None,
            stderr: None,
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
            stdin: None,
            stdout: None,
            stderr: None,
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
            stdin: None,
            stdout: None,
            stderr: None,
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
            stdin: None,
            stdout: None,
            stderr: None,
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
    fn call_function_pushes_arg0_during_body() {
        // Define a function whose body reads $0 into a var; verify the var
        // contains the function name after the call.
        let mut shell = Shell::new();
        exec_script("myfunc() { CAPTURED=$0; }\nmyfunc\n", &mut shell);
        assert_eq!(shell.get("CAPTURED"), Some("myfunc"));
    }

    #[test]
    fn call_function_pops_arg0_after_return() {
        let mut shell = Shell::new();
        exec_script("myfunc() { :; }\nmyfunc\n", &mut shell);
        assert!(shell.function_arg0.is_empty(),
            "function_arg0 should be empty after function returns, got: {:?}",
            shell.function_arg0);
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
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn sparse_compound_assign_respects_explicit_subscripts() {
        let mut s = Shell::new();
        run_line(&mut s, "a=([5]=x [2]=y)");
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&5).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("y"));
    }

    #[test]
    fn element_assign_creates_array() {
        let mut s = Shell::new();
        run_line(&mut s, "a[3]=hello");
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.get(&3).map(String::as_str), Some("hello"));
    }

    #[test]
    fn element_assign_promotes_scalar() {
        let mut s = Shell::new();
        run_line(&mut s, "a=old");
        run_line(&mut s, "a[2]=new");
        let m = s.get_array("a").expect("scalar should promote to array");
        assert_eq!(m.get(&0).map(String::as_str), Some("old"));
        assert_eq!(m.get(&2).map(String::as_str), Some("new"));
    }

    #[test]
    fn append_array_extends() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y)");
        run_line(&mut s, "a+=(z w)");
        let m = s.get_array("a").unwrap();
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
        let m = s.get_array("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("hello_world"));
    }

    #[test]
    fn readonly_blocks_compound_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a=(changed)");
        let m = s.get_array("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("initial"));
    }

    #[test]
    fn readonly_blocks_element_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a[5]=new");
        let m = s.get_array("a").unwrap();
        assert!(m.get(&5).is_none());
    }

    #[test]
    fn unset_element_removes_one_key() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a[1]");
        let m = s.get_array("a").unwrap();
        assert!(m.get(&1).is_none());
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn unset_whole_array_removes_variable() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a");
        assert!(s.get_array("a").is_none());
        assert!(s.get("a").is_none());
    }

    #[test]
    fn scalar_append_to_existing_array_writes_element_zero() {
        // `a=(x y); a+=z` in bash appends to element 0 (i.e. concatenates
        // with a[0]), yielding a[0]="xz".
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y)");
        run_line(&mut s, "a+=z");
        let m = s.get_array("a").expect("still an array");
        assert_eq!(m.get(&0).map(String::as_str), Some("xz"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    }

    #[test]
    fn indexed_lvalue_compound_rhs_rejected() {
        // `a[i]=(...)` is a syntax-level error in bash; huck rejects
        // it with a diagnostic and leaves `a` empty.
        let mut s = Shell::new();
        run_line(&mut s, "a[0]=(x y)");
        assert!(s.get_array("a").is_none());
    }

    #[test]
    fn unset_with_empty_subscript_errors() {
        // bash treats `unset a[]` as a syntax error
        // ("bad array subscript") and leaves `a` untouched.
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a[]");
        let m = s.get_array("a").expect("a should still exist");
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
        assert!(s.get_array("m").is_some());
        assert!(s.get_associative("m").is_none());
        assert_eq!(s.lookup_array_element("m", 0), Some("bar".into()));
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
        assert!(s.get_array("bar").is_some(), "bar should be indexed");
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
