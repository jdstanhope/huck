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

/// Refuse a file-target output redirect under a restricted policy. Input-only
/// (`ReadOnly`) is never refused, and fd-duplication never reaches here — both
/// match bash, where `<`, `>&2`, and `2>&1` stay permitted under `-r`.
///
/// `subject` is the word to NAME in the diagnostic; it differs from `path` for
/// a `{var}`-fd redirect, where bash names the variable (`{v}> f` reports `v`,
/// not `f`). The policy decision itself always looks at `path`.
#[inline]
fn check_restricted_redirect(
    mode: &FileMode,
    path: &str,
    subject: &str,
    shell: &Shell,
) -> Result<(), ()> {
    if !matches!(
        mode,
        FileMode::Truncate | FileMode::Append | FileMode::Clobber | FileMode::ReadWrite
    ) {
        return Ok(());
    }
    if let Err(msg) = shell
        .policy
        .check(crate::policy::Op::RedirectFile { path, subject })
    {
        let mut err = err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        return Err(());
    }
    Ok(())
}

/// Materialize the stderr writer. Production is one-model (#197 Stage 3): this
/// is a `CaptureStderr` over `io::stderr()` — real writes go to fd 2, and a
/// `#[cfg(test)]` capture (if active on the thread) intercepts them into the
/// captured stderr buffer.
pub(crate) fn err_writer() -> Box<dyn std::io::Write> {
    Box::new(crate::fd_writer::CaptureStderr::new(
        crate::fd_writer::CaptureStream::Err,
    ))
}

/// Emit a path-bearing redirect-open failure in bash's format:
/// `<prologue>{path}: {strerror}` via `sh_error_to!` — the non-interactive
/// prologue (`<src>: line N: `) or interactive `huck: `, the offending path,
/// and `bash_io_error` (strerror with no Rust `(os error N)` suffix). Single
/// home for every `open_redirect_file` redirect-open
/// error so the format stays identical across execution contexts. Routes
/// through the CALLER's redirect-aware writer (built from `err_sink`/`out_sink`)
/// rather than the thread-local sink, so an inner `2>&1`/capture on the
/// surrounding command is honored (v269 T4fix).
pub(crate) fn redir_open_error(shell: &Shell, path: &str, e: &std::io::Error) {
    let mut err = err_writer();
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
    if shell.shell_options.errexit && !shell.errexit_suppressed() && status != 0 {
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
/// Which question a checkpoint is asking.
///
/// The two phases differ TODAY, and v354 preserves that asymmetry rather than
/// normalising it — making every site consult everything would fire outcomes
/// at sites that have never consulted them, which is a behaviour change
/// wearing a refactor's clothes. Writing the two orders down here is the
/// point: previously they were implied by statement order in two functions in
/// different parts of the file.
pub(crate) enum UnwindPhase {
    /// Around a command — the six `check_interrupt` sites and the wait loops.
    /// SIGINT -> timeout -> exit. Never consults discard or fatal.
    Around,
    /// After a command produced `Continue(c)` — `finish_command` only.
    /// discard -> fatal -> exit. Never consults the atomics.
    After,
}

/// The single place that decides what stops a command, and in what order.
///
/// REPORTS but never CONSUMES: the caller does the taking, because a discard's
/// take must pair with `set_last_status(1)` (#351) and a `&Shell` reporter
/// cannot write that.
pub(crate) fn pending_unwind(shell: &Shell, phase: UnwindPhase) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;
    match phase {
        UnwindPhase::Around => {
            // SIGINT is CONSUMED here (compare_exchange). When SIGINT is
            // trapped we return None HAVING CLEARED it, so the trap action
            // runs instead of the command being interrupted.
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
            // The timeout flag stays SET — the builder's epilogue does a
            // single `swap(false)` at the run boundary to override to 124.
            if shell.timeout_flag.load(Ordering::Relaxed) {
                return Some(ExecOutcome::Interrupted(InterruptReason::Timeout));
            }
            // #442: peeked, not taken — consumed at the run boundary. Reported
            // LAST: SIGINT and a timeout are externally imposed and outrank
            // the script's own request.
            shell
                .exit_pending()
                .map(|n| ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)))
        }
        UnwindPhase::After => {
            // v312 (#3/#49): the discard flavour wins if both it and a fatal
            // were raised by the same command.
            if shell.discard_pending() {
                return Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand));
            }
            // A pending fatal is NOT reported as an outcome here: its arm must
            // return `Continue(c)`, and `c` belongs to the caller, so
            // reporting it would mean inventing a status. `finish_command`
            // keeps an explicit `fatal_pending()` check immediately after this
            // call, in the position it occupies today.
            if shell.fatal_pending() {
                return None;
            }
            shell
                .exit_pending()
                .map(|n| ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)))
        }
    }
}

/// Around-phase checkpoint. A named wrapper so its six call sites read
/// unchanged.
pub(crate) fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    pending_unwind(shell, UnwindPhase::Around)
}

/// Runs a top-level sequence, sending the terminal pipeline-stage's stdout to
/// the real fd table. Production is one-model (#197 Stage 3); `execute` is a
/// thin wrapper.
pub fn execute_with_sink(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    // #184: mark this thread as executing for the duration of this call, so the
    // fork check in `fork_and_run_in_subshell` can tell whether any OTHER thread
    // is executing. Re-entrant (nested constructs, eval/source, function
    // bodies); the counters stay balanced. Dropped last, on scope exit/panic.
    let _exec_active = crate::exec_guard::ExecActive::enter();
    execute_with_sink_inner(seq, shell, source)
}

fn execute_with_sink_inner(seq: &Sequence, shell: &mut Shell, _source: &str) -> ExecOutcome {
    // Fast path: a trailing-`&` that backgrounds a SINGLE and-or group (no
    // `&`-separators inside the list). This preserves the real source-derived
    // job-display label for the common `cmd &` / `a && b &` / `a | b &` forms.
    // Sequences containing `Connector::Amp` (mid-list `&` separators) fall
    // through to the group-aware `execute_sequence_body`.
    let has_amp_separator = seq.rest.iter().any(|(c, _)| matches!(c, Connector::Amp));
    if seq.background && !has_amp_separator {
        if seq.rest.is_empty() {
            // Single-pipeline or subshell backgrounded — existing paths. Deparse
            // the unit for the normalized `jobs` display (#80).
            if let Command::Pipeline(p) = &seq.first {
                let display = render_job_command(&seq.first);
                return run_background_sequence(p, shell, &display);
            }
            if let Command::Subshell { .. } = &seq.first {
                let display = render_job_command(&seq.first);
                return run_background_subshell(&seq.first, shell, false, &display);
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
            // Deparse the and-or group itself (not the synthetic subshell wrapper)
            // so `jobs` shows `a && b`, matching bash (#80).
            let display = render_job_sequence(&inner);
            let subshell = Command::Subshell {
                body: Box::new(inner),
            };
            return run_background_subshell(&subshell, shell, false, &display);
        }
    }
    execute_sequence_body(seq, shell)
}

/// Runs a top-level sequence with stdout going to the terminal. Thin wrapper
/// over `execute_with_sink` with a Terminal sink.
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    execute_with_sink(seq, shell, source)
}

/// Runs a sequence with stdout captured to a buffer. Used by command
/// substitution; the trailing `&` is ignored here because substitutions
/// must complete before their output is interpolated. Spawning real
/// background children whose pids the parent's JobTable doesn't track
/// would let them escape `wait`/`jobs` and litter the terminal.
///
/// Stage 3 (#197): the `Capture` sink is gone. This runs the body with
/// `Terminal` sinks under a `capture_test_hook` thread-local capture, which
/// intercepts in-process (builtin) stdout at the writer chokepoints. stderr is
/// NOT captured (`capture_err = false`) so it still reaches the real fd 2,
/// preserving this path's old terminal-stderr behavior. It is `#[cfg(test)]`
/// only — production command substitution forks a real subshell over a pipe
/// (`capture_via_fork`), which captures external output too.
#[cfg(test)]
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
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
    let (buf, _err, outcome) = crate::capture_test_hook::with_capture(false, false, || {
        // stderr inside $() still inherits the process (real fd 2); capturing
        // stderr through command substitution isn't plumbed here.
        execute_sequence_body(&sanitized, shell)
    });
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
                // v312 (#3/#49): a comsub boundary CONTAINS the discard — return
                // 1 WITHOUT re-raising a flag, so the enclosing command
                // CONTINUES (bash: `x=$( echo $((3.5)) ); echo after` runs
                // `after`). Unlike the Sigint/Timeout arms above, no flag is
                // re-stored: the discard died at this boundary.
                InterruptReason::DiscardCommand => 1,
                // #442: an exit request is NOT contained here — bash's
                // `trap "exit 9" ERR; x=$(false); echo after` exits 9 and never
                // runs `after`. `check_interrupt` PEEKS at `pending_exit`
                // rather than taking it, so the flag is still set and the
                // enclosing list observes it at its next checkpoint; nothing to
                // re-store.
                InterruptReason::ExitRequested(n) => n,
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
/// The post-command epilogue shared by every element of an and-or list.
///
/// Runs after a command has produced `Continue(c)`: status propagation, the two
/// pending-unwind checks, signal-trap dispatch, the ERR trap and errexit.
/// Returns `Some(outcome)` when the caller must return it immediately, `None`
/// to carry on with the rest of the list.
///
/// bash's and-or rule — only the SYNTACTICALLY last command of a list fires
/// ERR/errexit — is no longer a flag here (v356). An exempt element runs
/// inside a suppression scope raised by `run_list_element`, so this reads that
/// state instead. `err_armed` is the PRE-command snapshot of whether the ERR
/// trap was set (#444, bash's `was_error_trap`), so a command that installs
/// the ERR trap is not itself caught by it; errexit is deliberately NOT gated
/// on it.
fn finish_command(
    cmd: &Command,
    c: i32,
    err_armed: bool,
    shell: &mut Shell,
) -> Option<ExecOutcome> {
    // B-11: propagate `$?` across sequence connectors. The top-level loop in
    // shell.rs only refreshes `shell.last_status` after `process_line` returns,
    // so without this update the second command in `false; echo $?` would see a
    // stale value.
    shell.set_last_status(c);
    // v312 (#3/#49): a pending arithmetic-discard converts this command's
    // outcome into the Interrupted(DiscardCommand) unwind — the current
    // top-level command is discarded (out of loops/functions), contained at a
    // comsub boundary, and continued (not exited) at the driver loop. Checked
    // BEFORE pending_fatal_status: the discard flavor wins if both were somehow
    // raised by the same command.
    // v354 (#466): the reporter DECIDES (precedence in one place), the caller
    // CONSUMES. This is the call where all three shell-raised signals can be
    // pending at once, so it is what makes the `After` precedence
    // load-bearing rather than decorative.
    match pending_unwind(shell, UnwindPhase::After) {
        Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand)) => {
            shell.take_discard();
            // #351: a discarded command's exit status is 1 (DiscardCommand
            // maps to 1 at the driver). `set_last_status(c)` above wrote the
            // "success" status of a bare assignment (`v=$((1/0))` → c=0);
            // overwrite it so a following `$?` reads 1, matching bash.
            shell.set_last_status(1);
            return Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand));
        }
        Some(other) => return Some(other),
        None => {}
    }
    // A pending fatal returns `Continue(c)`, which the reporter cannot build
    // (it has no `c`) — hence an explicit check here, in the position it has
    // always occupied.
    if shell.fatal_pending() {
        return Some(ExecOutcome::Continue(c));
    }
    crate::traps::dispatch_pending_traps(shell);
    // #442: a trap action (including one just dispatched above) ran `exit N`.
    // Checked BEFORE errexit so a trap's exit beats the errexit status — bash's
    // `set -e; trap "exit 9" ERR; false` exits 9, not 1.
    if let Some(o) = pending_unwind(shell, UnwindPhase::After) {
        return Some(o);
    }
    if c != 0 && !shell.err_trap_suppressed() && !is_negated_pipeline(cmd) {
        if err_armed && !body_already_fired_err(cmd) {
            crate::traps::fire_err_trap(shell);
            // #442: the ERR action itself ran `exit N`. Checked here, AFTER the
            // fire and BEFORE errexit, so a trap's exit beats the errexit
            // status: bash's `set -e; trap "exit 9" ERR; false` exits 9, not 1.
            if let Some(o) = pending_unwind(shell, UnwindPhase::After) {
                return Some(o);
            }
        }
        if let Some(out) = maybe_errexit(shell, c) {
            return Some(out);
        }
    }
    None
}

/// Fires the DEBUG trap and folds in #442: an `exit N` performed by the DEBUG
/// action must abort the PENDING command, because DEBUG fires BEFORE it —
/// bash's `trap "exit 9" DEBUG; echo a` never prints `a`. `Err` carries the
/// outcome the dispatch site must return immediately; `Ok` is the usual
/// Proceed / SkipCommand / ReturnFromSub decision.
fn debug_trap_gate(shell: &mut Shell) -> Result<crate::traps::DebugDecision, ExecOutcome> {
    let decision = crate::traps::fire_debug_trap(shell);
    match shell.exit_pending() {
        Some(n) => Err(ExecOutcome::Interrupted(InterruptReason::ExitRequested(n))),
        None => Ok(decision),
    }
}

/// Stamps `$BASH_COMMAND` and fires the DEBUG trap for a command whose runner
/// does not do it itself — `[[ ]]` and `(( ))`, whose runners take destructured
/// parts rather than the `Command`. Called from the dispatch arm, which is
/// where the `Command` is still in hand and, not coincidentally, BEFORE the
/// runner emits its own `set -x` line: bash runs the DEBUG action first. #269
///
/// `Err(outcome)` is the caller's immediate return value. A DEBUG-skipped
/// command yields 0 here, the same as `case` and a simple command (measured;
/// it is NOT the previous command's status).
fn debug_gate_for_command(cmd: &Command, line: u32, shell: &mut Shell) -> Result<(), ExecOutcome> {
    // $LINENO is stamped BEFORE the fire, not left to the runner: the action
    // reads `$LINENO` and bash reports the line of the command about to run,
    // not the previous one. The runners stamp it again with the same value.
    if line != 0 {
        shell.current_lineno = shell.line_base() + line;
    }
    // Same freeze rule as `run_single`: while any pseudo-trap action is
    // running, its own commands must not re-stamp $BASH_COMMAND.
    //
    // `command_to_source` is the renderer bash's own text matches — verified
    // byte-for-byte through `type f` for both shapes, including the asymmetry
    // that `[[ ]]` normalizes its spacing while `(( ))` keeps the source
    // verbatim, and that neither expands `$x`.
    if shell.firing_traps.is_empty() {
        shell.current_command = crate::generate::command_to_source(cmd, 0);
    }
    match debug_trap_gate(shell)? {
        crate::traps::DebugDecision::Proceed => Ok(()),
        crate::traps::DebugDecision::SkipCommand => Err(ExecOutcome::Continue(0)),
        crate::traps::DebugDecision::ReturnFromSub(n) => Err(ExecOutcome::FunctionReturn(n)),
    }
}

/// Runs ONE element of an and-or list: the exempt scope, the command, the
/// interrupt checkpoint, control-flow propagation, and the post-command
/// epilogue. `Err(outcome)` means the caller must return it immediately;
/// `Ok(status)` means carry on with the list.
///
/// `exempt` is bash's ignore-return: an element that is NOT the syntactically
/// last of its list is "part of a list being tested", so neither it nor
/// anything it runs counts — not the brace group's statements, not a
/// function's body, not a forked subshell's. The scope therefore spans the
/// body AND the epilogue, which is what lets `finish_command` decide from
/// suppression state instead of carrying a separate `is_last` flag: one
/// question, one mechanism.
///
/// Owning both ends here is also what makes the scope leak-proof. There is a
/// single exit path, where previously the raise and the lower straddled five
/// early returns — and a leak would have made `set -e` silently stop working
/// for the rest of the list.
fn run_list_element(
    cmd: &Command,
    exempt: bool,
    shell: &mut Shell,
) -> Result<ExecOutcome, ExecOutcome> {
    // #444 (bash's `was_error_trap`): snapshot BEFORE the command runs, so a
    // command that INSTALLS the ERR trap is not itself caught by it.
    let err_armed = crate::traps::err_trap_armed(shell);
    if exempt {
        shell.suppress_both();
    }
    let status = run_command(cmd, shell);
    let out = 'elem: {
        if let Some(o) = check_interrupt(shell) {
            break 'elem Err(o);
        }
        if matches!(
            status,
            ExecOutcome::Exit(_)
                | ExecOutcome::LoopBreak(_, _)
                | ExecOutcome::LoopContinue(_)
                | ExecOutcome::FunctionReturn(_)
                | ExecOutcome::Interrupted(_)
        ) {
            break 'elem Err(status);
        }
        if let ExecOutcome::Continue(c) = status
            && let Some(o) = finish_command(cmd, c, err_armed, shell)
        {
            break 'elem Err(o);
        }
        Ok(status)
    };
    if exempt {
        shell.unsuppress_both();
    }
    out
}

fn run_andor_group(
    first: &Command,
    rest: &[(Connector, &Command)],
    shell: &mut Shell,
) -> ExecOutcome {
    // An element is exempt iff it is not the syntactically last of the list.
    let mut status = match run_list_element(first, !rest.is_empty(), shell) {
        Ok(s) => s,
        Err(o) => return o,
    };
    for i in 0..rest.len() {
        let (connector, command) = &rest[i];
        let should_run = match connector {
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
            // Semi/Amp are group boundaries; they never appear inside a group.
            Connector::Semi | Connector::Amp => true,
        };
        if should_run {
            status = match run_list_element(command, i + 1 != rest.len(), shell) {
                Ok(s) => s,
                Err(o) => return o,
            };
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

fn execute_sequence_body(seq: &Sequence, shell: &mut Shell) -> ExecOutcome {
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
            // Deparse the whole backgrounded and-or group to a normalized
            // bash-style command line for the `jobs` display (#80). Rendered from
            // the AST (pre-expansion), so `sleep $x && echo hi` shows unexpanded.
            // bash: a bare multi-stage pipeline backgrounded via `&` keeps the
            // shell's stdin on stage 0; every other async unit gets /dev/null.
            let inherit_stdin = group.rest.is_empty()
                && matches!(group.first, Command::Pipeline(p) if p.commands.len() > 1);
            let display = render_job_sequence(&inner);
            let subshell = Command::Subshell {
                body: Box::new(inner),
            };
            // Launch; ignore the Continue(0) it returns — the foreground status
            // is unchanged by a background launch.
            run_background_subshell(&subshell, shell, inherit_stdin, &display);
        } else {
            last_status = run_andor_group(group.first, &group.rest, shell);
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
        // #175/#418: between-command job-table maintenance — reap, announce any
        // state change, prune the terminal jobs. bash notices a death while
        // running the command, so the notice lands AFTER that command's output
        // and carries ITS line number; running the pass at the end of the group
        // reproduces both. The interactive REPL does the same per prompt
        // (`repl.rs`), so this is restricted to non-interactive shells to avoid
        // a mid-line notice there.
        if !shell.is_interactive {
            let blocked = std::mem::take(&mut shell.blocked_on_child);
            crate::jobs::reap_and_notify_ex(shell, blocked);
        }
    }
    last_status
}

/// Dispatches a single sequence element.
fn run_command(cmd: &Command, shell: &mut Shell) -> ExecOutcome {
    // `set -n` / `-n` (noexec): read and parse but do not execute. Per-command
    // and non-interactive only (bash ignores -n interactively). Parsing already
    // happened (the reader caught any syntax error) — we simply skip running.
    // Once on, this also skips a later `set +n`, so noexec cannot be turned back
    // off mid-script — matching bash.
    if shell.shell_options.noexec && !shell.is_interactive {
        return ExecOutcome::Continue(0);
    }
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell),
        Command::Simple(s) => run_single(s, shell),
        Command::If(clause) => run_if(clause, shell),
        Command::While(clause) => run_while(clause, shell),
        Command::For(clause) => run_for(clause, shell),
        Command::Case(clause) => run_case(clause, shell),
        Command::BraceGroup(seq) => execute_sequence_body(seq, shell),
        Command::Subshell { .. } => {
            // Stage 3 (#197): the subshell's stdout/stderr always inherit the
            // shell's real fd 1/2. The deleted `Capture`/`Merged` sinks used to
            // wire per-stream capture pipes here; production command
            // substitution now forks its own subshell over a pipe elsewhere
            // (`capture_via_fork`), so a plain `( … )` never captures.
            let interactive = shell.job_control_active();
            let child_stdio = ChildStdio::new(ChildFd::Inherit, ChildFd::Inherit, ChildFd::Inherit);

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
                    let mut err = err_writer();
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "fork: {}",
                        crate::bash_io_error(&e)
                    );
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

            if interactive {
                // Interactive subshell path keeps the wait_with_untraced shape,
                // preserving job-control / stopped-job semantics for the REPL.
                // stdout/stderr inherit fd 1/2, so there are no capture pipes.

                // Foreground subshell: make it a job that owns the terminal,
                // mirroring the single-command/pipeline dance. Without this the
                // subshell runs in a background pgroup and deadlocks on tty I/O.
                // (fork_and_run_in_subshell already race-closes setpgid in the
                // parent; this is belt-and-suspenders to guarantee the pgrp
                // exists before give_terminal_to, mirroring the pipeline path.)
                setpgid_self(pid);
                // #167: only hand the terminal to the job group when we own a
                // controlling tty. Under `set -m` in a pipe the setpgid + wait
                // still happen, matching bash's non-interactive job control.
                if stdin_is_tty() {
                    give_terminal_to(pid);
                }
                note_blocked_on_child(shell);
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
                            let mut err = err_writer();
                            e!(&mut *err, "\n{line}");
                        }
                        128 + sig
                    }
                    Ok((raw_status, false)) => raw_status_to_exit_code(raw_status, shell),
                    Err(()) => 1,
                };
                if stdin_is_tty() {
                    give_terminal_to(shell.shell_pgid);
                }
                shell.set_pipestatus(&[outcome]);
                ExecOutcome::Continue(outcome)
            } else {
                // Non-interactive (script), nested subshell, or completion:
                // block until the child exits, on the embedder's thread.
                // #418: about to BLOCK on a foreground child — bash reaps (and so may
                // announce a background job's death) exactly at such a point.
                note_blocked_on_child(shell);
                let loop_result = crate::stream_loop::wait_for_child(pid as libc::pid_t);
                let raw_status = match loop_result {
                    Ok(s) => s,
                    Err(_) => return ExecOutcome::Continue(1),
                };
                let code = raw_status_to_exit_code(raw_status, shell);
                shell.set_pipestatus(&[code]);
                ExecOutcome::Continue(code)
            }
        }
        Command::FunctionDef { name, body, line } => {
            // POSIX: a function may not be named after a special builtin; a
            // non-interactive posix shell errors and exits (default mode allows it).
            if shell.shell_options.posix && builtins::is_special_builtin(name) {
                {
                    let mut err = err_writer();
                    crate::sh_error_to!(shell, &mut *err, None, "{name}: is a special builtin");
                }
                shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage {
                    status: 2,
                });
                return ExecOutcome::Continue(2);
            }
            shell.define_function(name.clone(), body.clone(), *line);
            ExecOutcome::Continue(0)
        }
        Command::DoubleBracket {
            expr,
            inline_assignments,
            line,
        } => {
            // #269: these two runners take destructured parts, so their DEBUG
            // fire lives here where the `Command` is still available.
            if let Err(o) = debug_gate_for_command(cmd, *line, shell) {
                return o;
            }
            run_double_bracket(expr, inline_assignments, *line, shell)
        }
        Command::ArithFor(clause) => run_arith_for(clause, shell),
        Command::Arith(expr, line) => {
            if let Err(o) = debug_gate_for_command(cmd, *line, shell) {
                return o;
            }
            run_arith(expr, *line, shell)
        }
        Command::Select(clause) => run_select(clause, shell),
        Command::Redirected { inner, redirects } => run_redirected(inner, redirects, shell),
        Command::Coproc { name, body } => run_coproc(name, body, shell),
        _ => {
            {
                let mut err = err_writer();
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
/// v156: generalized into the ordered in-process redirect applier. `apply_redirects`
/// resolves each redirection with `lower_one_redirect` and applies its ops to the
/// real fds (open/dup2/close) INTERLEAVED in source order, so e.g. `2>&1 >file`
/// differs from `>file 2>&1` and a `{var}`'s side effects (assign `$v`, allocate a
/// persistent fd) are visible to the next redirection. A failure mid-list returns
/// `Err(outcome)`; Drop then rolls back the entries already applied (atomic).
/// `dup(fd)` into an OWNED handle, or `None` on EBADF (the fd was not open).
/// The returned `OwnedFd` closes the dup on drop — the RAII basis for
/// `RedirectScope`'s save/restore (#197 Class-A).
fn own_dup(fd: RawFd) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    // #398: park the saved copy at a HIGH fd. A plain `dup(2)` takes the LOWEST
    // free descriptor, so once a redirect list has CLOSED one — `echo hi >&-
    // 2>/dev/null` frees fd 1 — the very next redirect's save-dup lands ON the
    // fd that was just closed. fd 1 came back open (pointing at fd 2's original
    // target), the builtin's write succeeded, and the close the user asked for
    // had silently evaporated. bash keeps its saved fds out of the user-visible
    // range for exactly this reason.
    //
    // CLOEXEC too: the saved copy is the shell's own bookkeeping and has no
    // business surviving into an exec'd program.
    let d = crate::child_fd::dup_to_high_fd(fd, 10, true).ok()?;
    Some(unsafe { std::os::fd::OwnedFd::from_raw_fd(d) })
}

struct RedirectScope {
    /// Per redirect: the real fd we replaced, and an OWNED dup of its original
    /// contents (`None` when the target was unopened). Drop restores from the
    /// owned dup, whose destructor then closes it — no manual close (#197 Class-A).
    saved: Vec<(RawFd, Option<std::os::fd::OwnedFd>)>,
}

impl RedirectScope {
    fn new() -> Self {
        RedirectScope { saved: Vec::new() }
    }

    /// Replace `target_fd` with a dup of `new_fd`, saving the original so Drop
    /// can restore it. `new_fd` is NOT consumed (caller closes it). If
    /// `target_fd` is not currently open, the saved slot is recorded as `-1`
    /// (Drop closes it back to unopened) — bash leaves a fresh high fd open
    /// only for the command's duration.
    fn redirect(&mut self, shell: &Shell, new_fd: RawFd, target_fd: RawFd) -> Result<(), ()> {
        unsafe {
            // `dup` fails with EBADF when target_fd is not open (e.g. a fresh
            // fd>2 like `>&3` when fd 3 was never opened). That is fine — record
            // `None` so Drop closes target_fd back to its unopened state.
            let saved = own_dup(target_fd);
            if libc::dup2(new_fd, target_fd) < 0 {
                {
                    let mut err = err_writer();
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "dup2: {}",
                        io::Error::last_os_error()
                    );
                }
                // `saved` (if Some) drops here → closes the dup automatically.
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
            // Record the swap so Drop restores (`None` if it was unopened).
            let saved = own_dup(target_fd);
            self.saved.push((target_fd, saved));
            libc::close(target_fd);
        }
    }

    /// Apply a single lowered `PlanOp` to the real fds, save/restore aware
    /// (Drop rolls back). Dup sources are NOT re-validated here: the caller's
    /// `lower_one_redirect(None)` already validated the source against the real
    /// fd table, and lower→apply for a single redirect is atomic w.r.t. the fd
    /// table, so re-validation would be redundant.
    fn apply_one(&mut self, op: PlanOp, shell: &mut Shell) -> Result<(), ExecOutcome> {
        use std::os::fd::{AsRawFd, IntoRawFd};
        match op {
            PlanOp::InstallOwned { target, source } => {
                let raw = source.as_raw_fd();
                if raw == target {
                    let fd = source.into_raw_fd();
                    unsafe {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags >= 0 {
                            let _ = libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                        }
                    }
                    self.saved.push((target, None));
                } else if self.redirect(shell, raw, target).is_err() {
                    return Err(ExecOutcome::Continue(1));
                } else {
                    drop(source);
                }
            }
            PlanOp::InstallDup { target, source } => {
                if self.redirect(shell, source, target).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
            }
            PlanOp::Close { target } => self.close_target(target),
            PlanOp::NamedFd {
                high,
                name,
                virtual_fd: _,
            } => {
                let fd = high.into_raw_fd();
                shell.set(&name, fd.to_string());
            }
        }
        Ok(())
    }

    /// Resolve-then-apply each redirection in source order against the shell's
    /// real fds. Interleaved, so a `{var}`'s $v assignment + fd allocation and
    /// each redirect's fd-table mutation are visible to the NEXT redirect —
    /// matching bash and the pre-C `apply`.
    fn apply_redirects(
        &mut self,
        redirs: &[Redirection],
        shell: &mut Shell,
    ) -> Result<(), ExecOutcome> {
        for redir in redirs {
            let ops = lower_one_redirect(redir, shell, None).map_err(ExecOutcome::Continue)?;
            for op in ops {
                self.apply_one(op, shell)?;
            }
        }
        Ok(())
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
                if let Some(owned) = saved {
                    use std::os::fd::AsRawFd;
                    libc::dup2(owned.as_raw_fd(), target_fd);
                    // `owned` drops here → closes the dup automatically.
                } else {
                    // target_fd was not open before we redirected it — close it
                    // back to its unopened state.
                    libc::close(target_fd);
                }
            }
        }
        // #169/v307: there is nothing to reap here any more. Heredoc bodies are
        // delivered by `heredoc_body_to_fd` (pipe or temp file) with no forked
        // writer, so the restore-then-reap ordering #142 needed — restore first so
        // a writer blocked on a full pipe gets EPIPE and can be waited on — is
        // moot. The guard remains in heredoc_redirect_fail_hang_diff_check.sh.
    }
}

/// True if `cmd` carries any explicit redirect — the gate for wrapping a
/// body-running command (function / eval / source) in `with_redirect_scope`.
fn has_any_redirect(cmd: &ExecCommand) -> bool {
    !cmd.redirects.is_empty()
}

/// The final effective destination of a fd after applying a redirect list
/// left-to-right (source order). Used by `final_dests_for_1_2` to decide
/// whether the builtin-redirect path's in-memory `>&2` / `2>&1` software
/// routing applies — it must fire ONLY when no earlier file/pipe redirect
/// already intercepted the source fd.
#[cfg(test)] // only the `#[cfg(test)]` capture-routing tags consult this
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
#[cfg(test)] // production no longer expands the dup source for capture routing (#223)
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

/// Applies redirects at the real-fd level (saved/restored via `RedirectScope`),
/// forcing a `Terminal` inner sink when a stdout redirect is present so the
/// redirect wins over an outer capture, then runs `run_inner(shell, inner_sink)`
/// and returns its status. Redirects are applied in source order. A
/// redirect-open failure prints `huck: <target>: <err>` and returns
/// `Continue(1)` WITHOUT running `run_inner`. Shared by `run_redirected` for
/// compound commands and by the function / eval / source call branches.
fn with_redirect_scope<F>(redirs: &[Redirection], shell: &mut Shell, run_inner: F) -> ExecOutcome
where
    F: FnOnce(&mut Shell) -> ExecOutcome,
{
    // Snapshot the procsub stack BEFORE expanding any redirect-target words.
    // Any process substitutions realized while expanding redirect words (e.g.
    // `cmd < <(inner)`) are recorded in [procsub_base..]. We drain that slice
    // on every exit path. The run_exec_single layer inside run_inner handles
    // its own argument procsubs (those go in [run_exec_base..] which is a
    // subset captured later), so the two layers cover disjoint ranges.
    let procsub_base = shell.procsub_pending.len();

    // Flush buffered terminal/builtin output BEFORE swapping fds, for two
    // reasons. (1) Prior output must not be diverted into the redirect target.
    // (2) v308: the builtin's own stdout now goes straight to real fd 1 via
    // `FdWriter` (unbuffered), so anything still sitting in `io::stdout()`'s
    // buffer would be overtaken by those raw writes and surface out of order.
    // Emptying the buffer here is what keeps the two writers in step.
    let _ = io::stdout().flush();

    let mut scope = RedirectScope::new();

    // Apply every redirection IN SOURCE ORDER, so `2>&1 >file` differs from
    // `>file 2>&1`. A failure mid-list returns early; the scope's Drop rolls
    // back the entries already applied (atomic, matching pre-v156 behavior).
    if let Err(outcome) = scope.apply_redirects(redirs, shell) {
        drop(scope);
        drain_procsubs(shell, procsub_base);
        return outcome;
    }

    // Run the inner compound with the now-redirected fds. Its (possibly
    // buffered) terminal output is flushed by the scope's Drop before restore.
    // Stage 3 (#197): the sinks are always `Terminal`, so the inner body's
    // stdout/stderr flow to the real (redirected) fd 1/2 — the redirect wins
    // outright. (The deleted `Capture` sink needed an in-memory `force Terminal`
    // + `Merged` reroute here to keep a `$(...)` capture honoring `>file` /
    // `2>&1`; with capture done at the real-fd level that is unnecessary.)
    let outcome = run_inner(shell);
    let _ = io::stdout().flush();
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
/// The builtin's stdout goes to real fd 1 via `FdWriter` (unbuffered; = the
/// redirect target); its stderr to real fd 2 via `CaptureStderr`. The redirects
/// are applied at the real-fd level, so a `>file` / `2>&1` / `>&2` is realized by
/// the actual dup chain.
///
/// **Test capture (`#[cfg(test)]`):** when a `capture_test_hook` capture is
/// active, `FdWriter`/`CaptureStderr` intercept the bytes in memory instead of
/// hitting the fd. A trailing `>&2` / `2>&1` (with no later override of the
/// target fd) has no real fd to observe under an in-memory capture, so
/// `out_cap`/`err_cap` tag each writer to the correct capture stream — the
/// stdout of a `>&2` feeds the captured stderr buffer, and vice versa. In
/// production no capture is active and the tags are inert.
///
/// `read`'s stdin (`<`, `<<`, `<<<`) lands on fd 0 via the scope, so the builtin
/// reads from the redirected descriptor. Heredoc/here-string writer pids spawned
/// during apply are reaped after the call.
fn run_builtin_with_redirects(
    resolved: &ResolvedCommand,
    redirs: &[Redirection],
    shell: &mut Shell,
) -> ExecOutcome {
    let procsub_base = shell.procsub_pending.len();
    // Flush buffered terminal/builtin output BEFORE applying the real-fd
    // redirect scope below, for two reasons. (1) Prior output must not be
    // diverted into the redirect target. (2) The builtin's own stdout goes
    // straight to real fd 1 via `FdWriter` (unbuffered) further down in this
    // function, so anything still sitting in `io::stdout()`'s buffer would be
    // overtaken by those raw writes and surface out of order. Emptying the
    // buffer here is what keeps the two writers in step.
    let _ = io::stdout().flush();

    // Compute each of fd 1 / fd 2's FINAL effective destination after the whole
    // redirect list (source order), so we know where the builtin's stdout and
    // stderr should land:
    //   - `Sink`        → no redirect on that fd; the builtin's stream flows to
    //                     its real fd (fd 1 / fd 2), and a `#[cfg(test)]` capture
    //                     intercepts it there (`CaptureStream::Out` / `Err`).
    //   - `Follows(2)` on fd 1 with fd 2 == `Sink`  → trailing `>&2`: stdout is
    //                     logically the stderr stream. Under capture it must feed
    //                     the captured *stderr* buffer (the real dup2 is a no-op
    //                     for the intercepted writer); tag fd 1's writer `Err`.
    //   - `Follows(1)` on fd 2 with fd 1 == `Sink`  → trailing `2>&1`: symmetric,
    //                     tag fd 2's writer `Out`.
    //   - anything else (a file / pipe / close on that fd) → the real-fd dup
    //                     chain owns it; DON'T intercept — real write wins so a
    //                     `>file` is honored over an outer capture (`None`).
    // In production no capture is active, so every stream does a real fd write
    // and the tags are inert — the real dup2 chain alone realizes the redirect.
    // `final_dests_for_1_2` resolves each `>&word` source (running any `$(...)`
    // in it), so calling it in production would expand the dup source a SECOND
    // time (`apply_redirects` below is the first) — `echo x >&"$(cmd)"` would run
    // `cmd` twice. Its result feeds only the capture tags, which are inert unless
    // a `#[cfg(test)]` capture is active, so gate the whole computation on `test`.
    #[cfg(test)]
    let (out_cap, err_cap) = {
        use crate::fd_writer::CaptureStream;
        let (final_1, final_2) = final_dests_for_1_2(redirs, shell);
        let out_cap = match (&final_1, &final_2) {
            (RedirectDest::Sink, _) => Some(CaptureStream::Out),
            (RedirectDest::Follows(2), RedirectDest::Sink) => Some(CaptureStream::Err),
            _ => None,
        };
        let err_cap = match (&final_2, &final_1) {
            (RedirectDest::Sink, _) => Some(CaptureStream::Err),
            (RedirectDest::Follows(1), RedirectDest::Sink) => Some(CaptureStream::Out),
            _ => None,
        };
        (out_cap, err_cap)
    };
    #[cfg(not(test))]
    let (out_cap, err_cap): (
        Option<crate::fd_writer::CaptureStream>,
        Option<crate::fd_writer::CaptureStream>,
    ) = (None, None);

    let mut scope = RedirectScope::new();
    if let Err(outcome) = scope.apply_redirects(redirs, shell) {
        drop(scope);
        drain_procsubs(shell, procsub_base);
        return outcome;
    }

    // #186/#190/#191: builtin stdout bound for a real fd goes through
    // `FdWriter`, NOT the process-global `io::stdout()`. `io::stdout()` swallows
    // EBADF (`std::io::stdio::handle_ebadf` upstream reports success for a write
    // that genuinely failed), it is a `LineWriter` — so a trailing newline
    // decides whether an error surfaces at `write_all` or at a later `flush` —
    // and it RETAINS failed bytes, which then reach whatever fd 1 is restored
    // to. `FdWriter` returns the true errno, buffers nothing, and records the
    // first error for the epilogue below. This replaces v298's (#137) `fcntl`
    // closed-fd probe and throwaway-buffer workaround: a raw write(2) reports
    // EBADF for a closed fd on its own. The `out_cap`/`err_cap` tags only
    // matter when a `#[cfg(test)]` capture is active (see above).
    let mut fd1_writer = crate::fd_writer::FdWriter::with_capture(libc::STDOUT_FILENO, out_cap);
    let mut err_w = crate::fd_writer::CaptureStderr::with_capture(err_cap);
    let run = |out: &mut dyn std::io::Write, err: &mut dyn std::io::Write, shell: &mut Shell| {
        if let Some(da) = resolved.decl_args.as_deref() {
            builtins::run_declaration_builtin(&resolved.program, da, out, err, shell)
        } else {
            builtins::run_builtin(&resolved.program, &resolved.args, out, err, shell)
        }
    };
    let outcome = run(&mut fd1_writer, &mut err_w, shell);
    // Keep flushing `io::stdout()` here even though builtin stdout no longer
    // goes through it: a diagnostic written through `io::stdout()` elsewhere
    // must land before `drop(scope)` restores fd 1 — otherwise it would be
    // flushed to the restored fd, i.e. the wrong destination (#191's failure
    // mode, on the stderr side).
    let _ = io::stdout().flush();
    let _ = std::io::Write::flush(&mut std::io::stderr());
    // The SINGLE reporter for every builtin write failure. `FdWriter` recorded
    // the first errno, and that recording is what makes this work: the great
    // majority of builtin write sites in builtins.rs discard their own `Result`
    // (`let _ = writeln!(out, …)`), so a per-builtin check could never cover
    // `declare -p x >&3` and friends — nor would it survive the next builtin
    // someone adds. A discarded `Result` no longer means a discarded error.
    //
    // The handful of sites that DO check keep their early return (stop writing
    // once the fd is broken) but emit nothing: this is the only place a write
    // error is worded, which is what keeps #190 fixed. Adding an emit at one of
    // those sites would double-report — `cd` and `export -f` both did, and both
    // were caught only by running them against bash.
    //
    // (No exact site count here on purpose: it depends on what you call a site
    // — the count in this comment has already been wrong twice, and the
    // argument does not need a number.)
    let outcome = match fd1_writer.first_error() {
        Some(e) => {
            {
                let mut ew = err_writer();
                crate::sh_error_to!(
                    shell,
                    &mut *ew,
                    None,
                    "{}: write error: {}",
                    resolved.program,
                    crate::bash_io_error(&e)
                );
            }
            ExecOutcome::Continue(1)
        }
        None => outcome,
    };
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
/// The source line a command begins on (1-based, LOCAL to the parse; 0/None =
/// unknown), recursing into compound bodies to the first sub-command. Used to
/// stamp `$LINENO` for the `line N:` prologue on a COMPOUND command's redirect
/// error (#170) — a simple command already sets `current_lineno` in `run_single`.
fn command_line(cmd: &Command) -> Option<u32> {
    use crate::command::{Command as C, SimpleCommand as S};
    match cmd {
        C::Simple(S::Exec(e)) => Some(e.line),
        C::Simple(S::Assign(_, l)) => Some(*l),
        C::For(c) => Some(c.line),
        C::Case(c) => Some(c.line),
        C::Select(c) => Some(c.line),
        C::ArithFor(c) => Some(c.line),
        C::FunctionDef { line, .. } => Some(*line),
        C::BraceGroup(seq) => command_line(&seq.first),
        C::Subshell { body } => command_line(&body.first),
        C::Pipeline(p) => p.commands.first().and_then(command_line),
        C::If(c) => command_line(&c.condition.first),
        C::While(c) => command_line(&c.condition.first),
        C::Redirected { inner, .. } => command_line(inner),
        _ => None,
    }
}

fn run_redirected(
    inner: &Command,
    redirects: &[crate::command::Redirection],
    shell: &mut Shell,
) -> ExecOutcome {
    // #170: stamp `current_lineno` from the compound command's first sub-command
    // BEFORE applying the trailing redirects, so a redirect-open error carries
    // the `line N:` prologue (a simple command's redirect already does — that
    // path goes through `run_single`, not here). A safe no-op when the line is
    // unknown (0/None) — the prologue then omits `line N:`, unchanged from before.
    if let Some(l) = command_line(inner).filter(|&l| l != 0) {
        shell.current_lineno = shell.line_base() + l;
    }
    with_redirect_scope(redirects, shell, |shell| run_command(inner, shell))
}

/// Runs a `while`/`until` loop. The body runs while the condition's
/// exit status satisfies the loop's polarity. `break` ends the loop;
/// `continue` jumps to the next condition test; `exit` propagates; a
/// pending SIGINT (Ctrl-C) ends the loop with status 130.
fn run_while(clause: &WhileClause, shell: &mut Shell) -> ExecOutcome {
    with_loop_depth(shell, |sh| run_while_inner(clause, sh))
}

/// Run `f` one loop level deeper, restoring the depth on the way out.
///
/// `loop_depth` is what `break`/`continue` count against, so every construct
/// they can escape must bump it exactly once around its body. Four constructs
/// (`while`/`until`, `for`, arithmetic `for`, `select`) spelled that
/// increment/call/decrement triple out by hand and identically; this is the one
/// owner. A closure rather than an RAII guard because the guard would have to
/// hold the `&mut Shell` it needs to hand to the body (cf. `cwd_scope::with_cwd`,
/// which takes the same shape for the same reason).
fn with_loop_depth<R>(shell: &mut Shell, f: impl FnOnce(&mut Shell) -> R) -> R {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = f(shell);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_while_inner(clause: &WhileClause, shell: &mut Shell) -> ExecOutcome {
    let mut last = ExecOutcome::Continue(0);
    loop {
        if let Some(o) = check_interrupt(shell) {
            return o;
        }
        shell.suppress_both();
        let cond = execute_sequence_body(&clause.condition, shell);
        shell.unsuppress_both();
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
        match execute_sequence_body(&clause.body, shell) {
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
fn run_for(clause: &ForClause, shell: &mut Shell) -> ExecOutcome {
    with_loop_depth(shell, |sh| run_for_inner(clause, sh))
}

fn run_for_inner(clause: &ForClause, shell: &mut Shell) -> ExecOutcome {
    // bash accepts any word as the loop variable at parse time but requires a
    // valid identifier at runtime; a bad name is a NON-FATAL error (status 1,
    // body not run, the surrounding list continues). Reserved words like `if`
    // are valid identifiers and fall through to run normally.
    if !crate::builtins::is_valid_name(&clause.var) {
        // Stamp the for-header line so the runtime error carries bash's
        // `line N:` prefix (compound commands don't stamp current_lineno).
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        {
            let mut err = err_writer();
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
            match glob_expand_word(word, shell, &mut *err_writer()) {
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
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        match match debug_trap_gate(shell) {
            Ok(d) => d,
            Err(o) => return o,
        } {
            crate::traps::DebugDecision::Proceed => {}
            // bash's execute_cmd.c does a C `continue` here (inside `for
            // (retval = ...; list; list = list->next)`), not a break: only
            // this iteration's assignment+body are skipped, and the loop
            // proceeds to the next value (verified against bash 5.2.21
            // source + empirically — see extdebug_skip_diff_check.sh).
            crate::traps::DebugDecision::SkipCommand => continue,
            crate::traps::DebugDecision::ReturnFromSub(n) => {
                return ExecOutcome::FunctionReturn(n);
            }
        }
        if let Some(o) = check_interrupt(shell) {
            return o;
        }
        // #205: precheck readonly on a plain (non-nameref) loop var and emit
        // the error ONCE via the redirect-aware writer — calling `try_set` on a
        // readonly var would ALSO emit it (via assign()'s thread-local sink),
        // producing a duplicate diagnostic. A nameref falls through to
        // `try_set`, which checks the RESOLVED target's readonly and reports.
        if !shell.is_nameref(&clause.var) && shell.is_readonly(&clause.var) {
            {
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "{}: readonly variable", clause.var);
            }
            shell.report_error(crate::error_fatality::ErrorKind::ReadonlyForVar);
            return ExecOutcome::Continue(1);
        }
        if shell.try_set(&clause.var, value).is_err() {
            // Nameref to a readonly target: try_set already reported the error.
            shell.report_error(crate::error_fatality::ErrorKind::ReadonlyForVar);
            return ExecOutcome::Continue(1);
        }
        match execute_sequence_body(&clause.body, shell) {
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
fn run_arith(body: &crate::lexer::Word, line: u32, shell: &mut Shell) -> ExecOutcome {
    // #352: stamp `$LINENO` to the `((` source line so a runtime error's
    // `line N:` prologue matches bash (0 = unknown, keep the prior value).
    if line != 0 {
        shell.current_lineno = shell.line_base() + line;
    }
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
            if crate::arith::should_wrap_expansion_error(&e) {
                let mut err = err_writer();
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    Some("(("),
                    "{}",
                    crate::arith::render_error_body(&src, &e)
                );
            }
            ExecOutcome::Continue(1)
        }
    }
}

/// Runs a C-style `for ((init; cond; step)) do BODY done` arith
/// for-loop. Evaluates `init` once, then loops while `cond` is non-zero
/// (None = always true). Evaluates `step` after each iteration body.
/// Mirrors `run_for`'s break/continue/return/exit/SIGINT handling.
fn run_arith_for(clause: &crate::command::ArithForClause, shell: &mut Shell) -> ExecOutcome {
    with_loop_depth(shell, |sh| run_arith_for_inner(clause, sh))
}

fn run_arith_for_inner(clause: &crate::command::ArithForClause, shell: &mut Shell) -> ExecOutcome {
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
    // bash fires DEBUG before the init section unconditionally — an empty
    // `for ((;;))` init still fires (verified vs bash 5.2.21). Decision
    // ignored (#262).
    if clause.line != 0 {
        shell.current_lineno = shell.line_base() + clause.line;
    }
    let mut run_init = true;
    match match debug_trap_gate(shell) {
        Ok(d) => d,
        Err(o) => return o,
    } {
        crate::traps::DebugDecision::Proceed => {}
        crate::traps::DebugDecision::SkipCommand => run_init = false,
        crate::traps::DebugDecision::ReturnFromSub(n) => {
            return ExecOutcome::FunctionReturn(n);
        }
    }
    if run_init
        && let Some(init) = &clause.init
        && let Err(e) = crate::expand::eval_arith_word(init, shell)
    {
        {
            let mut err = err_writer();
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
        // bash fires DEBUG before each cond evaluation, empty cond included.
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        match match debug_trap_gate(shell) {
            Ok(d) => d,
            Err(o) => return o,
        } {
            crate::traps::DebugDecision::Proceed => {}
            crate::traps::DebugDecision::SkipCommand => break,
            crate::traps::DebugDecision::ReturnFromSub(n) => {
                return ExecOutcome::FunctionReturn(n);
            }
        }
        // 2. Eval cond. Empty cond = always true (matches bash).
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::expand::eval_arith_word(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    {
                        let mut err = err_writer();
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
        match execute_sequence_body(&clause.body, shell) {
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
        // bash fires DEBUG before each step evaluation, empty step included.
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        let mut run_step = true;
        match match debug_trap_gate(shell) {
            Ok(d) => d,
            Err(o) => return o,
        } {
            crate::traps::DebugDecision::Proceed => {}
            crate::traps::DebugDecision::SkipCommand => run_step = false,
            crate::traps::DebugDecision::ReturnFromSub(n) => {
                return ExecOutcome::FunctionReturn(n);
            }
        }
        if run_step
            && let Some(step) = &clause.step
            && let Err(e) = crate::expand::eval_arith_word(step, shell)
        {
            {
                let mut err = err_writer();
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
fn run_select(clause: &crate::command::SelectClause, shell: &mut Shell) -> ExecOutcome {
    with_loop_depth(shell, |sh| run_select_inner(clause, sh))
}

fn run_select_inner(clause: &crate::command::SelectClause, shell: &mut Shell) -> ExecOutcome {
    // 1. Build the item list: expand `in WORDS` (Some), or "$@" (None).
    let items: Vec<String> = match &clause.words {
        Some(words) => {
            let mut v = Vec::new();
            for w in words {
                match glob_expand_word(w, shell, &mut *err_writer()) {
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
        // v324 (#257): DEBUG fires once per select header (menu display +
        // prompt), before each body iteration — matches bash's
        // execute_select_command, which re-runs the DEBUG trap at the top of
        // each iteration but NOT on an empty-REPLY re-prompt (that's an inner
        // retry of the same iteration).
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        match match debug_trap_gate(shell) {
            Ok(d) => d,
            Err(o) => return o,
        } {
            crate::traps::DebugDecision::Proceed => {}
            // A DEBUG-skipped `select` returns 0 (bash: `return
            // (EXECUTION_SUCCESS)`), not the loop's EOF-default status 1.
            crate::traps::DebugDecision::SkipCommand => return ExecOutcome::Continue(0),
            crate::traps::DebugDecision::ReturnFromSub(n) => {
                return ExecOutcome::FunctionReturn(n);
            }
        }

        // 3a. PS3 (default "#? ").
        let ps3 = shell.lookup_var("PS3").unwrap_or_else(|| "#? ".to_string());

        // 3b. select_query: (re)print menu when show_menu, prompt, read one
        //     line. An empty line reprints the menu; EOF terminates the loop.
        let selection: String = loop {
            {
                let mut err = err_writer();
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
                let mut w = crate::fd_writer::FdWriter::with_capture(
                    libc::STDOUT_FILENO,
                    Some(crate::fd_writer::CaptureStream::Out),
                );
                let _ = writeln!(w);
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
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "{}: readonly variable", clause.var);
            }
            return ExecOutcome::Continue(1);
        }

        // 3d. SIGINT check (mirror run_for).
        if let Some(o) = check_interrupt(shell) {
            return o;
        }

        // 3e. Run the body; bubble flow with the v79 decrement-and-bubble pattern.
        match execute_sequence_body(&clause.body, shell) {
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
        // v351 (#350): a `$(( ))` arith error while expanding a pattern (e.g.
        // `$((xx++))` on a readonly var) sets `pending_discard`. bash aborts the
        // whole `case` at that point — stop testing further patterns; the caller's
        // post-`case_item_matches` discard check unwinds the command (status 1).
        if shell.discard_pending() || shell.fatal_pending() {
            return false;
        }
        let hit = if (extglob && crate::glob_match::has_extglob(&pattern))
            || crate::glob_match::has_posix_class(&pattern)
            || crate::glob_match::has_collating_symbol(&pattern)
            || crate::glob_match::has_equivalence_class(&pattern)
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
fn run_case(clause: &CaseClause, shell: &mut Shell) -> ExecOutcome {
    // v318 (#218): the subject can be a process substitution (`case <(cmd)
    // in …`). `expand_assignment` realizes it and pushes onto
    // `procsub_pending`, but `case` — unlike a plain command — has no
    // enclosing per-command drain of its own. Snapshot here and drain on
    // every exit path (bash realizes AND closes the fd / reaps the child for
    // the `case` command). The inner body owns all the early returns.
    let procsub_base = shell.procsub_pending.len();
    let outcome = run_case_inner(clause, shell);
    drain_procsubs(shell, procsub_base);
    outcome
}

fn run_case_inner(clause: &CaseClause, shell: &mut Shell) -> ExecOutcome {
    // #352: stamp `$LINENO` to the `case` source line BEFORE expanding the
    // subject, so a `$(( ))` arith error in the subject reports the `case`
    // line (bash), not the previous statement's stale value.
    if clause.line != 0 {
        shell.current_lineno = shell.line_base() + clause.line;
    }
    let subject = expand_assignment(&clause.subject, shell);
    xtrace_compound(
        shell,
        &format!(
            "case {} in",
            crate::expand::reconstruct_word_source(&clause.subject)
        ),
    );
    match match debug_trap_gate(shell) {
        Ok(d) => d,
        Err(o) => return o,
    } {
        crate::traps::DebugDecision::Proceed => {}
        // A DEBUG-skipped `case` returns 0 (bash: `return (EXECUTION_SUCCESS)`),
        // not the prior command's status.
        crate::traps::DebugDecision::SkipCommand => return ExecOutcome::Continue(0),
        crate::traps::DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
    }
    let mut last = ExecOutcome::Continue(0);
    let mut i = 0;
    let mut fall_through = false;
    while i < clause.items.len() {
        let item = &clause.items[i];
        let run_this = fall_through || case_item_matches(item, &subject, shell);
        // v312 (#3/#49): a `$(( ))` arith error in the case subject OR a pattern
        // (v351 #350) discards the whole `case` command (unwind), mirroring
        // pending_fatal_status below. Set `$? = 1` here: unlike a plain command's
        // discard (which flows through `run_andor_group`'s Continue conversion),
        // `case` returns `Interrupted(DiscardCommand)` directly — the driver maps
        // it to a continue-status 1 but never writes `shell.last_status`, so the
        // next unit's `$?` would read the stale pre-error value without this.
        if shell.take_discard() {
            shell.set_last_status(1);
            return ExecOutcome::Interrupted(InterruptReason::DiscardCommand);
        }
        if let Some(status) = shell.fatal_status() {
            return ExecOutcome::Continue(status);
        }
        if !run_this {
            i += 1;
            continue;
        }
        match &item.body {
            None => last = ExecOutcome::Continue(0),
            Some(body) => match execute_sequence_body(body, shell) {
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
fn run_if(clause: &IfClause, shell: &mut Shell) -> ExecOutcome {
    shell.suppress_both();
    let cond = execute_sequence_body(&clause.condition, shell);
    shell.unsuppress_both();
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
        return execute_sequence_body(&clause.then_body, shell);
    }
    for elif in &clause.elif_branches {
        shell.suppress_both();
        let elif_cond = execute_sequence_body(&elif.condition, shell);
        shell.unsuppress_both();
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
            return execute_sequence_body(&elif.body, shell);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell);
    }
    ExecOutcome::Continue(0)
}

// ──────────────────────────────────────────────────────────────
// v30: `[[ ]]` extended test evaluator
// ──────────────────────────────────────────────────────────────

fn run_double_bracket(
    expr: &TestExpr,
    inline_assignments: &[crate::command::Assignment],
    line: u32,
    shell: &mut Shell,
) -> ExecOutcome {
    // #352: stamp `$LINENO` to the `[[` source line so a runtime error's
    // `line N:` prologue matches bash (0 = unknown, keep the prior value).
    if line != 0 {
        shell.current_lineno = shell.line_base() + line;
    }
    let snap = match apply_inline_assignments(inline_assignments, shell) {
        Ok(s) => s,
        Err(s) => {
            restore_inline_assignments(s, shell);
            return ExecOutcome::Continue(1);
        }
    };
    // v318 (#218): a `[[ … ]]` operand can be a process substitution
    // (`[[ -e <(cmd) ]]`, `[[ <(a) == … ]]`). Each operand expansion in
    // `eval_test_expr` / `render_test_leaf` realizes it and pushes onto
    // `procsub_pending`, but `[[ ]]` has no per-command drain of its own.
    // Snapshot before evaluation and drain after — one wrap covers every
    // internal operand site (including `render_test_leaf`'s second realize
    // under `set -x`) and runs on the success, false, and error paths alike
    // (bash realizes AND closes/reaps for the `[[ ]]` command).
    let procsub_base = shell.procsub_pending.len();
    let result = match eval_test_expr(expr, shell) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            {
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "[[: {msg}");
            }
            ExecOutcome::Continue(2)
        }
    };
    drain_procsubs(shell, procsub_base);
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

/// A `[[ … ]]` leaf whose operands have been EXPANDED — exactly once (#220).
///
/// The expansion is what runs a `<(cmd)` or `$(cmd)` in an operand, so doing it
/// separately for the `set -x` line and for the evaluation ran the inner
/// command TWICE, with its side effects. Expanding into this and driving both
/// from it is what makes the count one.
///
/// Each operand is expanded in the form ITS OPERATOR needs, which is also the
/// form bash traces: `==`/`!=` take a PATTERN (so `[[ abc == "a*" ]]` traces as
/// `abc == \a\*`) and `=~` a REGEX (`[[ a.b =~ "a.b" ]]` traces as
/// `a.b =~ a\.b`). Rendering from a plain word expansion, as huck used to,
/// dropped that escaping.
enum TestLeaf {
    Unary {
        op: TestUnaryOp,
        s: String,
    },
    Binary {
        op: TestBinaryOp,
        l: String,
        r: String,
    },
    Regex {
        l: String,
        p: String,
    },
}

/// The RHS of a binary op, expanded ONCE in the form that operator needs.
fn expand_test_rhs(op: TestBinaryOp, rhs: &crate::lexer::Word, shell: &mut Shell) -> String {
    match op {
        TestBinaryOp::StringEq | TestBinaryOp::StringNe => expand_pattern(rhs, shell),
        _ => expand_assignment(rhs, shell),
    }
}

/// Expand a LEAF's operands once. `None` for the connectives, which have no
/// operands of their own.
fn expand_test_leaf(expr: &TestExpr, shell: &mut Shell) -> Option<TestLeaf> {
    match expr {
        TestExpr::Unary { op, operand } => Some(TestLeaf::Unary {
            op: *op,
            s: expand_assignment(operand, shell),
        }),
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            let r = expand_test_rhs(*op, rhs, shell);
            Some(TestLeaf::Binary { op: *op, l, r })
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            // A QUOTED span matches literally (regex metachars escaped); an
            // unquoted span stays an active regex (bash 3.2+, L-23).
            let p = crate::expand::expand_regex_operand(pattern, shell);
            Some(TestLeaf::Regex { l, p })
        }
        TestExpr::Not(_) | TestExpr::And(_, _) | TestExpr::Or(_, _) => None,
    }
}

/// Render an already-expanded leaf for `set -x`.
fn render_test_leaf(leaf: &TestLeaf) -> String {
    match leaf {
        TestLeaf::Unary { op, s } => format!("{} {}", test_unary_op_str(*op), xtrace_operand(s)),
        TestLeaf::Binary { op, l, r } => format!(
            "{} {} {}",
            xtrace_operand(l),
            test_binary_op_str(*op),
            xtrace_operand(r)
        ),
        TestLeaf::Regex { l, p } => format!("{} =~ {}", xtrace_operand(l), xtrace_operand(p)),
    }
}

/// Evaluate an already-expanded leaf. Nothing here expands again.
fn eval_test_leaf(leaf: &TestLeaf, shell: &mut Shell) -> Result<bool, String> {
    match leaf {
        TestLeaf::Unary { op, s } => {
            if matches!(op, TestUnaryOp::VarSet) {
                return Ok(shell.element_or_var_is_set(s));
            }
            if matches!(op, TestUnaryOp::OptEnabled) {
                return Ok(crate::builtins::option_get(shell, s).unwrap_or(false));
            }
            Ok(eval_unary(*op, s))
        }
        TestLeaf::Binary { op, l, r } => eval_binary(*op, l, r, shell),
        TestLeaf::Regex { l, p } => {
            let p = if shell.nocasematch() {
                format!("(?i){p}")
            } else {
                p.clone()
            };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            match re.captures(l) {
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
    // #220: expand the leaf's operands ONCE, then drive BOTH the `set -x` line
    // and the evaluation from that. Expanding separately for each ran a
    // `<(cmd)` / `$(cmd)` operand twice — under `set -x` only, so the inner
    // command's side effects doubled exactly when someone was watching.
    if let Some(leaf) = expand_test_leaf(expr, shell) {
        if !suppress && shell.shell_options.xtrace {
            let p4 = ps4(shell);
            xtrace_emit(
                xtrace_target_fd(shell),
                &format!("{p4}[[ {} ]]", render_test_leaf(&leaf)),
            );
        }
        return eval_test_leaf(&leaf, shell);
    }
    match expr {
        TestExpr::Not(inner) => {
            // The negation is part of the traced body (`[[ ! -e x ]]`), so the
            // inner leaf is expanded here and evaluated from that expansion.
            if let Some(leaf) = expand_test_leaf(inner, shell) {
                if !suppress && shell.shell_options.xtrace {
                    let p4 = ps4(shell);
                    xtrace_emit(
                        xtrace_target_fd(shell),
                        &format!("{p4}[[ ! {} ]]", render_test_leaf(&leaf)),
                    );
                }
                return eval_test_leaf(&leaf, shell).map(|b| !b);
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
        // Every leaf returned above.
        TestExpr::Unary { .. } | TestExpr::Binary { .. } | TestExpr::Regex { .. } => {
            unreachable!("leaves are handled by expand_test_leaf")
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

/// `rhs` arrives ALREADY EXPANDED, in the form this operator needs
/// (`expand_test_rhs`) — a pattern for `==`/`!=`, a plain word otherwise. It
/// used to expand the RHS word itself, which is half of why a `$(cmd)` operand
/// ran twice under `set -x` (#220).
fn eval_binary(op: TestBinaryOp, lhs: &str, rhs: &str, shell: &mut Shell) -> Result<bool, String> {
    match op {
        TestBinaryOp::StringEq | TestBinaryOp::StringNe => {
            let pattern_str = rhs.to_string();
            let nocase = shell.nocasematch();
            // G3: the `==`/`!=` RHS inside `[[ … ]]` is ALWAYS an extended
            // pattern in bash — an `@(a|b)`/`!(x)`-shaped group matches as extglob
            // regardless of `shopt extglob` (the parser likewise force-recognizes
            // it). So gate ONLY on the pattern SHAPE here, not the runtime option
            // (unlike `case`/globbing, which honor the shopt).
            let matched = if crate::glob_match::has_extglob(&pattern_str)
                || crate::glob_match::has_posix_class(&pattern_str)
                || crate::glob_match::has_collating_symbol(&pattern_str)
                || crate::glob_match::has_equivalence_class(&pattern_str)
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
        TestBinaryOp::StringLt | TestBinaryOp::StringGt => Ok(match op {
            TestBinaryOp::StringLt => lhs < rhs,
            TestBinaryOp::StringGt => lhs > rhs,
            _ => unreachable!(),
        }),
        TestBinaryOp::IntEq
        | TestBinaryOp::IntNe
        | TestBinaryOp::IntLt
        | TestBinaryOp::IntGt
        | TestBinaryOp::IntLe
        | TestBinaryOp::IntGe => {
            // In `[[ ]]`, the arithmetic comparison ops evaluate BOTH operands
            // as arithmetic expressions: a bare variable name resolves to its
            // value, `2+3` -> 5, and an unset/empty operand -> 0.
            let l: i64 = arith_eval_operand(lhs, shell)?;
            let r: i64 = arith_eval_operand(rhs, shell)?;
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
            let op_str = match op {
                TestBinaryOp::NewerThan => "-nt",
                TestBinaryOp::OlderThan => "-ot",
                TestBinaryOp::SameFile => "-ef",
                _ => unreachable!(),
            };
            Ok(crate::test_builtin::compare_files(op_str, lhs, rhs))
        }
    }
}

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell) -> ExecOutcome {
    // #1: a `!`-negated pipeline's failure is EXPECTED (it is being tested), so
    // `set -e`/ERR must not fire for anything the body runs — including inner
    // executions like `eval` and brace groups that are NOT their own boundary.
    // Raise the shared errexit-suppression counter (the same mechanism a
    // while/if condition uses) around the body only; the outer and-or gate
    // already exempts the negated pipeline itself. A real `exit` still
    // propagates: it returns `ExecOutcome::Exit` directly, never through the
    // errexit gate the counter controls, and the negation below only rewrites
    // `Continue`.
    // #469: `!` exempts the negated command ITSELF from both — that part is
    // handled by `is_negated_pipeline` at the fire site. What it does NOT do is
    // stop a compound BODY firing the ERR trap: bash prints the trap's output
    // for `! { false; }` with `set +e` and stays silent with `set -e`.
    // Reproducing a bash quirk, not choosing a rule — the spec's contract table
    // records the measurement, including that it really is the inner command
    // firing (`! { (exit 5); }` reports the inner status).
    //
    // Read at ENTRY so `set -e` / `set +e` inside the body cannot unbalance the
    // counters, and so the choice matches errexit's state when the exemption
    // began — which is what bash's flag propagation does.
    let negate_suppresses_err_trap = shell.shell_options.errexit;
    if pipeline.negate {
        if negate_suppresses_err_trap {
            shell.suppress_both();
        } else {
            shell.suppress_errexit_only();
        }
    }
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage pipeline: run directly in the parent shell (no fork needed).
        // This covers both Simple commands and compound commands as single stages.
        run_command(&pipeline.commands[0], shell)
    } else {
        run_multi_stage(&pipeline.commands, shell)
    };
    if pipeline.negate {
        // Undo exactly what was raised — see `negate_suppresses_err_trap`.
        if negate_suppresses_err_trap {
            shell.unsuppress_both();
        } else {
            shell.unsuppress_errexit_only();
        }
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

/// True for a compound whose body runs IN THIS PROCESS through the same
/// `finish_command` epilogue — so the body has already had its own chance to
/// fire the ERR trap, and the compound's aggregate status must not fire it a
/// second time (#445).
///
/// bash fires ERR at the innermost failing command: `{ false; }` is ONE fire,
/// not two, and `{ { false; }; }` is still one. Nesting made huck's extra fire
/// compound — three for two braces.
///
/// Deliberately NOT listed:
/// - `Subshell` — its body runs in a FORKED child whose trap table was
///   cleared, so nothing fired inside; the parent's fire is the only one.
/// - a function call (a `Simple`/`Pipeline` here) — same reasoning via the
///   entry-unset in #438: the body does not fire, the call does.
/// - `Pipeline`, `Simple`, `Arith`, `DoubleBracket` — these ARE the innermost
///   failing command.
///
/// errexit is deliberately NOT gated on this: `set -e; { false; }` must still
/// exit, exactly as it does today.
fn body_already_fired_err(cmd: &Command) -> bool {
    // #481: look THROUGH a single-stage pipeline wrapper. An EVEN number of
    // `!` in front of a compound cancels to `negate: false`, which leaves the
    // compound wrapped in a `Pipeline` that `is_negated_pipeline` no longer
    // exempts — so `! ! { false; }` fired twice where `{ false; }` and
    // `! ! ! { false; }` each fire once. The wrapper does not change WHO the
    // innermost failing command is, so unwrap it before deciding. Only a
    // single-command pipeline: a real `{ false; } | cat` is a different shape
    // whose stages run in forked children.
    let cmd = match cmd {
        Command::Pipeline(p) if p.commands.len() == 1 => &p.commands[0],
        other => other,
    };
    matches!(
        cmd,
        Command::BraceGroup(_)
            | Command::For(_)
            | Command::ArithFor(_)
            | Command::Case(_)
            | Command::Select(_)
            | Command::If(_)
            | Command::While(_)
    )
}

// ----- background pipeline --------------------------------------------------

/// The stdin an async (`&`) child should start with. bash defaults async stdin
/// to `/dev/null` when the shell is non-interactive and the unit is not a bare
/// multi-stage pipeline; otherwise the child inherits the shell's stdin.
/// Decide an async child's default stdin as a `ChildFd`. `inherit` is true when
/// the unit must keep the shell's stdin regardless of interactivity (a bare
/// multi-stage pipeline). Interactive always inherits. Otherwise stdin defaults
/// to `/dev/null` (`Owned`); on open failure emit `/dev/null: <error>` + Err.
fn async_default_stdin(inherit: bool, shell: &mut Shell) -> Result<ChildFd, ()> {
    if inherit || shell.is_interactive {
        return Ok(ChildFd::Inherit);
    }
    match File::open("/dev/null") {
        Ok(f) => Ok(ChildFd::from(f)),
        Err(e) => {
            let mut err = err_writer();
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
    inherit_stdin: bool,
    display: &str,
) -> ExecOutcome {
    let display = display.to_string();
    let job_control = shell.job_control_active();
    // bash: an async command's stdin defaults to /dev/null (non-interactive, no
    // explicit input redirect) so it can't steal the terminal; a bare
    // multi-stage pipeline inherits instead (async_default_stdin, #126).
    let stdin = match async_default_stdin(inherit_stdin, shell) {
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
                    let mut err = err_writer();
                    e!(&mut *err, "[{id}] {pid}");
                }
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            {
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "fork: {}", crate::bash_io_error(&e));
            }
            ExecOutcome::Continue(1)
        }
    }
}

fn run_background_sequence(pipeline: &Pipeline, shell: &mut Shell, display: &str) -> ExecOutcome {
    let display = display.to_string();
    let job_control = shell.job_control_active();

    // Spawn every stage via the shared pipeline core in Background mode. This
    // applies the async stdin default (#129) for stage 0, never captures/merges
    // the last stage's stdout/stderr (a `pipeline &` writes to the terminal),
    // sets `first_pid` to the job's leader pid, and on a spawn failure kills+
    // reaps the partial pipeline via bail_teardown_pipeline(Background, …). All
    // that's left here is registering the job and returning immediately (no
    // wait) — that's what makes it "background".
    let sp = match spawn_pipeline(&pipeline.commands, SpawnMode::Background, shell) {
        Ok(sp) => sp,
        Err(outcome) => return outcome,
    };

    // Re-assert the job's process group in the parent (a race-close mirror of the
    // per-stage setpgid the spawners already did). `sp.pgid_target` is the leader
    // pid when job control is active, else NO_PGROUP (no grouping — every stage
    // stays in the shell's group). Best-effort: a stage that already exec'd or
    // exited yields EACCES/ESRCH, which is fine — its group was set at spawn.
    if sp.pgid_target != NO_PGROUP {
        for stage in &sp.stages {
            let PipelineStage::Forked(pid) = stage else {
                continue;
            };
            unsafe {
                libc::setpgid(*pid, sp.pgid_target);
            }
        }
    }

    let Some(pgid) = sp.first_pid else {
        // No actual children spawned (all-Assign pipeline). Treat as
        // synthetic Done. This shouldn't happen in practice — the parser
        // doesn't produce all-Assign backgrounded pipelines as a typical
        // user input shape, but we handle it defensively.
        shell.jobs.add_synthetic_done(display, 0);
        crate::jobs::reap_and_notify(shell);
        // Non-blocking drain: close parent fds so any inner child sees EOF.
        drain_procsubs_nonblocking(shell, sp.procsub_base);
        return ExecOutcome::Continue(0);
    };

    let pids: Vec<i32> = sp
        .stages
        .iter()
        .filter_map(|s| match s {
            PipelineStage::Forked(p) => Some(*p),
            PipelineStage::InProcess { .. } => None,
        })
        .collect();
    let last_pid = *pids.last().unwrap();
    shell.last_bg_pid = Some(last_pid);
    let id = shell.jobs.add_with_pgroup(pgid, pids, display, job_control);
    // bash suppresses automatic job notices inside a subshell environment / completion funcs
    if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
        {
            let mut err = err_writer();
            e!(&mut *err, "[{id}] {last_pid}");
        }
    }
    // Non-blocking drain: close parent fds and attempt WNOHANG reap of inner
    // procsub children. We don't block here because a long-running inner producer
    // (e.g. `cmd < <(long-running-gen) &`) should not make the background job
    // synchronous. Any child not yet exited is left to SIGCHLD/exit cleanup.
    drain_procsubs_nonblocking(shell, sp.procsub_base);
    ExecOutcome::Continue(0)
}

/// Unified pipeline-spawn error teardown. Both modes drain process substitutions
/// started this pipeline and close every parent-held pipe fd, then return
/// failure. Background additionally kills+reaps the already-spawned stages;
/// foreground does NOT — it tracks live pids via live_external_children/`stages`
/// and reaps them through wait_pipeline_raw, so reaping here would double-reap.
fn bail_teardown_pipeline(
    mode: SpawnMode,
    shell: &mut Shell,
    procsub_base: usize,
    first_pid: Option<i32>,
    stages: &[PipelineStage],
    parent_held: &mut Vec<RawFd>,
) -> ExecOutcome {
    drain_procsubs(shell, procsub_base);
    if mode == SpawnMode::Background {
        let pids: Vec<i32> = stages
            .iter()
            .filter_map(|s| match s {
                PipelineStage::Forked(p) => Some(*p),
                PipelineStage::InProcess { .. } => None,
            })
            .collect();
        cleanup_partial_pipeline_raw(first_pid, &pids);
    }
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

/// Render a `Command` back to a normalized bash-style source line for the
/// `jobs`/`fg`/`bg` display: whitespace collapsed to single spaces (already done
/// by the lexer), quotes preserved, and words shown UNEXPANDED — matching how
/// bash re-renders a job's command from its parsed form (it does not store the
/// literal source). This is an AST→source deparse, not source slicing. The
/// trailing `&` is appended by the `jobs` formatter, not here.
///
/// Byte-identical to bash 5.2.21 for the common simple/pipeline/and-or/redirect/
/// quoted forms. Documented best-effort residuals: exotic mixed quoting
/// re-renders canonically with `"…"` (huck retains `quoted: bool`, not the
/// original quote char), `&>`/`&>>` render as their desugared `> f 2>&1` form,
/// and full compound commands (if/for/while/case/…) fall back to a label rather
/// than bash's multi-line re-render.
fn render_job_command(cmd: &Command) -> String {
    match cmd {
        Command::Simple(s) => render_job_simple(s),
        Command::Pipeline(p) => p
            .commands
            .iter()
            .map(render_job_command)
            .collect::<Vec<_>>()
            .join(" | "),
        Command::Subshell { body } => format!("( {} )", render_job_sequence(body)),
        Command::BraceGroup(body) => format!("{{ {}; }}", render_job_sequence(body)),
        Command::Redirected { inner, redirects } => {
            let mut out = render_job_command(inner);
            for r in redirects {
                out.push(' ');
                out.push_str(&render_job_redirection(r));
            }
            out
        }
        // Full compound commands (if/for/while/case/select/[[…]]/((…))/coproc/
        // function) are rare as direct background jobs and bash re-renders them
        // multi-line; best-effort label rather than a byte-exact match.
        _ => render_job_compound_fallback(cmd),
    }
}

/// Render a `Sequence` (an and-or / `;`-joined list) for the job display,
/// joining commands with their real connectors.
fn render_job_sequence(seq: &Sequence) -> String {
    let mut s = render_job_command(&seq.first);
    for (conn, cmd) in &seq.rest {
        s.push_str(match conn {
            Connector::Semi => "; ",
            Connector::And => " && ",
            Connector::Or => " || ",
            Connector::Amp => " & ",
        });
        s.push_str(&render_job_command(cmd));
    }
    s
}

fn render_job_simple(s: &SimpleCommand) -> String {
    match s {
        SimpleCommand::Assign(assigns, _) => assigns
            .iter()
            .map(render_job_assignment)
            .collect::<Vec<_>>()
            .join(" "),
        SimpleCommand::Exec(e) => {
            let mut parts: Vec<String> = Vec::new();
            for a in &e.inline_assignments {
                parts.push(render_job_assignment(a));
            }
            parts.push(crate::expand::reconstruct_word_source(&e.program));
            for arg in &e.args {
                parts.push(crate::expand::reconstruct_word_source(arg));
            }
            let mut out = parts.join(" ");
            for r in &e.redirects {
                out.push(' ');
                out.push_str(&render_job_redirection(r));
            }
            out
        }
    }
}

fn render_job_assignment(a: &crate::command::Assignment) -> String {
    use crate::command::AssignTarget;
    let mut s = String::new();
    match &a.target {
        AssignTarget::Bare(n) => s.push_str(n),
        AssignTarget::Indexed { name, subscript } => {
            s.push_str(name);
            s.push('[');
            s.push_str(&crate::expand::reconstruct_word_source_inner(subscript));
            s.push(']');
        }
    }
    s.push_str(if a.append { "+=" } else { "=" });
    s.push_str(&crate::expand::reconstruct_word_source(&a.value));
    s
}

/// Render one redirection as bash's job display does: file redirects put a space
/// before the target (`> /dev/null`, `2> f`, `0<> f`), dup/move/close redirects
/// glue the source with no space (`2>&1`, `1>&2`), and bash makes the default fd
/// explicit for `<>`/`>&`/`<&` but not for plain `<`/`>`/`>>`/`>|`.
fn render_job_redirection(r: &Redirection) -> String {
    let fd_prefix = |explicit_default: Option<u16>| -> String {
        match &r.fd {
            RedirFd::Number(n) => n.to_string(),
            RedirFd::Var(name) => format!("{{{name}}}"),
            RedirFd::Default => explicit_default.map(|d| d.to_string()).unwrap_or_default(),
        }
    };
    let word = crate::expand::reconstruct_word_source;
    match &r.op {
        RedirOp::File { mode, target } => {
            let (op, show_default) = match mode {
                FileMode::ReadOnly => ("<", false),
                FileMode::Truncate => (">", false),
                FileMode::Append => (">>", false),
                FileMode::Clobber => (">|", false),
                FileMode::ReadWrite => ("<>", true),
            };
            let default = show_default.then(|| r.op.default_fd());
            format!("{}{} {}", fd_prefix(default), op, word(target))
        }
        RedirOp::Dup { source, output } => {
            let op = if *output { ">&" } else { "<&" };
            format!(
                "{}{}{}",
                fd_prefix(Some(r.op.default_fd())),
                op,
                word(source)
            )
        }
        RedirOp::Move { source, output } => {
            let op = if *output { ">&" } else { "<&" };
            format!(
                "{}{}{}-",
                fd_prefix(Some(r.op.default_fd())),
                op,
                word(source)
            )
        }
        RedirOp::Close => {
            // Direction (`>&-` vs `<&-`) isn't retained on `Close`; the explicit
            // target fd survives in `r.fd`. Best-effort `<&-` (exotic as a bg job).
            format!("{}<&-", fd_prefix(Some(r.op.default_fd())))
        }
        RedirOp::HereString(w) => format!("{}<<< {}", fd_prefix(None), word(w)),
        RedirOp::Heredoc { .. } => {
            // A heredoc on a backgrounded command is exotic; the delimiter isn't
            // retained (only the collected body), so render a best-effort marker.
            format!("{}<< (heredoc)", fd_prefix(None))
        }
    }
}

/// Best-effort `jobs`-listing label for a backgrounded compound command
/// (if/for/while/case/…) where a byte-exact multi-line re-render is out of
/// scope. Uses the leading command's static program name when it looks through
/// to a plain simple command, else a generic label.
fn render_job_compound_fallback(cmd: &Command) -> String {
    if let Command::Simple(SimpleCommand::Exec(e)) = cmd
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
        crate::sh_error_to!(
            shell,
            err,
            None,
            "{}: ambiguous redirect",
            crate::expand::reconstruct_word_source(word)
        );
        Err(())
    }
}

/// Expands `source` to a string and parses it as an fd number (e.g. "1" or "2").
/// Used for `RedirectSlot::Dup { source }` to obtain the target fd pre-fork.
/// Errors with "bad fd: ..." if the expansion is not a valid non-negative integer.
#[cfg(test)] // only `final_dests_for_1_2` (test-only) + its unit tests use this
fn resolve_fd_target(source: &crate::lexer::Word, shell: &mut Shell) -> Result<i32, io::Error> {
    let expanded = expand_assignment(source, shell);
    expanded
        .parse::<i32>()
        .map_err(|_| io::Error::other(format!("bad fd: {expanded}")))
}

/// bash's fd-word test (#542): a dup/move redirect word names an fd only when
/// the expanded field is entirely ASCII digits. `7` and `007` are fds; a sign
/// (`+7`, `-5`), surrounding space (`" 7"`) or trailing text (`7abc`) is NOT —
/// bash falls through to the filename / ambiguous-redirect branches, so
/// `parse::<i32>()` (which accepts a sign) silently dup'd where bash opened a
/// file. An all-digits word that overflows is still an fd to bash: it reports
/// `Bad file descriptor` rather than creating a file, so saturate instead of
/// failing the parse.
fn fd_word_number(field: &str) -> Option<RawFd> {
    if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(field.parse::<RawFd>().unwrap_or(RawFd::MAX))
}

/// What a dup/move source word resolved to: an fd, or bash's "close the
/// redirector" outcome for a word that expanded to exactly `-` (#544).
enum DupSource {
    Fd(RawFd),
    /// The word expanded to `-`: close the TARGET fd, exactly like a literal
    /// `>&-` / `<&-` (which the parser routes to `RedirOp::Close`).
    CloseTarget,
}

/// Resolve a `>&w` / `<&w` / move (`>&w-`) source word to an fd, writing bash's
/// error to the redirect-aware writer on failure. Shared by every dup/move
/// redirect-lowering site (`lower_redirects` for the in-process path and the
/// child-plan builders). Does NOT check the fd is currently open — the
/// in-process sites add that via `validate_fd_open` (they perform the `dup2`
/// in the parent); the child-plan sites defer the check to child replay.
/// Returns `Err(())` after emitting the error.
///
/// `bad_fd_label` is what bash names in a `Bad file descriptor` message — the
/// source word when the redirect targets its directional default fd, else the
/// target number (see the call site). Note the asymmetry, which is bash's:
/// `Bad file descriptor` names that label, `ambiguous redirect` names the
/// EXPANSION for a single non-numeric field but the SOURCE WORD when the word
/// produced a field count other than one.
///
/// The `&>file` filename fallback for an output dup on fd 1 is handled by the
/// caller before this point, so a non-numeric field here is always ambiguous.
fn resolve_dup_source(
    source: &crate::lexer::Word,
    shell: &mut Shell,
    bad_fd_label: &str,
) -> Result<DupSource, ()> {
    let fields = expand(source, shell);
    let word_src = crate::expand::reconstruct_word_source(source);
    if fields.len() != 1 {
        let mut err = err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{word_src}: ambiguous redirect");
        return Err(());
    }
    let field = fields.into_iter().next().unwrap().chars;
    // #544: bash tests the EXPANDED word for `-` before the all-digits test and
    // before the `&>file` fallback (redir.c), so `m=-; exec 3>&$m` closes fd 3
    // just as `exec 3>&-` does. huck used to report `-: ambiguous redirect` and
    // leave the fd OPEN, so the follow-on write silently succeeded.
    if field == "-" {
        return Ok(DupSource::CloseTarget);
    }
    if let Some(fd) = fd_word_number(&field) {
        return Ok(DupSource::Fd(fd));
    }
    let mut err = err_writer();
    if field.is_empty() {
        // `<&""`, or `${ARR[0]}` after the array was unset — a bad fd, not a
        // filename and not huck's invented `bad fd:` wording.
        crate::sh_error_to!(
            shell,
            &mut *err,
            None,
            "{bad_fd_label}: Bad file descriptor"
        );
    } else {
        crate::sh_error_to!(shell, &mut *err, None, "{field}: ambiguous redirect");
    }
    Err(())
}

/// Check that `src` is currently an open fd; on failure write bash's
/// `N: Bad file descriptor` and return `Err(())`. Used by the in-process
/// dup/move apply sites before the parent-side `dup2`.
fn validate_fd_open(src: RawFd, shell: &mut Shell, label: &str) -> Result<(), ()> {
    if unsafe { libc::fcntl(src, libc::F_GETFD) } < 0 {
        let mut err = err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{label}: Bad file descriptor");
        return Err(());
    }
    Ok(())
}

/// Validate a dup/move source at LOWER time, honoring earlier same-plan ops.
/// `fd_state` records fds THIS plan has already opened (`true`) or closed
/// (`false`); an fd the plan has not touched falls back to the real fd table.
/// Emits bash's `"{src}: Bad file descriptor"` and returns `Err(1)` when the
/// source is not open at this point in source order. Running this BEFORE later
/// file opens in the same list is what prevents an invalid dup from truncating a
/// file the command never successfully redirects to (bash left-to-right order).
fn validate_plan_source(
    src: RawFd,
    fd_state: &std::collections::HashMap<RawFd, bool>,
    shell: &mut Shell,
    label: &str,
) -> Result<(), i32> {
    let open = match fd_state.get(&src) {
        Some(&state) => state,
        None => (unsafe { libc::fcntl(src, libc::F_GETFD) }) >= 0,
    };
    if !open {
        let mut err = err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{label}: Bad file descriptor");
        return Err(1);
    }
    Ok(())
}

/// Non-emitting open-check mirroring [`validate_plan_source`]/[`validate_fd_open`].
/// Used by the `{var}` dup path to decide whether to emit bash's extra leading
/// `redirection error: cannot duplicate fd: …` line BEFORE the standard
/// `<label>: Bad file descriptor`, without double-emitting.
fn validate_source_is_open(
    src: RawFd,
    fd_state: Option<&std::collections::HashMap<RawFd, bool>>,
) -> bool {
    match fd_state.and_then(|st| st.get(&src)) {
        Some(&open) => open,
        None => (unsafe { libc::fcntl(src, libc::F_GETFD) }) >= 0,
    }
}

/// Validate a dup/move source. In-process (interleaved) passes `None` and we
/// check the REAL fd table (earlier redirects are already applied to it). The
/// child (batch) passes `Some(fd_state)` and we defer to the plan simulation.
/// Emits bash's `"{src}: Bad file descriptor"` and returns `Err(1)` if not open.
fn validate_source(
    src: RawFd,
    fd_state: Option<&std::collections::HashMap<RawFd, bool>>,
    shell: &mut Shell,
    label: &str,
) -> Result<(), i32> {
    match fd_state {
        Some(state) => validate_plan_source(src, state, shell, label),
        None => validate_fd_open(src, shell, label).map_err(|()| 1),
    }
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

/// v349 (#343, Root D): a post-expansion field arriving at a declaration
/// builtin (`declare`/`typeset`/`local`/`readonly`/`export`) of the shape
/// `<valid-ident>=<value>` is an ASSIGNMENT, even when the original word was
/// quoted (bash: `readonly 'x=hi'` == `readonly x=hi` after quote removal).
/// Convert such a field into a `DeclArg::Assign` with a SCALAR (quoted)
/// literal value so it is not re-expanded or globbed; the `-a`/`-A`
/// array-literal coercion of a `(...)`-shaped value happens later inside the
/// builtin (Root B). A field whose left-of-first-`=` is not a valid identifier
/// (or that has no `=`) stays `Plain`.
fn decl_field_to_arg(s: String) -> crate::command::DeclArg {
    if let Some(eq) = s.find('=') {
        let lhs = &s[..eq];
        // `name+=value` is an APPEND assignment (#344); `name=value` a plain one.
        // Only fire when the identifier left of `=`/`+=` is valid; else Plain.
        let (name, append) = match lhs.strip_suffix('+') {
            Some(n) => (n, true),
            None => (lhs, false),
        };
        if builtins::is_valid_name(name) {
            let val = s[eq + 1..].to_string();
            return crate::command::DeclArg::Assign(crate::command::Assignment {
                target: crate::command::AssignTarget::Bare(name.to_string()),
                value: crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
                    text: val,
                    quoted: true,
                }]),
                append,
            });
        }
    }
    crate::command::DeclArg::Plain(s)
}

/// Runs a command with NO program to execute (a syntactically-empty program, or
/// one whose word expanded to zero fields — #62): apply the inline assignments,
/// perform the redirects for side effects (with a no-op body), rc 0 (or the
/// redirect-open failure). bash processes the assignments with the ORIGINAL
/// (un-redirected) fds, so any readonly error / RHS command sub uses the
/// inherited stderr, not the command's own `2>…`. #155: a null command must not
/// persist a `{var}` fd binding — snapshot/restore each named-fd var.
fn run_empty_command_tail(
    cmd: &ExecCommand,
    shell: &mut Shell,
    procsub_base: usize,
) -> ExecOutcome {
    let named_fd_prior: Vec<(String, Option<String>)> = cmd
        .redirects
        .iter()
        .filter_map(|r| match &r.fd {
            crate::command::RedirFd::Var(n) => {
                Some((n.clone(), shell.get(n).map(|s| s.to_string())))
            }
            _ => None,
        })
        .collect();
    let st = run_assignment_list(&cmd.inline_assignments, shell);
    let outcome = with_redirect_scope(&cmd.redirects, shell, move |_shell| {
        ExecOutcome::Continue(st)
    });
    for (name, prior) in named_fd_prior {
        match prior {
            Some(v) => shell.set(&name, v),
            None => shell.unset(&name),
        }
    }
    drain_procsubs(shell, procsub_base);
    outcome
}

/// Emits a `command`/`builtin`-wrapper option/dispatch error (`msg` is the full
/// body), honoring a trailing `2>&1` that merges stderr into a CAPTURED stdout
/// (#77). The wrapper error path runs before `run_builtin_with_redirects`, so it
/// bypasses that function's `2>&1` swap; replicate it here so
/// `x=$(command -Z 2>&1)` captures the error like bash instead of leaking it.
fn emit_wrapper_error(cmd: &ExecCommand, shell: &mut Shell, msg: &str) {
    // The `command`/`builtin` wrapper detects an option error DURING its own
    // argument scan, before the command's redirects reach the real fds — so the
    // error must be emitted UNDER the command's own redirects, exactly as the
    // command itself would (`command -Z 2>file` → file; `$(command -Z 2>&1)` →
    // captured). Run the emission inside `with_redirect_scope` so the redirects
    // apply uniformly: a real `2>&1`/`2>file` dup2 at a terminal/pipe (incl. the
    // forked comsub child, #197), and the software `Merged` route under a capture
    // sink. Replaces the earlier capture-sink-only `Merged` special case (#77).
    with_redirect_scope(&cmd.redirects, shell, |shell| {
        let mut err = err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        ExecOutcome::Continue(2)
    });
}

/// Expands the program + argument words into a `ResolvedCommand`. Returns
/// `Ok(None)` when the program word expands to ZERO fields and no argument field
/// survives either — the caller then runs it as an empty command (#62).
fn resolve(
    cmd: &ExecCommand,
    shell: &mut Shell,
    err: &mut dyn std::io::Write,
) -> Result<Option<ResolvedCommand>, i32> {
    let prog_fields = match glob_expand_word(&cmd.program, shell, err) {
        Ok(v) => v,
        Err(()) => return Err(1),
    };
    if let Some(status) = shell.fatal_status() {
        return Err(status);
    }
    // v312 (#3/#49): a `$(( ))` arith error while expanding the program word
    // discards the command — skip resolution so it never runs (converted to
    // Interrupted(DiscardCommand) at the and-or conversion points).
    if shell.discard_pending() {
        return Err(1);
    }
    // #62: the program word split to ZERO fields (an empty unquoted expansion,
    // e.g. `$Z echo hi` with `Z` unset). bash's word-splitting drops the empty
    // leading word and PROMOTES the first surviving field of the remaining words
    // as the command; if nothing survives it is an empty command (→ `Ok(None)`).
    if prog_fields.is_empty() {
        let mut fields: Vec<String> = Vec::new();
        for word in &cmd.args {
            let f = match glob_expand_word(word, shell, err) {
                Ok(v) => v,
                Err(()) => return Err(1),
            };
            if let Some(status) = shell.fatal_status() {
                return Err(status);
            }
            if shell.discard_pending() {
                return Err(1);
            }
            fields.extend(f);
        }
        if fields.is_empty() {
            return Ok(None);
        }
        let program = fields.remove(0);
        if builtins::is_declaration_command(&program) {
            let decl_args = fields.into_iter().map(decl_field_to_arg).collect();
            return Ok(Some(ResolvedCommand {
                program,
                args: Vec::new(),
                decl_args: Some(decl_args),
            }));
        }
        return Ok(Some(ResolvedCommand {
            program,
            args: fields,
            decl_args: None,
        }));
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
            v.push(decl_field_to_arg(s));
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
        if let Some(status) = shell.fatal_status() {
            return Err(status);
        }
        // v312 (#3/#49): a `$(( ))` arith error while expanding an argument word
        // discards the command — skip so it never runs.
        if shell.discard_pending() {
            return Err(1);
        }
        if let Some(da) = decl_args.as_mut() {
            for s in fields {
                da.push(decl_field_to_arg(s));
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
    Ok(Some(ResolvedCommand {
        program,
        args,
        decl_args,
    }))
}

// ----- redirect file handling -----------------------------------------------

/// bash's `HEREDOC_PIPESIZE` (redir.c). A body at or below this goes to a pipe,
/// a larger one to a temp file. 65536 is the Linux default pipe capacity, which
/// is precisely why bash can write the pipe case from the parent without ever
/// blocking. Verified against bash 5.2.21: a 65536-byte body yields a pipe, a
/// 65537-byte body yields `/tmp/sh-thd.XXXXXX (deleted)`.
const HEREDOC_PIPESIZE: usize = 65536;

/// Deliver an expanded heredoc/here-string body and return a fresh READ-ONLY fd
/// positioned at offset 0, with the body ALREADY fully delivered — no forked
/// writer, matching bash's `here_document_to_fd`.
///
/// This is what makes #169 unreachable: a permanent (`exec`) redirect has no
/// reader until a LATER command runs, so any writer process still blocked on a
/// full pipe could never be reaped. With no writer, there is nothing to wait on.
///
/// `bytes.len()` is bash's `herelen` — here-string callers append the trailing
/// newline BEFORE calling, so it is included in the size decision.
///
/// `tmpdir` is the shell's `$TMPDIR` variable (NOT the process env: bash honors
/// an in-shell `TMPDIR=/x` whether exported or not, and huck does not sync
/// exports to the process env). An unusable value silently falls back to `/tmp`,
/// as bash does.
///
/// The caller owns the returned fd (and typically hands it to
/// `relocate_high_cloexec`). The contract is only "a fresh readable fd at offset
/// 0" — true of a pipe read end and a rewound file alike — so no call site needs
/// to know which path produced it.
fn heredoc_body_to_fd(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error> {
    // Size check FIRST so a large body does no wasted pipe work (bash's exact
    // behavior on Linux). The nonblocking probe inside `heredoc_body_to_pipe` is
    // the portability guard: bash hardcodes 65536 and writes BLOCKING, which is
    // safe only where a pipe holds 64KB. On macOS pipes start at 16KB, so that
    // same code has nothing to stop it wedging — we degrade to a temp file
    // instead of inheriting the hang (cf. #97, already a macOS-only hang).
    if bytes.len() <= HEREDOC_PIPESIZE
        && let Some(fd) = heredoc_body_to_pipe(bytes)
    {
        return Ok(fd);
    }
    heredoc_body_to_file(bytes, tmpdir)
}

/// Try to deliver `bytes` entirely into a pipe buffer, returning the read end.
/// `None` means "did not fit / could not" — the caller falls back to a temp file.
/// Never blocks: the write end is O_NONBLOCK, which is a property of THIS open
/// file description, so the reader's end (a distinct description) stays blocking
/// and the probe is invisible downstream.
fn heredoc_body_to_pipe(bytes: &[u8]) -> Option<RawFd> {
    use std::os::fd::{AsRawFd, IntoRawFd};
    let (r, w) = crate::child_fd::make_pipe_owned(false).ok()?;
    // `r`/`w` are OwnedFds: every early `return None` drops both (closing them),
    // and the success path drops `w` then transfers `r` out via `into_raw_fd`
    // (#197 Class-A — no manual close on any path).
    unsafe {
        let fl = libc::fcntl(w.as_raw_fd(), libc::F_GETFL);
        if fl < 0 || libc::fcntl(w.as_raw_fd(), libc::F_SETFL, fl | libc::O_NONBLOCK) < 0 {
            return None;
        }
    }
    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: writing from a live slice into an open fd.
        let n = unsafe {
            libc::write(
                w.as_raw_fd(),
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // EAGAIN: this platform's pipe is smaller than the body. Anything
            // else: let the temp-file path have a go. Either way, discard.
            return None;
        }
        if n == 0 {
            return None;
        }
        off += n as usize;
    }
    // Drop the write end so the reader sees EOF after the body, then hand the
    // read end to the caller. An empty body lands here directly — a pipe that is
    // immediately at EOF.
    drop(w);
    Some(r.into_raw_fd())
}

/// Spool `bytes` to an unlinked temp file and return a read-only fd at offset 0.
/// Tries `$TMPDIR` then `/tmp`, mirroring bash's silent fallback for an unset or
/// unusable `TMPDIR`.
fn heredoc_body_to_file(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error> {
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(d) = tmpdir
        && !d.is_empty()
    {
        candidates.push(d);
    }
    if !candidates.contains(&"/tmp") {
        candidates.push("/tmp");
    }
    let mut last_err: Option<io::Error> = None;
    for dir in candidates {
        match heredoc_body_to_file_in(bytes, dir) {
            Ok(fd) => return Ok(fd),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::from_raw_os_error(libc::ENOENT)))
}

/// One `mkstemp`-in-`dir` attempt. Follows bash's deliberate, race-conscious
/// order: open the read-only fd BEFORE closing the writable one, and only then
/// unlink — so nothing can substitute the name in between. `mkstemp` creates the
/// file 0600 (owner-only) and the unlink makes it unreachable by name at once,
/// so it also cannot survive a crash.
fn heredoc_body_to_file_in(bytes: &[u8], dir: &str) -> Result<RawFd, io::Error> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut tmpl: Vec<u8> = Vec::with_capacity(dir.len() + 16);
    tmpl.extend_from_slice(dir.as_bytes());
    if !tmpl.ends_with(b"/") {
        tmpl.push(b'/');
    }
    tmpl.extend_from_slice(b"sh-thd.XXXXXX\0");

    // SAFETY: `tmpl` is a NUL-terminated, writable buffer of the exact shape
    // mkstemp requires; it overwrites the trailing XXXXXX in place.
    let rw = unsafe { libc::mkstemp(tmpl.as_mut_ptr() as *mut libc::c_char) };
    if rw < 0 {
        return Err(io::Error::last_os_error());
    }
    // Own the writable fd so every error path closes it exactly once via drop —
    // the unlink stays explicit (it is not a close) (#197 Class-A).
    let rw = unsafe { std::os::fd::OwnedFd::from_raw_fd(rw) };
    let rw_fd = rw.as_raw_fd();
    let path = tmpl.as_ptr() as *const libc::c_char;

    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: writing from a live slice into an open fd.
        let n = unsafe {
            libc::write(
                rw_fd,
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            unsafe {
                libc::unlink(path);
            }
            return Err(e); // `rw` drops here → closes the writable fd
        }
        if n == 0 {
            unsafe {
                libc::unlink(path);
            }
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        off += n as usize;
    }

    // bash's order: second fd opened before the first is closed, then unlink.
    // The fresh O_RDONLY fd starts at offset 0 — no lseek needed.
    let ro = unsafe { libc::open(path, libc::O_RDONLY) };
    let err = if ro < 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        libc::unlink(path);
    }
    drop(rw); // close the writable fd (was a manual libc::close)
    match err {
        Some(e) => Err(e),
        None => Ok(ro),
    }
}

/// Where a freshly-opened redirect-source fd should land.
enum FdPlacement {
    /// Relocate to >= 10 and set FD_CLOEXEC. Used for redirect *targets* on real
    /// fds so the source stays out of the 0..9 swap range that explicit targets
    /// (`3>&1 2>&3`) operate on.
    Relocated,
    /// Return the raw low File fd as opened (CLOEXEC). Used only by the `{var}`
    /// path, which relocates once itself via `dup_to_high_fd` — relocating here
    /// too double-relocates the named fd (fd 11 vs bash's 10; the #135 regression).
    RawLow,
}

/// THE redirect file-open matrix: open `path` per `mode` (ReadOnly / Truncate
/// honoring `noclobber` / Clobber / Append / ReadWrite-no-truncate). When
/// `placement` is `Relocated`, relocate the fd >= 10 with FD_CLOEXEC
/// (best-effort on EMFILE, via relocate_high_cloexec) so a parent-opened
/// redirect *source* can never land in the 0..9 range that redirect *targets*
/// operate on (#135, #132-class). When `placement` is `RawLow`, return the raw
/// opened fd (Rust std ⇒ O_CLOEXEC, at a low number) WITHOUT relocating — this
/// is for callers that relocate the source themselves via `dup_to_high_fd`,
/// e.g. the `{var}`-fd lowering arms in `lower_redirects`; relocating
/// here too would double-relocate and land the named fd one number too high
/// (#135 regression).
/// Callers report failures via `redir_open_error(path, ..)` as today.
fn open_redirect_file(
    mode: &FileMode,
    path: &str,
    noclobber: bool,
    placement: FdPlacement,
) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    let file: File = match mode {
        FileMode::ReadOnly => File::open(path)?,
        FileMode::Truncate => open_writable(path, noclobber)?,
        FileMode::Clobber => open_writable(path, false)?,
        FileMode::Append => OpenOptions::new().create(true).append(true).open(path)?,
        FileMode::ReadWrite => OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?,
    };
    match placement {
        FdPlacement::RawLow => Ok(OwnedFd::from(file)),
        FdPlacement::Relocated => {
            let raw = relocate_high_cloexec(file.into_raw_fd());
            Ok(unsafe { OwnedFd::from_raw_fd(raw) })
        }
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

// ----- single command -------------------------------------------------------

// $PIPESTATUS leaf-site rule (M-50, v83): ONLY `run_single`, `run_multi_stage`,
// and the foreground subshell arm write `$PIPESTATUS`. Compound runners
// (`run_if`/`run_while`/`run_for`/`run_case`/`run_select`/brace group) are
// deliberately PIPESTATUS-transparent — they never write it; their inner leaf
// commands do. This matches bash: after `if cond; then ...; fi`, `$PIPESTATUS`
// reflects the last inner pipeline (e.g. `cond`), not the `if` itself. Do NOT
// add a set_pipestatus call to a compound runner.
fn run_single(cmd: &SimpleCommand, shell: &mut Shell) -> ExecOutcome {
    // $BASH_COMMAND: the source text of the command about to run — stamped
    // before the DEBUG trap fires (bash sets it at the same point, so a DEBUG
    // action reading $BASH_COMMAND sees the command that triggered it). While
    // ANY pseudo-trap action (DEBUG/ERR/RETURN) is itself executing, its own
    // commands must NOT re-stamp this — bash holds $BASH_COMMAND fixed at the
    // triggering command for the whole trap action (verified against real
    // bash: `trap 'echo $BASH_COMMAND' ERR/RETURN/DEBUG` reads the command
    // that fired the trap, not the action's own text, even across an
    // intervening `true`/function call). `firing_traps` is non-empty exactly
    // while such an action runs. (EXIT and real-signal traps don't push
    // `firing_traps`, so their freeze is a separate follow-up.)
    if shell.firing_traps.is_empty() {
        shell.current_command = render_job_simple(cmd);
    }
    let outcome = match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell),
        SimpleCommand::Assign(items, line) => {
            // Stamp $LINENO before expanding RHS so it reflects this assignment's line.
            if *line != 0 {
                shell.current_lineno = shell.line_base() + *line;
            }
            // v318 (#218): a bare assignment (`f=<(cmd)`, no redirects — the
            // `run_exec_single_inner` empty-program path handles the
            // assignment+redirect shape and already has its own procsub_base/
            // drain) previously had NO procsub_pending scope of its own here,
            // so a RHS process substitution realized by `run_assignment_list`
            // (e.g. `f=<(cmd)`) stayed pending and was only closed as
            // collateral by whatever command's drain ran next — an
            // uncontrolled, too-late close. Bracket it here like every other
            // command dispatch: bash itself closes an assignment-RHS procsub's
            // fd right after the assignment's OWN command, even inside a
            // group/function/subshell/`$()` — never across a later command —
            // so a plain per-command drain is the bash-matching lifetime; no
            // extension needed.
            match match debug_trap_gate(shell) {
                Ok(d) => d,
                Err(o) => return o,
            } {
                crate::traps::DebugDecision::Proceed => {
                    let procsub_base = shell.procsub_pending.len();
                    let st = run_assignment_list(items, shell);
                    drain_procsubs(shell, procsub_base);
                    ExecOutcome::Continue(st)
                }
                crate::traps::DebugDecision::SkipCommand => {
                    ExecOutcome::Continue(shell.last_status())
                }
                crate::traps::DebugDecision::ReturnFromSub(n) => ExecOutcome::FunctionReturn(n),
            }
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
            let mut err = err_writer();
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
    // A function body has its own $LINENO context (absolute source lines of the
    // function definition), independent of any eval/DEBUG-trap line reframe
    // active in the caller. Clear the eval frame for the body so a caller's
    // reframe (e.g. a DEBUG trap firing `func $LINENO`) doesn't shift the
    // function's own $LINENO; restore it below.
    let saved_eval_frame = shell.eval_frame.take();
    // v325 (#266): likewise clear the piped-stdin cumulative base for the body
    // so `line_base()`'s stdin fallback doesn't add the caller's stream
    // position to the function's own (definition-relative) line numbering.
    let saved_stdin_base = std::mem::take(&mut shell.stdin_line_base);
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

    // #434: without functrace the body does not inherit the caller's RETURN
    // trap — bash unsets it for the duration of the call and puts it back on
    // return only if the body left RETURN untrapped. Taken here (before the
    // entry DEBUG fire, as in bash's execute_function) so the body, and any
    // `trap -p RETURN` it runs, sees the trap gone.
    let saved_return_trap = crate::traps::take_return_trap_for_call(shell);
    // #438: same story for ERR, gated on `errtrace` (`set -E`) instead.
    let saved_err_trap = crate::traps::take_err_trap_for_call(shell);
    // #439: and for DEBUG, gated on functrace like RETURN. Taken BEFORE the
    // entry fire below, which is the whole point — without functrace bash has
    // no inherited trap left to fire there.
    let saved_debug_trap = crate::traps::take_debug_trap_for_call(shell);

    // v329 (#274): fire the DEBUG trap ONCE on function ENTRY (after the
    // call-site fire, before the first body command), with $LINENO stamped to
    // the function's DEFINITION line — matching bash's function-tracing
    // entry fire. Fired AFTER the frame push (above) so the action sees the
    // right FUNCNAME and `fire_debug_trap`'s functrace gate sees the
    // just-pushed Function frame. The entry fire's DebugDecision
    // (extdebug SkipCommand/ReturnFromSub) is deferred — see #277 — so this
    // fires the action only (Proceed semantics); `fire_debug_trap` itself is
    // a no-op when DEBUG isn't inherited here (no functrace/extdebug).
    if let Some(&def_line) = shell.function_def_line.get(name)
        && def_line != 0
    {
        shell.current_lineno = def_line;
    }
    if let Err(o) = debug_trap_gate(shell) {
        return o;
    }

    let result = run_command(&body, shell);

    // RETURN trap fires with $? set to the function's status AND the
    // function's positional args still in scope. After the action runs,
    // restore the caller's frame.
    let status_for_trap = match &result {
        // #441: an EXPLICIT `return N` does not install N before the trap runs
        // — bash's run_return_trap() saves last_command_exit_value and the
        // action still sees the status of the last command executed BEFORE the
        // `return` (`f() { trap "echo \$?" RETURN; return 4; }` prints the
        // trap builtin's 0, not 4). N is installed for the CALLER afterwards,
        // by this function's `FunctionReturn(n) => Continue(n)` tail. (bash)
        ExecOutcome::FunctionReturn(_) => shell.last_status(),
        ExecOutcome::Continue(c) => *c,
        // Exit/LoopBreak/LoopContinue propagate up; keep $? as-is.
        _ => shell.last_status(),
    };
    shell.set_last_status(status_for_trap);
    crate::traps::fire_return_trap(shell);
    // #434: the trap that just fired (if any) is the body's own; the caller's
    // saved one comes back only if the body left RETURN untrapped.
    crate::traps::restore_return_trap_after_call(shell, saved_return_trap);
    crate::traps::restore_err_trap_after_call(shell, saved_err_trap);
    // #439: same rule — the caller's DEBUG comes back only if the body left
    // DEBUG untrapped, so a trap the body installed for itself survives the
    // call and replaces the caller's.
    crate::traps::restore_debug_trap_after_call(shell, saved_debug_trap);

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
    shell.eval_frame = saved_eval_frame;
    shell.stdin_line_base = saved_stdin_base;
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
    call_function(name, body, args, shell)
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

/// Resolve the fd that `set -x` trace output goes to. bash's `BASH_XTRACEFD`,
/// when its value parses to a valid non-negative integer, is the xtrace
/// destination fd; unset / empty / non-numeric falls back to fd 2 (stderr).
/// Resolved at emit time (v339 #310) — no separate assign-time capture — which
/// naturally handles set→fd, `unset`→stderr, and invalid→stderr.
fn xtrace_target_fd(shell: &Shell) -> i32 {
    match shell.lookup_var("BASH_XTRACEFD") {
        Some(v) => v
            .trim()
            .parse::<i32>()
            .ok()
            .filter(|&n| n >= 0)
            .unwrap_or(2),
        None => 2,
    }
}

/// Emit one xtrace line (the trailing newline is added) to `fd` (resolved via
/// `xtrace_target_fd`, normally fd 2) in a SINGLE `write(2)`. Pipeline stages
/// run in separate processes (a forked in-process stage and the parent tracing
/// an external stage) and share fd 2; a multi-write `eprintln!` lets a
/// sibling's bytes wedge between the prefix and the body (`+ + echo a` / `cat`).
/// A single write of the whole line keeps each trace line intact (stages may
/// still REORDER, which is best-effort per spec).
fn xtrace_emit(fd: i32, line: &str) {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let bytes = buf.as_bytes();
    // Ignore partial write / EINTR: trace lines are small (< PIPE_BUF = 4096, the
    // POSIX pipe-atomicity threshold) and best-effort; a short write at most
    // truncates one line. Single write keeps a line intact against concurrent
    // fd-2 writers (forked pipeline stages).
    unsafe {
        let _ = libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
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
        xtrace_emit(xtrace_target_fd(shell), &format!("{p4}{body}"));
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
        if let Some(ps) = shell.procsub_pending.pop()
            && let Some((pid, code)) = crate::procsub::cleanup(ps)
        {
            shell.jobs.record_terminal_status(pid, code);
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
            // #197 Class-A: owned but left raw — `parent_fd` lives in the
            // `ProcSub` record, which must stay `Clone` (Shell derives Clone);
            // `OwnedFd` is not `Clone`, so it stays `RawFd` and is closed here.
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
            let r = unsafe { libc::waitpid(ps.pid, &mut status, libc::WNOHANG) };
            if r > 0 {
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else if libc::WIFSIGNALED(status) {
                    128 + libc::WTERMSIG(status)
                } else {
                    0
                };
                shell.jobs.record_terminal_status(ps.pid, code);
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
fn run_assignment_list(items: &[crate::command::Assignment], shell: &mut Shell) -> i32 {
    // Reset so only THESE assignments' RHS command substitutions count.
    shell.set_last_cmd_sub_status(None);
    let mut st = 0;
    for a in items {
        let name = a.target.name();
        // For namerefs, skip the early readonly check and let assign() check the
        // RESOLVED target's readonly — a readonly nameref lets you write through.
        if !shell.is_nameref(name) && shell.is_readonly(name) {
            {
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "{name}: readonly variable");
            }
            // v226/v312 (#31/#3): POSIX non-interactive EXITS, default mode
            // DISCARDS the current command. v358 (#198): both, and the exit
            // code, come from the classifier — the 127 here was hardcoded and
            // wrong for a script or stdin, where bash exits 1.
            shell.report_error(crate::error_fatality::ErrorKind::Expansion);
            st = 1;
            break;
        }
        shell.set_xtrace_assign_rhs(None);
        if apply_one_assignment(a, shell, &mut *err_writer()).is_err() {
            // v226/v312 (#31/#3): POSIX non-interactive EXITS, default mode
            // DISCARDS the current command. v358 (#198): both, and the exit
            // code, come from the classifier — the 127 here was hardcoded and
            // wrong for a script or stdin, where bash exits 1.
            shell.report_error(crate::error_fatality::ErrorKind::Expansion);
            st = 1;
            break;
        }
        // `set -a` / `-o allexport`: a variable assigned here is auto-exported.
        if shell.shell_options.allexport {
            shell.export(name);
        }
        if shell.shell_options.xtrace {
            let op = if a.append { "+=" } else { "=" };
            // #311: an array-literal RHS traces as its LITERAL source
            // (`a=(1 2 3)`, `a+=(4 5)`, `a=($x)`) — UNQUOTED — matching bash and
            // the inline-prefix xtrace path. A scalar RHS uses the recorded value
            // (xtrace_quoted as needed). `take_xtrace_assign_rhs` clears the slot.
            let rendered = if let Some(elems) = array_literal_elements(&a.value) {
                shell.take_xtrace_assign_rhs();
                crate::generate::array_literal_to_source(elems)
            } else {
                // `match` (not `unwrap_or_else`) so the mut borrow from `take_…`
                // fully ends before the `lookup_var` shared borrow.
                let val = match shell.take_xtrace_assign_rhs() {
                    Some(rhs) => rhs,
                    None => shell.lookup_var(name).unwrap_or_default(),
                };
                crate::param_expansion::xtrace_quote(&val)
            };
            let p4 = ps4(shell);
            xtrace_emit(
                xtrace_target_fd(shell),
                &format!("{p4}{name}{op}{rendered}"),
            );
        }
    }
    // bash: a bare assignment's status is the last command substitution in its
    // RHS (or 0 if none). A readonly/apply error keeps st=1.
    if st == 0 {
        st = shell.last_cmd_sub_status().unwrap_or(0);
    }
    st
}

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell) -> ExecOutcome {
    run_exec_single_inner(cmd, shell, false)
}

/// `wrapped` is true when this invocation was reached via the pre-resolve
/// `command <decl>` / `builtin <decl>` rewrite (the inner program is therefore
/// command/builtin-wrapped even though `command_prefix`/`require_builtin` were
/// reset on recursion). It suppresses the bare-special-builtin posix-fatal
/// consume so `command export AA[4]=1` / `builtin export AA[4]=1` strip the
/// usage/assignment fatal, matching bash.
fn run_exec_single_inner(cmd: &ExecCommand, shell: &mut Shell, wrapped: bool) -> ExecOutcome {
    // POSIX case #1: clear any usage-error signal a prior command may have left
    // un-consumed (e.g. a `command`-wrapped special builtin). Each special
    // builtin re-sets it at its own usage/assignment error site during dispatch.
    shell.builtin_usage_error = None;
    // Stamp $LINENO from the parse-time source line before any expansion.
    // The guard prevents synthesized line-0 commands (rewrites, builtins-via-command)
    // from clobbering a real current line.
    if cmd.line != 0 {
        shell.current_lineno = shell.line_base() + cmd.line;
    }
    // Snapshot the procsub stack. Any process substitutions realized during
    // argument expansion (resolve()) or redirect expansion are recorded in
    // shell.procsub_pending. We drain [procsub_base..] on every exit path so
    // the parent fd is closed and the inner child is reaped after the command
    // runs. The two recursive pre-resolve paths (command/builtin re-dispatch)
    // return before any expansion, so the drain is a no-op on those paths.
    let procsub_base = shell.procsub_pending.len();
    match match debug_trap_gate(shell) {
        Ok(d) => d,
        Err(o) => return o,
    } {
        crate::traps::DebugDecision::Proceed => {}
        crate::traps::DebugDecision::SkipCommand => {
            return ExecOutcome::Continue(shell.last_status());
        }
        crate::traps::DebugDecision::ReturnFromSub(n) => {
            return ExecOutcome::FunctionReturn(n);
        }
    }

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
        return run_exec_single_inner(&inner, shell, true);
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
        return run_exec_single_inner(&inner, shell, true);
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
        // `File{ReadOnly}` (fd 0), and a `#[cfg(test)]` capture is active.
        // (Production `$(< file)` is handled earlier by
        // `expand::try_read_file_substitution`; this mirrors it for the
        // in-process test capture.)
        if cmd.inline_assignments.is_empty()
            && cmd.redirects.len() == 1
            && crate::capture_test_hook::active()
        {
            let redir = &cmd.redirects[0];
            if let RedirOp::File {
                mode: crate::command::FileMode::ReadOnly,
                target,
            } = &redir.op
                && redir.target_fd() == Some(0)
            {
                let path = match expand_single(target, shell, &mut *err_writer()) {
                    Ok(p) => p,
                    Err(()) => {
                        drain_procsubs(shell, procsub_base);
                        return ExecOutcome::Continue(1);
                    }
                };
                let outcome = match std::fs::read(&path) {
                    Ok(bytes) => {
                        crate::capture_test_hook::push_out(&bytes);
                        ExecOutcome::Continue(0)
                    }
                    Err(e) => {
                        redir_open_error(shell, &path, &e);
                        ExecOutcome::Continue(1)
                    }
                };
                drain_procsubs(shell, procsub_base);
                return outcome;
            }
        }
        return run_empty_command_tail(cmd, shell, procsub_base);
    }

    // Bind the result first so the `err_writer` borrow of sink/err_sink ends
    // before the `Ok(None)` arm reuses them.
    let resolved_result = resolve(cmd, shell, &mut *err_writer());
    let mut resolved = match resolved_result {
        Ok(Some(r)) => r,
        // #62: the program word expanded to ZERO fields AND no arg field
        // survived either — bash treats this as an empty command (assignments +
        // redirects, rc 0), NOT `command not found`.
        Ok(None) => return run_empty_command_tail(cmd, shell, procsub_base),
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
        // Scan leading flags via the shared scanner (spec "pVv", same as
        // `builtin_command`'s own scan for the introspect path below) so
        // bundling and the invalid-option diagnostic match bash exactly.
        // bash's internal_getopt scans the WHOLE leading cluster before
        // deciding anything — `command -v -Z ls` still reports `-Z` invalid,
        // so this does not stop at the first `-v`/`-V`.
        let mut diag: Vec<u8> = Vec::new();
        let mut g = crate::builtin_opts::Getopt::new(
            "command",
            crate::builtin_opts::ArgView::Plain(&resolved.args),
            "pVv",
        );
        let mut introspect = false;
        let mut scan_err = None;
        loop {
            match g.next_opt(shell, &mut diag) {
                Ok(Some(o)) => {
                    if matches!(o.ch, 'v' | 'V') {
                        introspect = true;
                    } // 'p' — accept; v99 uses current $PATH
                }
                Ok(None) => break,
                Err(code) => {
                    scan_err = Some(code);
                    break;
                }
            }
        }
        if let Some(code) = scan_err {
            // The scanner already wrote bash's two-line diagnostic into `diag`
            // (unscoped); replay it under the command's own redirect scope —
            // #77: `$(command -Z 2>&1)` must capture the error, not leak it
            // to the real stderr.
            //
            // Note: `g.next_opt` already set `shell.builtin_usage_error =
            // Some(2)` as a side effect of the scan; nothing here consumes
            // it (we return the code directly). Harmless today — it's
            // unconditionally cleared at the top of the next
            // `run_exec_single_inner` — but a latent trap if a future
            // posix-fatality check ever reads it before that clear runs.
            with_redirect_scope(&cmd.redirects, shell, |_shell| {
                let mut err = err_writer();
                let _ = err.write_all(&diag);
                ExecOutcome::Continue(code)
            });
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(code);
        }
        if introspect {
            break; // leave program=="command" -> dispatch runs builtin_command (-v/-V)
        }
        let idx = g.rest_index();
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
        // `builtin` accepts no options at all (spec ""); only `--` terminates.
        // Scanning via the shared scanner (rather than special-casing `-Q` as
        // a bogus target name) makes an invalid option a proper usage error,
        // matching bash.
        let mut diag: Vec<u8> = Vec::new();
        let mut g = crate::builtin_opts::Getopt::new(
            "builtin",
            crate::builtin_opts::ArgView::Plain(&resolved.args),
            "",
        );
        let scan_err = match g.next_opt(shell, &mut diag) {
            Ok(Some(_)) => unreachable!("empty spec never yields an option"),
            Ok(None) => None,
            Err(code) => Some(code),
        };
        if let Some(code) = scan_err {
            // #77: replay the diagnostic under the command's own redirect
            // scope so `$(builtin -Q 2>&1)` captures it instead of leaking
            // to the real stderr. (`shell.builtin_usage_error` is left
            // unconsumed here too — see the matching comment in the
            // `command` loop above.)
            with_redirect_scope(&cmd.redirects, shell, |_shell| {
                let mut err = err_writer();
                let _ = err.write_all(&diag);
                ExecOutcome::Continue(code)
            });
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(code);
        }
        let idx = g.rest_index();
        match resolved.args.get(idx) {
            None => {
                drain_procsubs(shell, procsub_base);
                return ExecOutcome::Continue(0);
            } // `builtin` (or `builtin --`) alone
            Some(_) => {
                let new_program = resolved.args[idx].clone();
                let new_args = resolved.args[idx + 1..].to_vec();
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
        let msg = format!(
            "builtin: {}: declaration builtins must not be wrapped by `command builtin`",
            resolved.program
        );
        emit_wrapper_error(cmd, shell, &msg);
        drain_procsubs(shell, procsub_base);
        return ExecOutcome::Continue(1);
    }
    if require_builtin && !builtins::is_builtin(&resolved.program) {
        let msg = format!("builtin: {}: not a shell builtin", resolved.program);
        emit_wrapper_error(cmd, shell, &msg);
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
    let snap = match apply_inline_assignments(&cmd.inline_assignments, shell) {
        Ok(s) => s,
        Err(s) => {
            restore_inline_assignments(s, shell);
            if builtins::is_special_builtin(&resolved.program) {
                shell.report_error(crate::error_fatality::ErrorKind::Expansion);
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
            xtrace_emit(
                xtrace_target_fd(shell),
                &format!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val)),
            );
        }
        if !resolved.program.is_empty() {
            // #372: a declaration builtin's COMPOUND array operands are traced
            // by bash on their own preceding lines, re-quoted, with the operand
            // itself reduced to the bare NAME on the command line:
            //
            //     declare -A m=([k]=v [j]=w)
            //     + m=(['k']='v' ['j']='w')
            //     + declare -A m
            //
            // huck emitted the raw source as one line. Collected here and
            // emitted below, after any inline-assignment prefix lines.
            let mut compound_lines: Vec<String> = Vec::new();
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
                                let rendered = crate::expand::xtrace_array_literal(elems, shell);
                                compound_lines.push(format!("{name}={rendered}"));
                                parts.push(name.to_string());
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
            for line in compound_lines {
                xtrace_emit(xtrace_target_fd(shell), &format!("{p4}{line}"));
            }
            xtrace_emit(xtrace_target_fd(shell), &format!("{p4}{body}"));
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
        // bash refuses `exec` only when it has a COMMAND WORD to replace the
        // shell with. Redirect-only / bare / options-only `exec` (`exec 3<f`,
        // `exec 2>&1`, `exec -c`) is PERMITTED — its redirections are then
        // subject to the ordinary Op::RedirectFile check applied by
        // `run_exec_builtin`, which is why `exec 3> f` reports the redirect
        // diagnostic, not `exec: restricted`.
        // A bad option falls through so run_exec_builtin reports the usage
        // error, as bash does.
        let has_command_word = parse_exec_flags(&resolved.args)
            .map(|f| resolved.args.len() > f.operand_start)
            .unwrap_or(false);
        // bash evaluates the redirections BEFORE refusing the exec, so a
        // restricted `exec 2>/dev/null true` reports the REDIRECT diagnostic
        // (`/dev/null: restricted: cannot redirect output`), not `exec:
        // restricted`. Pre-scan the redirects in order and let the first
        // refusal speak. Only reachable when the exec is doomed anyway
        // (`Op::Exec` is refused by every restricted policy), so the targets
        // expanded here are never expanded a second time.
        let mut refused_redirect = false;
        if has_command_word && shell.policy.is_restricted() {
            for redir in &cmd.redirects {
                let RedirOp::File { mode, target: word } = &redir.op else {
                    continue;
                };
                let path = match expand_single(word, shell, &mut *err_writer()) {
                    Ok(p) => p,
                    Err(()) => {
                        refused_redirect = true;
                        break;
                    }
                };
                let subject = match &redir.fd {
                    RedirFd::Var(name) => name.as_str(),
                    _ => path.as_str(),
                };
                if check_restricted_redirect(mode, &path, subject, shell).is_err() {
                    refused_redirect = true;
                    break;
                }
            }
        }
        // Refused, with the diagnostic already on stderr in every case: the
        // scan above emits its own, so only the `exec: restricted` verdict is
        // left to report here.
        let refused = refused_redirect
            || (has_command_word
                && match shell.policy.check(crate::policy::Op::Exec) {
                    Ok(()) => false,
                    Err(msg) => {
                        let mut err = err_writer();
                        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
                        true
                    }
                });
        if refused {
            // exec's variable assignments persist (special builtin), but the
            // #28 child-env scalar overlay must NOT — pop it so a later command
            // doesn't inherit this inline scalar. (inline_scopes isn't pushed on
            // this early-return path, so only the overlay is popped.)
            pop_inline_scalar_overlay(snap.overlay_pushed, shell);
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
        let outcome = run_exec_builtin(&resolved, cmd, shell);
        // POSIX case #1: `exec -z` (bad option) exits a non-interactive posix
        // shell. A `command exec`/`builtin exec` wrapper strips it (matches bash).
        // exec returns early here (it never reaches the post-dispatch consume), so
        // mirror that consume on this path, gated identically on a BARE invocation.
        if !wrapped
            && command_prefix.is_empty()
            && !require_builtin
            && let Some(st) = shell.builtin_usage_error.take()
        {
            shell
                .report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: st });
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

    if let Err(msg) = shell
        .policy
        .check(crate::policy::Op::CommandName(&resolved.program))
    {
        {
            let mut err = err_writer();
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
        run_builtin_with_redirects(&resolved, &cmd.redirects, shell)
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned()
    {
        let name = resolved.program.clone();
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, move |shell| {
                call_function(&name, body, args, shell)
            })
        } else {
            call_function(&name, body, args, shell)
        }
    // eval/source must run their commands with the ENCLOSING sink (so `$(eval …)`
    // / `$(source …)` captures the output) and honour redirects — like a function
    // call. The generic builtin path below routes them through run_builtin, which
    // resets to a fresh Terminal sink (leaking the output and re-entering job
    // control inside a substitution → the nvm ls-remote hang), so intercept here.
    } else if resolved.program == "eval" {
        let args = resolved.args;
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, move |shell| {
                builtins::eval_in_sink(&args, shell)
            })
        } else {
            builtins::eval_in_sink(&args, shell)
        }
    } else if resolved.program == "source" || resolved.program == "." {
        let args = resolved.args;
        let invoked = resolved.program.clone();
        if has_any_redirect(cmd) {
            with_redirect_scope(&cmd.redirects, shell, move |shell| {
                builtins::source_in_sink(&args, &invoked, shell)
            })
        } else {
            builtins::source_in_sink(&args, &invoked, shell)
        }
    } else if builtins::builtin_active(&resolved.program, shell) {
        // v156 task 7: ALL redirects flow through one ordered RedirectScope (via
        // run_builtin_with_redirects) applied to the real fds in source order, so
        // `echo x 2>&1 >file`, `>file 3>&1`, `read < file`, `cmd <<<here` etc. all
        // honor source order and fd>2 uniformly — no more last-wins bridge. On a
        // redirect-open failure the helper rolls back and returns Continue(1); we
        // still owe the inline-assignment restore for temporary-scope targets.
        run_builtin_with_redirects(&resolved, &cmd.redirects, shell)
    } else {
        // v156 task 4: lower the FULL ordered redirect list (on the original
        // ExecCommand, not the bridged ResolvedCommand) into a child replay plan.
        // Files are opened (and heredoc bodies delivered via `heredoc_body_to_fd`)
        // in the parent here; the child replays the dup2/close ops in source
        // order. This handles fd>2,
        // `<&` dup-in, `N>&-` close, and `<>` uniformly with fds 0/1/2.
        match build_child_redir_plan(&cmd.redirects, shell) {
            Ok(plan) => run_subprocess(&resolved, plan, shell),
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
        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage { status: st });
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

/// Why `exec`'s flag parse failed. A structured error, not a rendered message,
/// because this parser must stay PURE: it is called twice — once at the
/// restricted-policy check purely to ask whether a command word is present
/// (errors deliberately swallowed there), and once for real. Emitting inside it
/// would report the diagnostic twice. The caller that reports renders it via
/// the shared emitters, so `exec` says exactly what every scanner-based builtin
/// says, usage line included (#516).
#[derive(Debug)]
enum ExecFlagErr {
    /// The raw BYTE of the rejected option — bash scans the flag word byte by
    /// byte, so `exec -é` names only the first of the two bytes `é` occupies
    /// (#522). A `char` here would name the whole code point.
    Invalid(u8),
    MissingValue(char),
}

/// Parses leading `-c`/`-l`/`-a NAME` flags from `exec`'s arguments. Stops at
/// the first non-flag word or `--`.
///
/// Deliberately NOT on the shared `builtin_opts` scanner: its scanning is
/// already bash-correct (bundling, `--`, a lone `-` as an operand, `-aNAME`
/// attached or separate, stop-at-first-non-option all measured identical), and
/// the scanner emits as it goes, which this function must not do. What #516 was
/// really about — the missing usage line — is fixed by sharing the EMIT, above.
fn parse_exec_flags(args: &[String]) -> Result<ExecFlags, ExecFlagErr> {
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
        // Bytes, not chars: bash's scan is byte-wise, which is what makes the
        // diagnostic for a non-ASCII flag name one raw byte (#522). Slicing
        // `body` at `j + 1` stays on a char boundary because every byte before
        // it matched an ASCII flag letter.
        let bytes = body.as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            match bytes[j] {
                b'c' => f.clear_env = true,
                b'l' => f.login = true,
                b'a' => {
                    // `-a NAME`: the rest of this word, else the next word.
                    let rest: String = body[j + 1..].to_string();
                    if rest.is_empty() {
                        i += 1;
                        if i >= args.len() {
                            return Err(ExecFlagErr::MissingValue('a'));
                        }
                        f.argv0 = Some(args[i].clone());
                    } else {
                        f.argv0 = Some(rest);
                    }
                    i += 1;
                    continue 'outer;
                }
                other => return Err(ExecFlagErr::Invalid(other)),
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
fn apply_redirects_permanently(cmd: &ExecCommand, shell: &mut Shell) -> Result<(), ()> {
    let mut scope = RedirectScope::new();

    // Resolve-then-apply each redirection INTERLEAVED in source order via the
    // ordered RedirectScope applier (`apply_redirects` → `lower_one_redirect` +
    // `apply_one`). Handles numeric fds, dup, close, heredoc, and `{var}`
    // uniformly. On failure, `scope` Drop rolls back any already-applied
    // redirects atomically (temporary semantics) and we return Err(()) to the
    // caller.
    if scope.apply_redirects(&cmd.redirects, shell).is_err() {
        // scope Drop restores the partially-applied redirects (atomic rollback).
        return Err(());
    }

    // SUCCESS → make the redirections permanent.
    //
    // For `{var}` redirections the `NamedFd` arm leaves the high fd OPEN and
    // does NOT register it in `scope.saved`, so it already persists beyond this
    // function — no special-casing needed.
    //
    // #169: a heredoc/here-string body is delivered via `heredoc_body_to_fd`
    // (pipe or temp file) with no forked writer, so a PERMANENT redirect (whose
    // reader is a *later* command) has nothing left to hang on — the body is
    // already fully written before this function returns.

    // Close each saved-original fd (or skip -1 = "was closed before us") so
    // Drop's restore loop has nothing to do. Draining first means Drop sees an
    // empty `saved` vec even if it somehow runs.
    for (_target, saved) in scope.saved.drain(..) {
        // Dropping the owned dup (if any) closes it; None was never open.
        drop(saved);
        // saved == -1 means the target fd was previously free/closed — there is
        // no original fd to restore, so we leave the target fd open (permanent).
    }

    // Drop the scope normally: `saved` was drained+closed above, so Drop's
    // restore loop is a no-op — but dropping (rather than `mem::forget`) FREES
    // the scope's heap (the `saved` Vec buffer). Forgetting leaked ~100 bytes
    // per `exec` redirect, unbounded over a long-running process (#178).
    drop(scope);
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
) -> ExecOutcome {
    let flags = match parse_exec_flags(&resolved.args) {
        Ok(f) => f,
        Err(e) => {
            {
                let mut err = err_writer();
                // Shared emitters (#516): same two-line shape, and the usage
                // line comes from the same `usage_for` table as every other
                // builtin's. They also set `builtin_usage_error`, which the
                // exec interception consumes for a bare invocation.
                match e {
                    ExecFlagErr::Invalid(c) => {
                        crate::builtin_opts::emit_invalid_option("exec", c, shell, &mut *err)
                    }
                    ExecFlagErr::MissingValue(c) => {
                        crate::builtin_opts::emit_missing_value("exec", c, shell, &mut *err)
                    }
                }
            }
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
        if apply_redirects_permanently(cmd, shell).is_err() {
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
                let mut err = err_writer();
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
        let mut errw = err_writer();
        crate::sh_error_to!(shell, &mut *errw, None, "exec: {name}: {err}");
    }
    exit_or_continue(code, shell)
}

/// A single pure-fd operation replayed in a child `pre_exec` (v156 task 4).
/// Both variants use only async-signal-safe libc calls (`dup2`/`close`). File
/// opens and heredoc body delivery (`heredoc_body_to_fd`) happen in the PARENT
/// before the spawn; the resulting source fd is inherited across fork and
/// named here by number.
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

/// The neutral result of BATCH-lowering an ordered redirect list (`lower_redirects`):
/// what fds the command will see, resolved but not yet installed. Consumed by the
/// child path only — `redir_plan_to_child` (child dup2/close replay). The in-process
/// path does not build a `RedirPlan`; it interleaves `lower_one_redirect` +
/// `RedirectScope::apply_one` per redirect (see `apply_redirects`). Ownership of every
/// parent-opened temp (files, heredoc read ends, `{var}` high fds) lives INSIDE
/// the ops, so a lowering error drops them (no leak; P1 discipline).
struct RedirPlan {
    ops: Vec<PlanOp>,
}

/// One resolved, ordered redirect action. Source order is preserved.
enum PlanOp {
    /// A parent-opened temp (`>file`, heredoc/here-string read end) duped onto
    /// `target`. In-process: if `source`'s fd == `target` (a relocated file that
    /// landed on its own target, target >= 10) clear FD_CLOEXEC in place (#135);
    /// else dup2 + save/restore, then close the temp. Child: dup2 (replay's
    /// `source == target` arm clears CLOEXEC), temp held until spawn.
    InstallOwned {
        target: RawFd,
        source: std::os::fd::OwnedFd,
    },
    /// A borrowed shell fd (`>&w` / `<&w`, and the dup half of a move). `source`
    /// is a resolved fd NUMBER. In-process: dup2 + save/restore (the source was
    /// already validated open by `lower_one_redirect` against the real fds). Child:
    /// dup2 (no validation — the fd is inherited).
    InstallDup { target: RawFd, source: RawFd },
    /// `N>&-`, and the source-close half of a move (`>&w-`).
    Close { target: RawFd },
    /// `{var}` named-fd. `high` is the parent-parked live descriptor (already
    /// allocated non-CLOEXEC, >= 10). In-process: assign `$name = high` and let it
    /// persist (take it out of the plan; do NOT save/restore) — `virtual_fd` is
    /// `high` there and is ignored. Child: `dup2(high -> virtual_fd)` then
    /// `close(high)` (unless equal), keeping `high` held until fork; `$name` is set
    /// to `virtual_fd` DURING batch lowering (so a later sibling `2>&$v` resolves
    /// to it) but restored by `lower_redirects` — bash doesn't persist `{var}` to
    /// the parent for an external command.
    NamedFd {
        high: std::os::fd::OwnedFd,
        // Read by the in-process applier (`RedirectScope::apply_one`) to assign
        // `$name`; the child path (`redir_plan_to_child`) assigns `virtual_fd`.
        name: String,
        /// Child path: the virtual destination the parked `high` is duped onto
        /// (lowest fd >= 10 not used as an earlier plan target / earlier `{var}`
        /// virtual). Equals `high` on the in-process build (where it is unused).
        virtual_fd: RawFd,
    },
}

/// The parent-side result of lowering `cmd.redirects` into an ordered replay
/// list for an external (forked) command. `ops` is applied IN ORDER in the
/// child's `pre_exec`. `held` keeps the parent-opened files / heredoc read-ends
/// alive (and FD_CLOEXEC'd, so they vanish on the child's exec while the dup2'd
/// targets survive) until after `spawn`.
struct ChildRedirPlan {
    ops: Vec<ChildRedirOp>,
    held: Vec<std::os::fd::OwnedFd>,
}

/// Relocate a freshly-opened parent fd to a high number (>= 10) with FD_CLOEXEC,
/// returning the new fd and closing the original. This keeps parent-opened
/// redirect *source* fds out of the low 0..9 range that explicit redirect
/// *targets* (e.g. `3>&1 2>&3`) operate on, so a source fd never collides with a
/// fd the child is still swapping (matches how bash relocates redirect fds).
/// On fcntl failure the original fd is returned unchanged (best-effort).
fn relocate_high_cloexec(fd: RawFd) -> RawFd {
    crate::child_fd::move_to_high_fd(fd, 10, true).unwrap_or_else(|_| {
        // Could not relocate (e.g. EMFILE) — fall back to the original fd with
        // CLOEXEC set; collisions are unlikely in the common case.
        crate::child_fd::set_cloexec(fd);
        fd
    })
}

/// Resolve a SINGLE redirection into 0-2 neutral `PlanOp`s: opens files (as
/// OwnedFd), delivers heredoc bodies via `heredoc_body_to_fd`, resolves dup
/// WORDS to fd NUMBERS, and allocates `{var}` high fds. Shared by
/// the batch `lower_redirects` (child path, `fd_state: Some(..)`, validating
/// dup sources against the plan simulation) and the in-process interleaved
/// applier (`fd_state: None`, validating dup sources against the real fd
/// table since earlier redirects are already applied). Does not reap/close on
/// error — the caller does that.
fn lower_one_redirect(
    redir: &Redirection,
    shell: &mut Shell,
    mut fd_state: Option<&mut std::collections::HashMap<RawFd, bool>>,
) -> Result<Vec<PlanOp>, i32> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut ops: Vec<PlanOp> = Vec::new();
    if let RedirFd::Var(name) = &redir.fd {
        if matches!(&redir.op, RedirOp::Close) {
            let cur = shell.lookup_var(name).unwrap_or_default();
            let fd: RawFd = match cur.trim().parse::<i32>() {
                Ok(n) if n >= 0 => n,
                _ => {
                    {
                        let mut err = err_writer();
                        crate::sh_error_to!(shell, &mut *err, None, "{name}: ambiguous redirect");
                    }
                    return Err(1);
                }
            };
            ops.push(PlanOp::Close { target: fd });
            if let Some(st) = fd_state.as_deref_mut() {
                st.insert(fd, false);
            }
            return Ok(ops);
        }
        let is_move = matches!(&redir.op, RedirOp::Move { .. });
        // Resolve the source fd. `owned_src` holds an fd we opened (File / heredoc /
        // here-string read end) that must be closed after duping to `high`; `dup_src`
        // is a borrowed shell fd number (a Dup/Move source) left alone.
        let mut owned_src: Option<OwnedFd> = None;
        let mut dup_src: Option<RawFd> = None;
        // The raw Dup/Move source word, threaded out of the match so the dup
        // validation below can name it in bash's error (`$v: Bad file descriptor`).
        let mut dup_source_word: Option<&crate::lexer::Word> = None;
        match &redir.op {
            RedirOp::File { mode, target: word } => {
                let path = match expand_single(word, shell, &mut *err_writer()) {
                    Ok(p) => p,
                    Err(()) => return Err(1),
                };
                // `{name}> f`: bash names the VARIABLE in the restricted
                // diagnostic, not the resolved file.
                if check_restricted_redirect(mode, &path, name, shell).is_err() {
                    return Err(1);
                }
                // RawLow: the {var}-fd relocation happens once below via dup_to_high_fd.
                match open_redirect_file(
                    mode,
                    &path,
                    shell.shell_options.noclobber,
                    FdPlacement::RawLow,
                ) {
                    Ok(fd) => owned_src = Some(fd),
                    Err(e) => {
                        redir_open_error(shell, &path, &e);
                        return Err(1);
                    }
                }
            }
            RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
                // #154: a `{var}`-fd dup whose source is not a valid fd NUMBER —
                // a multi/zero-field expansion (`{zz}>&$unset`) OR a single
                // non-numeric field (`{zz}>&foo`) — is an `ambiguous redirect`
                // naming the `{var}` NAME (bash), not the source word or `bad fd`
                // (that is `resolve_dup_source`'s plain-fd rule). A numeric source
                // proceeds to the dup validation below, which names the NUMBER on
                // a closed fd (`{zz}>&7` → `7: Bad file descriptor`).
                let fields = expand(source, shell);
                let parsed = if fields.len() == 1 {
                    fields
                        .into_iter()
                        .next()
                        .unwrap()
                        .chars
                        .parse::<RawFd>()
                        .ok()
                } else {
                    None
                };
                let src = match parsed {
                    Some(fd) => fd,
                    None => {
                        let mut err = err_writer();
                        crate::sh_error_to!(shell, &mut *err, None, "{name}: ambiguous redirect");
                        return Err(1);
                    }
                };
                dup_src = Some(src);
                dup_source_word = Some(source);
            }
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                let tmpdir = shell.lookup_var("TMPDIR");
                match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                    Ok(rfd) => {
                        owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer();
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
                let tmpdir = shell.lookup_var("TMPDIR");
                match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                    Ok(rfd) => {
                        owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer();
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
        }
        // Validate a Dup/Move source at lower time. No owned file has been opened
        // for a Dup/Move (`owned_src` is None), so an error here truncates nothing.
        if let Some(s) = dup_src {
            let label = dup_source_word
                .map(crate::expand::reconstruct_word_source)
                .unwrap_or_else(|| s.to_string());
            // #140a: a `{var}` dup of a bad fd prints TWO lines in bash — a leading
            // `redirection error: cannot duplicate fd: <strerror>` (bash's
            // redirection_error path, which omits `line N:`) followed by the standard
            // `<word>: Bad file descriptor`. Emit the leading line via the no-line
            // emitter (so huck also omits `line N:`, matching bash's raw bytes), then
            // let the normal validation emit the second (line-prefixed) and return Err.
            if !validate_source_is_open(s, fd_state.as_deref()) {
                {
                    let mut err = err_writer();
                    crate::sh_error_noline_to!(
                        shell,
                        &mut *err,
                        None,
                        "redirection error: cannot duplicate fd: {}",
                        crate::bash_io_error(&std::io::Error::from_raw_os_error(libc::EBADF))
                    );
                }
            }
            validate_source(s, fd_state.as_deref(), shell, &label)?;
        }
        let raw_src: RawFd = match (&owned_src, dup_src) {
            (Some(o), _) => o.as_raw_fd(),
            (None, Some(s)) => s,
            _ => unreachable!("resolved exactly one source"),
        };
        let high = match crate::child_fd::dup_to_high_fd(raw_src, 10, false) {
            Ok(h) => h,
            Err(e) => {
                // owned_src drops here (closes it); a dup_src is the shell's, left open.
                {
                    let mut err = err_writer();
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
        // Close the owned source now that it's been duped to `high`.
        drop(owned_src);
        // #224: a READONLY `{var}` is bash's two-line refusal, and the redirect
        // FAILS — the command does not run. Checked after the source is in
        // hand so the diagnostics come in bash's order, and before `$name` is
        // written by either path.
        if shell.is_readonly(name) {
            {
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "{name}: readonly variable");
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "{name}: cannot assign fd to variable"
                );
            }
            unsafe {
                libc::close(high);
            }
            return Err(1);
        }
        // Child (batch, `fd_state = Some`) path: allocate a VIRTUAL destination fd
        // — the lowest number >= 10 that is not already used as a target by an
        // earlier plan op and not an earlier `{var}`'s virtual number (both are
        // recorded `true` in `fd_state`). Child replay dup2s the parked `high`
        // onto `virtual_fd` and closes `high`, so `cmd 3>a {v}>x` sees fd 10 like
        // bash rather than the parked 11 (#141). Set `$name = virtual_fd` DURING
        // lowering so a later sibling `2>&$v` resolves to it (#140d) — the caller
        // (`lower_redirects`) snapshots/restores `$name`, since bash does not
        // persist a `{var}` to the parent for an external command. In-process
        // (`fd_state = None`): `virtual_fd == high` and `$name` is assigned by
        // `apply_one` at apply time, so nothing changes here.
        let virtual_fd = if let Some(st) = fd_state.as_deref_mut() {
            let mut v: RawFd = 10;
            while st.get(&v) == Some(&true) {
                v += 1;
            }
            st.insert(v, true);
            shell.set(name, v.to_string());
            v
        } else {
            high
        };
        ops.push(PlanOp::NamedFd {
            high: unsafe { OwnedFd::from_raw_fd(high) },
            name: name.to_string(),
            virtual_fd,
        });
        if is_move {
            // Move: close the original source (a shell fd) after the dup. Only a
            // Dup/Move source reaches here (owned sources aren't moves).
            if let Some(s) = dup_src {
                ops.push(PlanOp::Close { target: s });
                if let Some(st) = fd_state.as_deref_mut() {
                    st.insert(s, false);
                }
            }
        }
        return Ok(ops);
    }
    // UNREACHABLE: `target_fd()` returns `None` only for `RedirFd::Var`, and the
    // `RedirFd::Var` block above returns on every path — so `target` is always
    // `Some` here. Left as a bare message (no source word is in scope to name for
    // #152); a future reader should not puzzle over why it wasn't updated.
    let Some(target) = redir.target_fd() else {
        {
            let mut err = err_writer();
            crate::sh_error_to!(shell, &mut *err, None, "ambiguous redirect");
        }
        return Err(1);
    };
    let target = target as RawFd;
    match &redir.op {
        RedirOp::File { mode, target: word } => {
            let path = match expand_single(word, shell, &mut *err_writer()) {
                Ok(p) => p,
                Err(()) => return Err(1),
            };
            if check_restricted_redirect(mode, &path, &path, shell).is_err() {
                return Err(1);
            }
            let owned = match open_redirect_file(
                mode,
                &path,
                shell.shell_options.noclobber,
                FdPlacement::Relocated,
            ) {
                Ok(fd) => fd,
                Err(e) => {
                    redir_open_error(shell, &path, &e);
                    return Err(1);
                }
            };
            ops.push(PlanOp::InstallOwned {
                target,
                source: owned,
            });
            if let Some(st) = fd_state.as_deref_mut() {
                st.insert(target, true);
            }
        }
        RedirOp::Dup { source, .. } | RedirOp::Move { source, .. } => {
            let is_move = matches!(&redir.op, RedirOp::Move { .. });
            let is_output_dup = matches!(&redir.op, RedirOp::Dup { output: true, .. });
            // #542: what bash names in `Bad file descriptor`, measured. Three
            // cases, in this order:
            //   * the word is a LITERAL fd number (bash's `[n]>&NUMBER`
            //     production) -> its NUMERIC VALUE, whatever the target:
            //     `4>&77` says `77` and `4>&007` says `7` (not `007`).
            //     Quoting defeats the production, so `4>&'77'` says `4`.
            //   * else the redirect targets its DIRECTIONAL DEFAULT fd
            //     (`>&`/`1>&` -> 1, `<&`/`0<&` -> 0) -> the SOURCE WORD,
            //     e.g. `>&"$x"` says `"$x"`.
            //   * else -> the TARGET fd: `4>&"$z"` says `4`.
            // Explicitness of the prefix is NOT the discriminator — `1>&""`
            // names the word while `2>&""` names `2`, though both spell an fd.
            let word_src = crate::expand::reconstruct_word_source(source);
            let literal_fd = (!word_src.is_empty() && word_src.bytes().all(|b| b.is_ascii_digit()))
                .then(|| word_src.parse::<RawFd>().unwrap_or(RawFd::MAX));
            let bad_fd_label = match literal_fd {
                Some(n) => n.to_string(),
                None if target == RawFd::from(redir.op.default_fd()) => word_src,
                None => target.to_string(),
            };
            // #223: `>&word` / `1>&word` (an OUTPUT dup on fd 1) whose word is a
            // single NON-NUMERIC, non-`-` field is a synonym for `&>word` —
            // redirect BOTH stdout and stderr to that file. Resolve from ONE
            // expansion (so a `>&$(cmd)` source isn't run twice) and take the
            // fallback when the word is not an fd number.
            let src = if is_output_dup && !is_move && target == libc::STDOUT_FILENO {
                // Expand exactly like a `>file` target (`expand_single`): a single
                // field after glob/split, or "<word>: ambiguous redirect" + Err on
                // many — so `>&*.two-matches` and `>&$x`(x="a b") diverge to the
                // same error bash gives. Also runs any `$(...)` in the source once.
                let field = match expand_single(source, shell, &mut *err_writer()) {
                    Ok(p) => p,
                    Err(()) => return Err(1),
                };
                if field == "-" {
                    // `>&$x` with `x=-` closes the target fd, exactly like a
                    // literal `>&-` (which the parser routes to `RedirOp::Close`
                    // before reaching here). Do NOT treat `-` as a filename.
                    ops.push(PlanOp::Close { target });
                    if let Some(st) = fd_state.as_deref_mut() {
                        st.insert(target, false);
                    }
                    return Ok(ops);
                }
                if field.is_empty() {
                    // #542: an empty expansion is a BAD FD, not a filename.
                    // Falling into the `&>file` branch made `>&"${ARR[0]}"`
                    // (array unset, e.g. a reaped coproc) try to open "" and
                    // report `: No such file or directory`.
                    let mut err = err_writer();
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "{bad_fd_label}: Bad file descriptor"
                    );
                    return Err(1);
                }
                match fd_word_number(&field) {
                    Some(fd) => fd, // `>&2` etc. — a normal dup; fall through below.
                    None => {
                        // `&>file` fallback: open the file on fd 1, then fd 2 <- fd 1.
                        use crate::command::FileMode;
                        if check_restricted_redirect(&FileMode::Truncate, &field, &field, shell)
                            .is_err()
                        {
                            return Err(1);
                        }
                        let owned = match open_redirect_file(
                            &FileMode::Truncate,
                            &field,
                            shell.shell_options.noclobber,
                            FdPlacement::Relocated,
                        ) {
                            Ok(fd) => fd,
                            Err(e) => {
                                redir_open_error(shell, &field, &e);
                                return Err(1);
                            }
                        };
                        ops.push(PlanOp::InstallOwned {
                            target: libc::STDOUT_FILENO,
                            source: owned,
                        });
                        if let Some(st) = fd_state.as_deref_mut() {
                            st.insert(libc::STDOUT_FILENO, true);
                        }
                        ops.push(PlanOp::InstallDup {
                            target: libc::STDERR_FILENO,
                            source: libc::STDOUT_FILENO,
                        });
                        if let Some(st) = fd_state.as_deref_mut() {
                            st.insert(libc::STDERR_FILENO, true);
                        }
                        return Ok(ops);
                    }
                }
            } else {
                match resolve_dup_source(source, shell, &bad_fd_label) {
                    Ok(DupSource::Fd(n)) => n,
                    Ok(DupSource::CloseTarget) => {
                        // `m=-; exec 3>&$m` — close the redirector, like `3>&-`.
                        ops.push(PlanOp::Close { target });
                        if let Some(st) = fd_state.as_deref_mut() {
                            st.insert(target, false);
                        }
                        return Ok(ops);
                    }
                    Err(()) => return Err(1),
                }
            };
            // Degenerate `N>&N-` (source == target): bash no-op (redir.c's
            // `redir_fd != redirector` guard). Contributes nothing.
            if !(is_move && src == target) {
                // Validate the source NOW (before any later file opens) so an invalid
                // dup errors without truncating a later `>file`. Same-plan targets are
                // recorded open in fd_state, so `3>g 4>&3` still passes. `bad_fd_label`
                // carries bash's naming rule (source word on the directional default
                // fd, target number otherwise — #542); a numeric literal (`>&9`)
                // reconstructs back to its own number, unchanged.
                validate_source(src, fd_state.as_deref(), shell, &bad_fd_label)?;
                ops.push(PlanOp::InstallDup {
                    target,
                    source: src,
                });
                if let Some(st) = fd_state.as_deref_mut() {
                    st.insert(target, true);
                }
                if is_move {
                    ops.push(PlanOp::Close { target: src });
                    if let Some(st) = fd_state.as_deref_mut() {
                        st.insert(src, false);
                    }
                }
            }
        }
        RedirOp::Close => {
            ops.push(PlanOp::Close { target });
            if let Some(st) = fd_state.as_deref_mut() {
                st.insert(target, false);
            }
        }
        RedirOp::Heredoc { body, .. } => {
            let bytes = expand_assignment(body, shell).into_bytes();
            let tmpdir = shell.lookup_var("TMPDIR");
            match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                Ok(rfd) => {
                    let rfd = relocate_high_cloexec(rfd);
                    let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                    ops.push(PlanOp::InstallOwned {
                        target,
                        source: owned,
                    });
                    if let Some(st) = fd_state.as_deref_mut() {
                        st.insert(target, true);
                    }
                }
                Err(e) => {
                    {
                        let mut err = err_writer();
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
            let tmpdir = shell.lookup_var("TMPDIR");
            match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                Ok(rfd) => {
                    let rfd = relocate_high_cloexec(rfd);
                    let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                    ops.push(PlanOp::InstallOwned {
                        target,
                        source: owned,
                    });
                    if let Some(st) = fd_state {
                        st.insert(target, true);
                    }
                }
                Err(e) => {
                    {
                        let mut err = err_writer();
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
    Ok(ops)
}

/// The single redirect lowering (Phase 3a): a batch loop over
/// `lower_one_redirect` for the child (fork) path. Walks `redirects` in
/// source order, threading a `fd_state` simulation (fds this plan opens/closes)
/// through each item so a same-plan dup source (e.g. `3>g 4>&3`) validates
/// correctly. On any error it closes every fd opened so far (dropping
/// `plan.ops`) and returns Err(code) with the diagnostic already printed.
fn lower_redirects(redirects: &[Redirection], shell: &mut Shell) -> Result<RedirPlan, i32> {
    // Snapshot every `{var}` name a redirect in this batch will assign. During
    // lowering `lower_one_redirect` sets `$name` to the virtual fd so a later
    // sibling `2>&$v` resolves against it (#140d), but bash does NOT persist a
    // `{var}` to the parent for an external command — so restore the prior value
    // (unset or otherwise) before returning, on both the success and error paths.
    // Mirrors the inline-assignment snapshot/restore. Snapshot each name once
    // (its pre-command value); restore LIFO.
    let mut var_snaps: Vec<(String, Option<crate::shell_state::Variable>)> = Vec::new();
    for redir in redirects {
        if let RedirFd::Var(name) = &redir.fd
            && !var_snaps.iter().any(|(n, _)| n == name)
        {
            var_snaps.push((name.clone(), shell.snapshot_var(name)));
        }
    }
    let restore = |shell: &mut Shell,
                   snaps: Vec<(String, Option<crate::shell_state::Variable>)>| {
        for (name, prior) in snaps.into_iter().rev() {
            shell.restore_var(&name, prior);
        }
    };
    let mut fd_state: std::collections::HashMap<RawFd, bool> = std::collections::HashMap::new();
    let mut plan = RedirPlan { ops: Vec::new() };
    for redir in redirects {
        match lower_one_redirect(redir, shell, Some(&mut fd_state)) {
            Ok(ops) => plan.ops.extend(ops),
            Err(code) => {
                // Dropping `plan.ops` closes every fd opened so far (heredoc
                // read ends included). No writers exist to reap (#169).
                plan.ops.clear();
                restore(shell, var_snaps);
                return Err(code);
            }
        }
    }
    restore(shell, var_snaps);
    Ok(plan)
}

/// Lower `redirects` into a `ChildRedirPlan` for an external (forked) command by
/// running the shared `lower_redirects` lowering and translating the neutral
/// `RedirPlan` into the child dup2/close replay plan.
fn build_child_redir_plan(
    redirects: &[Redirection],
    shell: &mut Shell,
) -> Result<ChildRedirPlan, i32> {
    Ok(redir_plan_to_child(lower_redirects(redirects, shell)?))
}

/// Translate a neutral `RedirPlan` into the child dup2/close replay plan. Owned
/// temps move into `held` (kept alive until the fork); `{var}` high fds replay a
/// defensive same-fd op and are held (the child inherits them non-CLOEXEC).
/// `$var` is NOT assigned — bash doesn't for an external command.
fn redir_plan_to_child(plan: RedirPlan) -> ChildRedirPlan {
    use std::os::fd::AsRawFd;
    let mut child = ChildRedirPlan {
        ops: Vec::new(),
        held: Vec::new(),
    };
    for op in plan.ops {
        match op {
            PlanOp::InstallOwned { target, source } => {
                let raw = source.as_raw_fd();
                child.ops.push(ChildRedirOp::Dup {
                    target,
                    source: raw,
                });
                child.held.push(source);
            }
            PlanOp::InstallDup { target, source } => {
                child.ops.push(ChildRedirOp::Dup { target, source });
            }
            PlanOp::Close { target } => child.ops.push(ChildRedirOp::Close { target }),
            PlanOp::NamedFd {
                high,
                name: _,
                virtual_fd,
            } => {
                let raw = high.as_raw_fd();
                // dup2(high -> virtual_fd) wires the command's inherited `{var}`
                // fd to the bash-matching low number; close the parked `high`
                // afterwards (unless it already IS the virtual fd, in which case
                // the `source == target` Dup arm just clears FD_CLOEXEC). `high`
                // stays `held` so the parent keeps it alive across the fork.
                child.ops.push(ChildRedirOp::Dup {
                    target: virtual_fd,
                    source: raw,
                });
                if virtual_fd != raw {
                    child.ops.push(ChildRedirOp::Close { target: raw });
                }
                child.held.push(high);
            }
        }
    }
    child
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
fn emit_exec_spawn_diag(shell: &Shell, stderr_follows_stdout: bool, body: std::fmt::Arguments) {
    // This diagnostic is emitted IN-PROCESS by the parent (the fork/exec never
    // happened), so a `#[cfg(test)]` capture can intercept it. Under a trailing
    // `2>&1` (fd 2 → fd 1) route it to the captured stdout so `$(cmd 2>&1)` sees
    // it; otherwise the captured/real stderr. In production the capture is
    // inactive and both go to the real fd 2 (unchanged from before).
    let stream = if stderr_follows_stdout {
        crate::fd_writer::CaptureStream::Out
    } else {
        crate::fd_writer::CaptureStream::Err
    };
    let mut w = crate::fd_writer::CaptureStderr::new(stream);
    crate::emit_error_to(shell, &mut w, None, body);
}

/// v156 task 4: the single (non-pipeline) external command path. `plan` is the
/// ordered `dup2`/`close` replay list lowered from `cmd.redirects` by
/// `build_child_redir_plan` in the PARENT (files already opened, heredoc bodies
/// already delivered via `heredoc_body_to_fd`). The child replays `plan.ops` IN
/// SOURCE ORDER in a `pre_exec` after the signal-reset hook, so e.g.
/// `3>&1 1>&2 2>&3` performs the fd swap correctly. fds 0/1/2 and fd>2 are all
/// handled uniformly by the replay; this function no longer wires
/// `.stdin/.stdout/.stderr` from opened files (only the capture pipe, which
/// any explicit fd-1 redirect in the replay then overrides).
fn run_subprocess(
    cmd: &ResolvedCommand,
    mut plan: ChildRedirPlan,
    shell: &mut Shell,
) -> ExecOutcome {
    let interactive = shell.job_control_active();

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
    // A heredoc/here-string body was delivered by `build_child_redir_plan` via
    // `heredoc_body_to_fd`; its read end is in `plan.held` (FD_CLOEXEC,
    // inherited across fork, replayed by the ops below).
    // Stage 3 (#197): an external command's stdout/stderr always inherit the
    // shell's real fd 1/2 (the deleted `Capture`/`Merged` sinks wired capture
    // pipes / a `dup2(1,2)` here). Any `>file` / `2>&1` in the SCRIPT is a real
    // redirect replayed in the child via `replay_redir_ops`.
    unsafe {
        process.pre_exec(move || replay_redir_ops(&ops));
    }

    if interactive {
        process.process_group(0);
    }

    // #172: if the program can't be run, don't leave the diagnostic to the
    // parent-side `Err` arms below — they emit in the PARENT (so a `2>file`
    // redirect, applied only in the child, is missed) and always return rc 1.
    // Instead classify BEFORE spawn and, when unrunnable, install a final
    // `pre_exec` (registered last → runs after the redirect replay) that writes
    // the bash-formatted diagnostic to the child's own — now redirected — fd 2
    // and `_exit`s 126/127. `execvp` is never reached, so the message routes to
    // `2>file` / the `$(… 2>&1)` capture pipe / the terminal exactly like a real
    // command, and the existing capture/wait/reap machinery collects the right
    // exit code. Mirrors the pipeline path's child-side emit (#78), reusing this
    // path's ProcessCommand scaffolding rather than a separate diagnostic fork.
    if let StageRunnability::NotRunnable { body, code } =
        classify_command_runnability(&cmd.program, shell)
    {
        let mut diag: Vec<u8> = Vec::new();
        crate::emit_error_to(shell, &mut diag, None, format_args!("{body}"));
        unsafe {
            process.pre_exec(move || {
                // async-signal-safe: raw write + _exit, no allocation.
                libc::write(2, diag.as_ptr() as *const libc::c_void, diag.len());
                libc::_exit(code);
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
    match spawn_result {
        Ok(child) => {
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

            if interactive {
                // Race-close: also setpgid in the parent so the child's pgrp
                // is guaranteed to exist before we call tcsetpgrp.
                setpgid_self(pid);
                // #167: hand the terminal to the child's group only when we own
                // a controlling tty. `set -m` under a pipe still groups + waits.
                if stdin_is_tty() {
                    give_terminal_to(pid);
                }

                // wait_with_untraced already waitpid'd the child, so each arm
                // mem::forget's the Child to keep its Drop from re-reaping
                // (already-reaped pid would give -ECHILD).
                note_blocked_on_child(shell);
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
                            let mut err = err_writer();
                            e!(&mut *err, "\n{line}");
                        }
                        // The command was STOPPED: its process substitutions are
                        // still alive (tied to the stopped job), so the drain in
                        // run_exec_single's epilogue must be non-blocking.
                        shell.fg_stopped = true;
                        std::mem::forget(child);
                        if stdin_is_tty() {
                            give_terminal_to(shell.shell_pgid);
                        }
                        ExecOutcome::Continue(128 + sig)
                    }
                    Ok((raw_status, false)) => {
                        // Child exited or was killed by a signal.
                        let code = raw_status_to_exit_code(raw_status, shell);
                        std::mem::forget(child);
                        if stdin_is_tty() {
                            give_terminal_to(shell.shell_pgid);
                        }
                        ExecOutcome::Continue(code)
                    }
                    Err(()) => {
                        std::mem::forget(child);
                        if stdin_is_tty() {
                            give_terminal_to(shell.shell_pgid);
                        }
                        ExecOutcome::Continue(1)
                    }
                }
            } else {
                // Non-interactive path: stdout/stderr inherit the real fd 1/2
                // since #197 Stage 3 (no capture pipes), so this is simply a
                // blocking wait on the embedder's thread.
                // #418: about to BLOCK on a foreground child — bash reaps (and so may
                // announce a background job's death) exactly at such a point.
                note_blocked_on_child(shell);
                let loop_result = crate::stream_loop::wait_for_child(pid as libc::pid_t);
                // We have already reaped the child via waitpid(WNOHANG) inside
                // the loop. Tell `Child` not to reap (or wait on) again — the
                // pid has been collected and would otherwise be -ECHILD.
                std::mem::forget(child);
                match loop_result {
                    Ok(raw_status) => {
                        let code = raw_status_to_exit_code(raw_status, shell);
                        ExecOutcome::Continue(code)
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer();
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
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // bash format: `<src>: line N: <name>: command not found` (the name
            // precedes the phrase; error_prefix supplies the prologue + mode split).
            emit_exec_spawn_diag(
                shell,
                stderr_follows_stdout,
                format_args!("{}: command not found", cmd.program),
            );
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            emit_exec_spawn_diag(
                shell,
                stderr_follows_stdout,
                format_args!("{}: {}", cmd.program, crate::bash_io_error(&e)),
            );
            ExecOutcome::Continue(1)
        }
    }
}

// ----- multi-stage pipeline -------------------------------------------------

/// Per-stage outcome after spawning: either a forked child pid to be waited
/// for (`Forked`), or — under `shopt lastpipe` — the last stage running
/// in-process in the current shell, recorded by its resolved stdin fd rather
/// than a pid (`InProcess`; not forked, not waited on by `wait_pipeline_raw`,
/// run directly by `run_multi_stage` before the wait).
#[derive(Clone, Copy)]
enum PipelineStage {
    Forked(i32),
    InProcess { stdin_fd: RawFd }, // lastpipe: last stage runs in the parent (not waited)
}

/// Which caller a pipeline is being spawned for. Foreground (`run_multi_stage`)
/// is a strict superset of Background (`run_background_sequence`): the mode-guards
/// in `spawn_pipeline` disable the foreground-only bits (capture wiring, live-pid
/// registry, stage-0 inherit) for Background.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpawnMode {
    Foreground,
    Background,
}

/// The result of spawning every stage of a pipeline (see `spawn_pipeline`). Holds
/// the spawned pids; the caller-specific epilogue (foreground: wait +
/// $PIPESTATUS; background: job registration) lives in the wrapper, not here.
/// Stage 3 (#197): stages always inherit the real fd 1/2, so there are no
/// parent-held capture read ends to carry.
struct SpawnedPipeline {
    first_pid: Option<i32>,
    stages: Vec<PipelineStage>,
    /// The pipeline's target process group (job-control leader pid, else
    /// `NO_PGROUP`). Consumed by the background wrapper (v295 Task 3) to re-assert
    /// the job's process group; the foreground wrapper tracks the terminal handoff
    /// via `first_pid`.
    pgid_target: i32,
    procsub_base: usize,
}

/// Start a coprocess: fork the body with stdin/stdout wired to two pipes, hold
/// the shell-side ends (relocated high + close-on-exec) as NAME[0] (read) /
/// NAME[1] (write), publish NAME_PID, $!, and a job. Returns 0 on a successful
/// spawn (the coproc runs asynchronously), 1 on pipe/fork failure. coproc
/// ALWAYS forks (no builtin/function fast-path).
fn run_coproc(name: &str, body: &Command, shell: &mut Shell) -> ExecOutcome {
    // The parser accepts ANY single non-keyword word as the coproc NAME (bash's
    // grammar `coproc WORD compound-command`) and defers the valid-identifier
    // check to here (RUNTIME), matching bash: `coproc @ { :; }` parses, then at
    // runtime prints `` `@': not a valid identifier `` and does NOT start the
    // coprocess (exit status 1, body not run).
    if !crate::builtins::is_valid_name(name) {
        {
            let mut err = err_writer();
            crate::sh_error_to!(shell, &mut *err, None, "`{}': not a valid identifier", name);
        }
        return ExecOutcome::Continue(1);
    }
    // v157 single-active: warn (but proceed) if a coproc is already live.
    if let Some(existing) = shell.coprocs.first() {
        {
            let mut err = err_writer();
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
    // `child_fd::make_pipe` returns (read_end, write_end).
    // pipe_in: shell writes in_w -> coproc reads in_r (its stdin).
    // pipe_out: coproc writes out_w (its stdout) -> shell reads out_r.
    //
    // #197 Class-A: owned but left raw. These four ends are entangled beyond a
    // safe OwnedFd migration: `in_r`/`out_w` transfer into the forked child via
    // `ChildFd::owned_raw`, `in_w`/`out_r` are relocated by `move_to_high_fd`
    // (which consumes a RawFd), and the surviving parent ends are stored as
    // `RawFd` in the `Coproc` record — which must stay `Clone` (Shell derives
    // Clone), and `OwnedFd` is not `Clone`. Half-migrating any single end would
    // risk a double-close on this path, so the whole set stays raw.
    let (in_r, in_w) = match crate::child_fd::make_pipe(false) {
        Ok(p) => p,
        Err(e) => {
            {
                let mut err = err_writer();
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
    let (out_r, out_w) = match crate::child_fd::make_pipe(false) {
        Ok(p) => p,
        Err(e) => {
            unsafe {
                libc::close(in_r);
                libc::close(in_w);
            }
            {
                let mut err = err_writer();
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
                let mut err = err_writer();
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
    let read_fd = match crate::child_fd::move_to_high_fd(out_r, 10, true) {
        Ok(hi) => hi,
        Err(e) => {
            unsafe {
                libc::close(out_r);
                libc::close(in_w);
            }
            {
                let mut err = err_writer();
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
    let write_fd = match crate::child_fd::move_to_high_fd(in_w, 10, true) {
        Ok(hi) => hi,
        Err(e) => {
            unsafe {
                libc::close(read_fd);
                libc::close(in_w);
            }
            {
                let mut err = err_writer();
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
    // #224: a readonly `NAME_PID` is reported and left alone — the coprocess
    // still starts and the status stays 0 (measured against bash).
    let pid_var = format!("{name}_PID");
    if shell.is_readonly(&pid_var) {
        crate::sh_error!(shell, None, "{pid_var}: readonly variable");
    } else {
        shell.set(pid_var.as_str(), pid.to_string());
    }
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
fn spawn_pipeline(
    commands: &[Command],
    mode: SpawnMode,
    shell: &mut Shell,
) -> Result<SpawnedPipeline, ExecOutcome> {
    // Job-control process-grouping is only correct in the top-level shell. Inside
    // a forked subshell it places the inner pipeline in a background process group
    // with default SIGTTOU/SIGTTIN handling, deadlocking the subshell's wait on a
    // controlling terminal (M-104). A subshell's inner pipeline uses the
    // non-job-control path (stages stay in the subshell's pgrp), matching bash.
    let job_control = shell.job_control_active();
    let interactive = job_control;
    // Mode-aware process-grouping predicate. Foreground groups the pipeline only
    // when it owns the terminal (`interactive`); background groups whenever job
    // control is active (it never owns the terminal, but the job still needs its
    // own process group). Keyed here so every pgid_target computation below stays
    // consistent: fg keeps its exact prior behavior (`group == interactive`), bg
    // gets the leader pid + job_control-keyed pgrouping its wrapper needs.
    let group = match mode {
        SpawnMode::Foreground => interactive,
        SpawnMode::Background => job_control,
    };
    let n = commands.len();

    // lastpipe (#306): under this gate the LAST stage does not fork — its
    // resolved stdin fd is recorded as `PipelineStage::InProcess` and run by
    // `run_multi_stage` directly in the current shell (before the wait), so
    // its assignments/exit status/control flow apply to the shell itself and
    // it is recorded in $PIPESTATUS. Gated to Foreground + job control OFF +
    // Terminal sink: a background pipeline, a job-controlled foreground
    // pipeline (subshell would still be needed for the process group), and a
    // `$()`/capture context (bash itself runs those in a subshell, so a
    // "last stage in the CURRENT shell" would still not be the outer shell)
    // are all out of scope here.
    let lastpipe = mode == SpawnMode::Foreground
        && shell.shopt_options.get("lastpipe").unwrap_or(false)
        && !job_control;

    // lastpipe: the in-process last stage's resolved stdin fd, preserved
    // through the parent bulk-close below.
    let mut inprocess_stdin_fd: Option<RawFd> = None;

    // Pid tracking.
    let mut first_pid: Option<i32> = None;

    // Live-children registry: every stage pid is published while the pipeline
    // is running so the timeout timer thread can SIGTERM all stages in one pass
    // when the deadline fires. Cleared in one bulk pass after wait_pipeline_raw.
    let live_pids_arc = shell.live_external_children.clone();

    // All forked stages (pid + optional inline exit status for Done stages).
    let mut stages: Vec<PipelineStage> = Vec::with_capacity(n);

    // Read-end of the pipe from the previous stage (None for stage 0).
    //
    // #197 Class-A: this inter-stage wiring is owned but deliberately left raw.
    // Each inter-stage read end is tracked simultaneously by fd NUMBER in BOTH
    // `prev_pipe_read` and `parent_held`, while its single true owner alternates
    // between "held for the next stage" and "moved into `ChildFd::owned_raw`"
    // (which transfers it to the child). The `retain`/`position`/`.raw()` filters
    // downstream all key on fd-number identity. Introducing `OwnedFd` into either
    // container without also collapsing the double-tracking would create two
    // owners of one fd — the exact double-close hazard on this hot path — so the
    // whole state machine stays `RawFd` (a coherent migration would be a
    // behavior-visible restructure, out of scope for this sweep).
    let mut prev_pipe_read: Option<RawFd> = None;

    // All raw fds the parent currently holds (for the child's
    // parent_fds_to_close list so it doesn't inherit stale pipe ends).
    let mut parent_held: Vec<RawFd> = Vec::new();

    // Stage 3 (#197): every stage's stdout/stderr inherits the real fd 1/2 (the
    // deleted `Capture`/`Merged` sinks wired per-stage capture pipes / a shared
    // stderr pipe here).

    // Stage-0 stdin default. Foreground inherits the shell's stdin (equivalent to
    // the prior ChildFd::Inherit). Background applies the async rule (#129):
    // /dev/null for a single async command when non-interactive, inherit for a
    // bare multi-stage pipeline / interactive.
    let stage0_default: ChildFd = match mode {
        SpawnMode::Foreground => ChildFd::Inherit,
        SpawnMode::Background => {
            async_default_stdin(commands.len() > 1, shell).map_err(|()| ExecOutcome::Continue(1))?
        }
    };

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

        // v324 (#257): bash fires DEBUG in the PARENT before forking each
        // pipeline stage, in stage order — its action's output must reach the
        // terminal, not the pipe, so this must run here (before any fork
        // below), not inside the forked child. Decision ignored (#262).
        debug_trap_gate(shell)?;

        // ---- Assign-only stages: no-op, just pass stdin through as empty ----
        // Skipped under `is_last && lastpipe`: falling through lets the
        // generic stdin-building code below (the `else` "compound command"
        // branch, since `SimpleCommand::Assign` isn't `Exec`) resolve this
        // stage's stdin fd and hand it to the lastpipe short-circuit, so
        // `run_multi_stage` runs the assignment in-process (persists to the
        // shell) instead of forking it away as inert.
        if let Command::Simple(SimpleCommand::Assign(items, aline)) = stage_cmd
            && !(is_last && lastpipe)
        {
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
            let pgid_target = if group {
                first_pid.unwrap_or(0)
            } else {
                NO_PGROUP
            };
            let stdout_child: ChildFd = if !is_last {
                // Create a pipe; next stage reads from it (will be empty).
                match crate::child_fd::make_pipe(false) {
                    Ok((r, w)) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                        unsafe { ChildFd::owned_raw(w) }
                    }
                    Err(e) => {
                        {
                            let mut err = err_writer();
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "pipe: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        // Clean up all held fds.
                        return Err(bail_teardown_pipeline(
                            mode,
                            shell,
                            procsub_base,
                            first_pid,
                            &stages,
                            &mut parent_held,
                        ));
                    }
                }
            } else {
                // Stage 3 (#197): the last stage's stdout always inherits the
                // real fd 1 (the deleted `Capture` sink wired a capture pipe here).
                ChildFd::Inherit
            };
            // Stage-0 stdin default (a distinct clone of the shared inherit /
            // /dev/null); Foreground's stage0_default is ChildFd::Inherit, so this
            // is equivalent to the prior ChildFd::Inherit.
            let stdin: ChildFd = match stage0_default.try_clone() {
                Ok(c) => c,
                Err(e) => {
                    {
                        let mut err = err_writer();
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "dup: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return Err(bail_teardown_pipeline(
                        mode,
                        shell,
                        procsub_base,
                        first_pid,
                        &stages,
                        &mut parent_held,
                    ));
                }
            };
            let mut fds_to_close: Vec<RawFd> = parent_held
                .iter()
                .copied()
                .filter(|&fd| Some(fd) != stdout_child.raw() && Some(fd) != stdin.raw())
                .collect();
            // The parent's shared stage0_default original (Background /dev/null) is
            // inherited by the child; close it there. Foreground's Inherit has no
            // raw fd, so this is a no-op there.
            if let Some(d) = stage0_default.raw() {
                fds_to_close.push(d);
            }
            match fork_and_run_in_subshell(
                &assign_cmd,
                shell,
                ChildStdio::new(stdin, stdout_child, ChildFd::Inherit),
                pgid_target,
                &fds_to_close,
                None,
                None,
            ) {
                Ok(pid) => {
                    // The pipe/capture write end was owned by the moved
                    // ChildStdio and closed in the parent by the call.
                    if (mode == SpawnMode::Background || interactive) && first_pid.is_none() {
                        first_pid = Some(pid);
                    }
                    if mode == SpawnMode::Foreground {
                        live_pids_arc.lock().unwrap().push(pid as libc::pid_t);
                    }
                    stages.push(PipelineStage::Forked(pid));
                }
                Err(e) => {
                    {
                        let mut err = err_writer();
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "fork: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    return Err(bail_teardown_pipeline(
                        mode,
                        shell,
                        procsub_base,
                        first_pid,
                        &stages,
                        &mut parent_held,
                    ));
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
                return Err(bail_teardown_pipeline(
                    mode,
                    shell,
                    procsub_base,
                    first_pid,
                    &stages,
                    &mut parent_held,
                ));
            }
        };

        // Classify the stage up front: the stdio-base construction differs for
        // External (pipe/capture-only base + a full ChildRedirPlan replayed in the
        // child) vs InProcess (existing slot-base; its body applies redirects). A
        // Simple(Exec) stage can be EITHER kind (external program vs builtin), so we
        // must key the base on the classification, not on Simple-vs-Compound.
        let kind = classify_stage(stage_cmd, shell);
        let stage_is_external = matches!(&kind, StageKind::External(_));

        // #145: a stage's OWN redirect setup failing must fail only this stage,
        // not the whole pipeline. On such a failure we print (already done), set
        // this flag, drop the partial fd, and fall through to spawn a wired
        // exit-1 child at the spawn point (spawn_failed_stage) instead of the
        // real command. Infrastructure failures (make_pipe) still bail.
        let mut redirect_failed = false;

        // ---- Build stdin fd --------------------------------------------------
        // Priority: explicit redirect on ExecCommand > prev_pipe_read > STDIN_FILENO.
        // For InProcess compound stages, there are no explicit redirects at the
        // stage level; the child handles them internally via run_command.

        // A heredoc/herestring stdin is delivered via a pipe or temp file, never a
        // forked writer (#169): the body is expanded NOW (while inline assignments
        // are applied so that $var references see the stage's own inline
        // assignments — v24 deferred-heredoc contract), handed to
        // `heredoc_body_to_fd`, and the read end becomes this stage's stdin.
        let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd
            && !stage_is_external
        {
            match &exec.slot_stdin() {
                Some(RedirectSlot::Read(word)) => {
                    // Take the previous stage's pipe read-end out of parent_held.
                    // On a successful open we CLOSE it (this stage overrides stdin,
                    // so that pipe would otherwise leak into parent_held, keeping
                    // the write-end alive and deadlocking the previous writer). On
                    // an open FAILURE (#145) the exit-1 dummy INHERITS it instead,
                    // so upstream does not SIGPIPE prematurely: a small upstream
                    // write buffers and succeeds exactly as in bash
                    // (`echo A | read x </no | cat` -> PIPESTATUS (0 1 0)), while a
                    // flooding upstream (`yes | …`) still SIGPIPEs once the dummy
                    // exits and closes the read end.
                    let prev = prev_pipe_read.take();
                    if let Some(r) = prev {
                        parent_held.retain(|&fd| fd != r);
                    }
                    let expanded = expand_single(word, shell, &mut *err_writer());
                    match expanded {
                        Err(()) => {
                            // #145: ambiguous-redirect / expansion failure on this
                            // stage's stdin fails ONLY this stage (error already
                            // printed). Hand the previous stage's pipe read-end to
                            // the exit-1 dummy, exactly like the open-failure arm.
                            redirect_failed = true;
                            match prev {
                                Some(r) => unsafe { ChildFd::owned_raw(r) },
                                None => ChildFd::Inherit,
                            }
                        }
                        Ok(path) => match open_redirect_file(
                            &FileMode::ReadOnly,
                            &path,
                            false,
                            FdPlacement::Relocated,
                        ) {
                            Ok(f) => {
                                if let Some(r) = prev {
                                    unsafe {
                                        libc::close(r);
                                    }
                                }
                                ChildFd::from(f)
                            }
                            Err(e) => {
                                redir_open_error(shell, &path, &e);
                                // #145: fail ONLY this stage; hand the previous
                                // stage's pipe read-end to the exit-1 dummy (holds
                                // it until _exit, matching bash's failed stage).
                                redirect_failed = true;
                                match prev {
                                    Some(r) => unsafe { ChildFd::owned_raw(r) },
                                    None => ChildFd::Inherit,
                                }
                            }
                        },
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
                    // then deliver it (pipe or temp file — no forked writer, #169).
                    let bytes = expand_assignment(body, shell).into_bytes();
                    let tmpdir = shell.lookup_var("TMPDIR");
                    match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                        Ok(r) => unsafe { ChildFd::owned_raw(r) },
                        Err(e) => {
                            {
                                let mut err = err_writer();
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return Err(bail_teardown_pipeline(
                                mode,
                                shell,
                                procsub_base,
                                first_pid,
                                &stages,
                                &mut parent_held,
                            ));
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
                    // then deliver it (pipe or temp file — no forked writer, #169).
                    let mut bytes = expand_assignment(body, shell).into_bytes();
                    bytes.push(b'\n');
                    let tmpdir = shell.lookup_var("TMPDIR");
                    match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                        Ok(r) => unsafe { ChildFd::owned_raw(r) },
                        Err(e) => {
                            {
                                let mut err = err_writer();
                                crate::sh_error_to!(
                                    shell,
                                    &mut *err,
                                    None,
                                    "heredoc: {}",
                                    crate::bash_io_error(&e)
                                );
                            }
                            restore_inline_assignments(snap, shell);
                            return Err(bail_teardown_pipeline(
                                mode,
                                shell,
                                procsub_base,
                                first_pid,
                                &stages,
                                &mut parent_held,
                            ));
                        }
                    }
                }
                _ => {
                    // No explicit stdin redirect: use prev_pipe_read or the
                    // stage-0 default (Foreground inherit / Background /dev/null).
                    match prev_pipe_read.take() {
                        Some(r) => {
                            parent_held.retain(|&fd| fd != r);
                            unsafe { ChildFd::owned_raw(r) }
                        }
                        None => match stage0_default.try_clone() {
                            Ok(c) => c,
                            Err(e) => {
                                {
                                    let mut err = err_writer();
                                    crate::sh_error_to!(
                                        shell,
                                        &mut *err,
                                        None,
                                        "dup: {}",
                                        crate::bash_io_error(&e)
                                    );
                                }
                                restore_inline_assignments(snap, shell);
                                return Err(bail_teardown_pipeline(
                                    mode,
                                    shell,
                                    procsub_base,
                                    first_pid,
                                    &stages,
                                    &mut parent_held,
                                ));
                            }
                        },
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
                None => match stage0_default.try_clone() {
                    Ok(c) => c,
                    Err(e) => {
                        {
                            let mut err = err_writer();
                            crate::sh_error_to!(
                                shell,
                                &mut *err,
                                None,
                                "dup: {}",
                                crate::bash_io_error(&e)
                            );
                        }
                        restore_inline_assignments(snap, shell);
                        return Err(bail_teardown_pipeline(
                            mode,
                            shell,
                            procsub_base,
                            first_pid,
                            &stages,
                            &mut parent_held,
                        ));
                    }
                },
            }
        };

        // ---- lastpipe: last stage runs in-process, no fork ------------------
        // `stdin` above is this stage's fully resolved stdin (the previous
        // stage's pipe read end, or an explicit `<`/heredoc/herestring
        // override). Under the gate we don't build stdout/stderr or spawn at
        // all — `run_multi_stage` runs `commands.last()` directly via
        // `run_command` (BEFORE waiting on the forked upstream stages, else
        // they'd deadlock writing into a full pipe with no reader), so it
        // needs the raw fd, not a spawned child. `redirect_failed` (set above
        // only for THIS stage's own stdin resolution) still falls back to the
        // existing forked exit-1-dummy path below, matching every other stage.
        if is_last && lastpipe && !redirect_failed {
            // Take the fd out of `stdin` WITHOUT closing it (`into_raw` never
            // runs `ChildFd`'s Drop). A multi-stage pipeline's last stage
            // always resolves to an OWNED stdin here: `is_last` implies `i >=
            // 1` (n >= 2 — `spawn_pipeline` is only called for multi-stage
            // pipelines), so `prev_pipe_read` is always `Some` by this point
            // unless overridden by an explicit `<`/heredoc/herestring, all of
            // which also produce an owned fd.
            let stdin_fd = stdin
                .into_raw()
                .expect("multi-stage pipeline's last stage always has an owned stdin fd");
            // Preserve it through the parent bulk-close below (mirrors
            // capture_read_fd/capture_err_read_fd) — belt-and-suspenders: the
            // fd was already removed from `parent_held` when it was resolved
            // into `stdin` above, so it wouldn't be touched by that loop
            // regardless, but this keeps the invariant explicit.
            inprocess_stdin_fd = Some(stdin_fd);
            stages.push(PipelineStage::InProcess { stdin_fd });
            // No pid: don't touch live_pids_arc / first_pid for this stage.
            restore_inline_assignments(snap, shell);
            continue;
        }

        // #144: InProcess stages get a NEUTRAL stdout/stderr base
        // (pipe/capture/inherit); the forked child re-applies the stage's own
        // `>file` / `2>file` / `2>&n` redirects in source order via run_command,
        // so `2>&1 >f` binds stderr to the pipe like bash. (External stages replay
        // their ChildRedirPlan; both now share the neutral-base model.) No pre-wired
        // `explicit_stdout`/`explicit_stderr` base fd and no dup-target
        // pre-resolution — the child is the single authoritative pass.
        //
        // For an EXTERNAL stage, lower the FULL ordered redirect list once (v292
        // machinery) and replay it in the child over the pipe/capture base. #69: set
        // current_lineno so a stage redirect-open error carries `line N:` like a
        // single command; the plan's opens route through redir_open_error.
        //
        // Ordering note: this runs before the inter-stage pipe / capture write end
        // is created below, but that ordering is now incidental. It used to be
        // load-bearing — `build_child_redir_plan` forked a heredoc writer, which
        // would inherit an already-created pipe write end and keep the downstream
        // stage from ever seeing EOF. Since #169 there is no fork:
        // `heredoc_body_to_fd` delivers the body in this process and returns a
        // single read-only fd. Nothing here depends on running first.
        let external_plan: Option<ChildRedirPlan> = if stage_is_external && !redirect_failed {
            if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
                // #69: stamp the stage's line so a redirect-open error carries
                // `line N:` (mirrors the single-command path at executor.rs:4550).
                if exec.line != 0 {
                    shell.current_lineno = shell.line_base() + exec.line;
                }
                match build_child_redir_plan(&exec.redirects, shell) {
                    Ok(p) => Some(p),
                    Err(_) => {
                        // #145: error already reported by build_child_redir_plan;
                        // fail ONLY this stage and fall through to the exit-1 child.
                        redirect_failed = true;
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // ---- Build stdout fd -------------------------------------------------
        // Priority: explicit redirect > inter-stage pipe > Capture sink pipe > STDOUT_FILENO.
        let stdout: ChildFd = if !is_last {
            // Create the inter-stage pipe.
            match crate::child_fd::make_pipe(false) {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    // w is owned by the child's stdout ChildFd (not parent_held).
                    unsafe { ChildFd::owned_raw(w) }
                }
                Err(e) => {
                    {
                        let mut err = err_writer();
                        crate::sh_error_to!(
                            shell,
                            &mut *err,
                            None,
                            "pipe: {}",
                            crate::bash_io_error(&e)
                        );
                    }
                    restore_inline_assignments(snap, shell);
                    // stdin / stderr drop here.
                    return Err(bail_teardown_pipeline(
                        mode,
                        shell,
                        procsub_base,
                        first_pid,
                        &stages,
                        &mut parent_held,
                    ));
                }
            }
        } else {
            // Stage 3 (#197): the last stage's stdout always inherits the real
            // fd 1 (the deleted `Capture` sink wired a capture pipe here).
            ChildFd::Inherit
        };

        // ---- Build stderr fd -------------------------------------------------
        // Stage 3 (#197): every stage's stderr inherits the real fd 2 unless an
        // explicit `2>file` / `2>&n` redirect was lowered into the child plan.
        // (The deleted `Merged` sink kernel-wired `2>&1` and `Capture` dup'd a
        // shared stderr pipe here.)
        let stderr: ChildFd = ChildFd::Inherit;

        // ---- Classify and spawn ----------------------------------------------
        let pgid_target = if group {
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
        let mut fds_to_close_in_child: Vec<RawFd> = parent_held
            .iter()
            .copied()
            .filter(|&fd| {
                Some(fd) != stdin.raw() && Some(fd) != stdout.raw() && Some(fd) != stderr.raw()
            })
            .collect();
        // The parent's shared stage0_default original (Background /dev/null) is
        // inherited by every child; close it there. Foreground's Inherit has no
        // raw fd, so this is a no-op there.
        if let Some(d) = stage0_default.raw() {
            fds_to_close_in_child.push(d);
        }

        // #144/#140d: no dup-target pre-resolution. The forked child re-applies
        // the FULL redirect list in source order — a `{v}>f` earlier in the stage
        // assigns `$v` before a later `2>&$v` reads it, so the child derives the
        // correct fd natively (no parent-side snapshot/restore, no spurious
        // `$v: ambiguous redirect`). Matches how stdin `<file` redirects and
        // compound stages already behave. (#147 will drop the now-unused
        // dup_target parameters from fork_and_run_in_subshell.)
        let (stdout_dup_target, stderr_dup_target): (Option<RawFd>, Option<RawFd>) = (None, None);

        // Build the child's fd environment ONCE; move it into whichever spawner.
        // Both spawners consume the ChildStdio and close the parent's owned
        // copies (on success AND error — RAII), so there is no post-spawn parent
        // close bookkeeping. The inter-stage pipe WRITE end is owned by `stdout`
        // (never entered parent_held); the READ end stays in parent_held for the
        // next stage to consume.
        let child_stdio = ChildStdio::new(stdin, stdout, stderr);
        let pid = if redirect_failed {
            // #145: the stage's own redirect setup failed (error already printed).
            // Fork a child wired to this stage's inter-stage pipe ends that exits
            // 1, so downstream reads EOF, upstream (if it fed this stage) gets
            // SIGPIPE, and $PIPESTATUS records 1 — matching bash's per-stage
            // failure. child_stdio is consumed HERE (not by the real spawners).
            match spawn_failed_stage(shell, child_stdio, pgid_target, &fds_to_close_in_child) {
                Ok(pid) => pid,
                Err(_) => {
                    // fork() failed => genuine infrastructure failure: abort.
                    restore_inline_assignments(snap, shell);
                    return Err(bail_teardown_pipeline(
                        mode,
                        shell,
                        procsub_base,
                        first_pid,
                        &stages,
                        &mut parent_held,
                    ));
                }
            }
        } else {
            let spawn_result = match kind {
                StageKind::External(simple) => spawn_external_with_fds(
                    simple,
                    shell,
                    child_stdio,
                    pgid_target,
                    &fds_to_close_in_child,
                    external_plan.expect("external stage always has a ChildRedirPlan"),
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
            match spawn_result {
                Ok(p) => p,
                Err(e) => {
                    {
                        let mut err = err_writer();
                        crate::sh_error_to!(shell, &mut *err, None, "{}", crate::bash_io_error(&e));
                    }
                    // child_stdio (with all owned stdio fds) was consumed by the
                    // failed call and already dropped. Close every remaining
                    // parent-held fd (inter-stage read ends + capture read ends);
                    // Background additionally kills+reaps the already-spawned stages.
                    restore_inline_assignments(snap, shell);
                    return Err(bail_teardown_pipeline(
                        mode,
                        shell,
                        procsub_base,
                        first_pid,
                        &stages,
                        &mut parent_held,
                    ));
                }
            }
        };

        // ---- Restore inline assignments (v23 scoping) -----------------------
        restore_inline_assignments(snap, shell);

        // (A heredoc/herestring body, if any, is written by the forked writer
        // process spawned above; the parent holds no write end here.)

        // ---- Track pid -------------------------------------------------------
        if (mode == SpawnMode::Background || interactive) && first_pid.is_none() {
            first_pid = Some(pid);
        }
        if mode == SpawnMode::Foreground {
            live_pids_arc.lock().unwrap().push(pid as libc::pid_t);
        }
        stages.push(PipelineStage::Forked(pid));
    }

    // Close any remaining parent-held fds that weren't consumed
    // (e.g., if the last stage had an explicit stdout redirect, prev_pipe_read
    // might still hold a stale value from a stage with a broken pipe — but that
    // shouldn't happen in a well-formed pipeline).
    // Preserve `inprocess_stdin_fd` (lastpipe's in-process last-stage stdin) —
    // `run_multi_stage` still needs it after this bulk-close, before it's dup2'd
    // onto fd 0 via `RedirectScope::redirect` and closed there. (Stage 3 (#197):
    // no capture read ends survive here anymore — stages inherit fd 1/2.)
    for fd in parent_held.iter().copied() {
        if Some(fd) != inprocess_stdin_fd {
            unsafe {
                libc::close(fd);
            }
        }
    }
    parent_held.retain(|&fd| Some(fd) == inprocess_stdin_fd);

    let pgid_target = if group {
        first_pid.unwrap_or(0)
    } else {
        NO_PGROUP
    };
    Ok(SpawnedPipeline {
        first_pid,
        stages,
        pgid_target,
        procsub_base,
    })
}

/// Foreground pipeline: spawn every stage via `spawn_pipeline`, then drain any
/// capture pipes, hand back the terminal, wait for all stages (setting
/// `$PIPESTATUS`), and drain process substitutions.
fn run_multi_stage(commands: &[Command], shell: &mut Shell) -> ExecOutcome {
    let interactive = shell.job_control_active();
    let SpawnedPipeline {
        first_pid,
        stages,
        pgid_target: _,
        procsub_base,
    } = match spawn_pipeline(commands, SpawnMode::Foreground, shell) {
        Ok(sp) => sp,
        Err(outcome) => return outcome,
    };
    // Reconstruct the per-stage pid list (same order as `stages`) for the wait
    // and the live-children clear. Only Forked stages have pids; an InProcess
    // stage (lastpipe) runs in the parent and is never waited/registered here.
    let stage_pids: Vec<i32> = stages
        .iter()
        .filter_map(|s| match s {
            PipelineStage::Forked(p) => Some(*p),
            PipelineStage::InProcess { .. } => None,
        })
        .collect();
    // Re-acquire the live-children registry handle to clear this pipeline's pids
    // after the wait (they were published per-stage inside spawn_pipeline).
    let live_pids_arc = shell.live_external_children.clone();

    // ---- lastpipe: run the in-process last stage BEFORE the wait ------------
    // Must run BEFORE `wait_pipeline_raw`: the forked upstream stages write
    // into pipes this in-process stage is the sole reader of, so running it
    // after the wait would deadlock (upstream blocks writing into a full pipe
    // with no reader). Under the gate that produces an InProcess stage
    // (`spawn_pipeline`'s `lastpipe`), `capture_read_fd`/`capture_err_read_fd`
    // are always None and `interactive` is always false (Terminal sink, job
    // control off), so this doesn't disturb the capture-drain loop or the
    // terminal-handoff/stopped handling below.
    let inproc: Option<ExecOutcome> =
        if let Some(PipelineStage::InProcess { stdin_fd }) = stages.last().copied() {
            let mut scope = RedirectScope::new();
            // Dup the resolved stdin fd onto fd 0 for the stage's duration.
            // Handles an already-closed fd 0 (`exec 0<&-`): `dup(0)` fails
            // EBADF, `RedirectScope` records the saved slot as -1, and Drop
            // closes fd 0 back to unopened afterward — see `redirect`'s doc.
            if scope.redirect(shell, stdin_fd, libc::STDIN_FILENO).is_ok() {
                let outcome = run_command(&commands[commands.len() - 1], shell);
                drop(scope); // restores (or re-closes) fd 0
                unsafe {
                    libc::close(stdin_fd);
                }
                Some(outcome)
            } else {
                unsafe {
                    libc::close(stdin_fd);
                }
                Some(ExecOutcome::Continue(1))
            }
        } else {
            None
        };
    let inproc_status = inproc.as_ref().map(outcome_status);

    // Stage 3 (#197): the last stage inherits the real fd 1/2, so there are no
    // capture pipes to drain here (the deleted `Capture` sink drained a
    // stdout/stderr pipe pair before the wait).

    // Give the terminal to the pipeline's process group if interactive.
    // #167: only when we own a controlling tty (set -m under a pipe skips the
    // handoff but still groups + waits on the pipeline's process group).
    if interactive
        && stdin_is_tty()
        && let Some(pgid) = first_pid
    {
        give_terminal_to(pgid);
    }

    // ---- Wait for all stages ------------------------------------------------
    let last_status = wait_pipeline_raw(
        &stages,
        &stage_pids,
        first_pid,
        shell,
        interactive,
        inproc_status,
    );

    // Clear this pipeline's stage pids from the live-children registry in one
    // pass. They were published per-stage above so the timeout timer thread
    // could SIGTERM every stage on a deadline fire.
    {
        let mut guard = live_pids_arc.lock().unwrap();
        guard.retain(|p| !stage_pids.iter().any(|s| (*s as libc::pid_t) == *p));
    }

    if interactive {
        if stdin_is_tty() {
            give_terminal_to(shell.shell_pgid);
        }
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

    // lastpipe: propagate the in-process last stage's control flow. $PIPESTATUS
    // is already set above (from `set_pipestatus`) regardless of which branch
    // this takes. A plain `Continue` falls through to the normal `status`
    // return below; anything else (`Exit`/`FunctionReturn`/`LoopBreak`/
    // `LoopContinue`/`Interrupted`) must propagate as-is — e.g. `exit 14` as
    // the last stage exits the shell, `return 42` from a function last stage
    // returns from the function.
    if let Some(o) = inproc
        && !matches!(o, ExecOutcome::Continue(_))
    {
        drain_procsubs(shell, procsub_base);
        return o;
    }

    // Drain any process substitutions realized during stage word expansion.
    // We drain here (after wait_pipeline_raw), not per-stage, because the
    // parent_fd must stay open until all stages that reference /dev/fd/N have
    // run.
    drain_procsubs(shell, procsub_base);
    ExecOutcome::Continue(status)
}

/// Maps an `ExecOutcome` to its plain exit-status integer, dropping
/// control-flow metadata — used to fill `PIPESTATUS`'s InProcess slot for the
/// lastpipe last stage. `run_multi_stage` separately propagates the full
/// `ExecOutcome` (not just this status) when it's not a plain `Continue`.
fn outcome_status(outcome: &ExecOutcome) -> i32 {
    match outcome {
        ExecOutcome::Continue(s)
        | ExecOutcome::Exit(s)
        | ExecOutcome::FunctionReturn(s)
        | ExecOutcome::LoopBreak(_, s) => *s,
        ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::Interrupted(_) => 130,
    }
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
    interactive: bool,
    inprocess_status: Option<i32>,
) -> PipelineWaitResult {
    // Status slots to fill: Forked stages via waitpid below; an InProcess slot
    // (lastpipe) is filled directly from `inprocess_status` instead.
    let mut stage_status: Vec<Option<i32>> = stages.iter().map(|_| None).collect();

    let pid_per_stage: Vec<Option<i32>> = stages
        .iter()
        .map(|s| match s {
            PipelineStage::Forked(pid) => Some(*pid),
            PipelineStage::InProcess { .. } => None,
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
                        let mut err = err_writer();
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
            let pid = match stage {
                PipelineStage::Forked(p) => p,
                PipelineStage::InProcess { .. } => {
                    *slot = inprocess_status;
                    continue;
                }
            };
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
                let mut err = err_writer();
                crate::sh_error_to!(shell, &mut *err, None, "{name}: readonly variable");
            }
            // #234/#203: a readonly-variable error in a command PREFIX assignment
            // is reported, but bash still RUNS the command with the OLD value
            // (rc = the command's exit) in default mode — it does NOT abort.
            // v358 (#198): measured — bash ABANDONS THE LIST in posix and
            // continues outright otherwise. It never exits here, unlike a
            // STANDALONE `r=2`, which does. The prefix is the distinction.
            if shell.shell_options.posix && !shell.is_interactive {
                shell.report_error(crate::error_fatality::ErrorKind::AssignmentPrefix);
                return Err(snap);
            }
            // Skip this failed assignment (leave the old value) and continue —
            // any remaining prefixes still apply and the command still runs.
            continue;
        }
        if apply_one_assignment(a, shell, &mut *err_writer()).is_err() {
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
                // bash: a bare scalar assignment to an array name targets element
                // [0] — `arr=v` == `arr[0]=v`, `arr+=v` == `arr[0]+=v` — for both
                // indexed AND associative arrays (v344 #327). huck already did
                // this for indexed arrays; mirror it here for associative (key
                // "0"). The integer attribute is honored inside assign().
                let val = crate::param_expansion::expand_word_to_string(&a.value, shell);
                if a.append {
                    return shell
                        .append_associative_element(name, "0", &val)
                        .map_err(|_| ());
                }
                return shell
                    .set_associative_element(name, "0".to_string(), val)
                    .map_err(|_| ());
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
            (AssignTarget::Indexed { name, subscript }, Some(_)) => {
                // #76: bash names the lvalue as WRITTEN, subscript included and
                // unexpanded (`a[i]`, `a[2+3]`, `a[$((1+1))]`), and says
                // `cannot assign list to array member` — the same wording for
                // an associative element as for an indexed one.
                let sub = crate::expand::reconstruct_word_source(subscript);
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "{name}[{sub}]: cannot assign list to array member"
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
            // Record the RHS this statement assigns so the `set -x` trace in
            // run_assignment_list can show `name+=rhs` / `name=rhs` (v339 #310).
            if shell.shell_options.xtrace {
                shell.set_xtrace_assign_rhs(Some(s.clone()));
            }
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
                    // v312 (#3/#49): if a `$(( ))` arith error in the subscript
                    // already raised the discard, the command is being unwound —
                    // suppress the secondary "bad array subscript" diagnostic
                    // (bash prints only the arith error, then discards).
                    if !shell.discard_pending() {
                        crate::sh_error_to!(shell, err, None, "{msg}");
                    }
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
        (AssignTarget::Indexed { name, subscript }, Some(_)) => {
            let sub = crate::expand::reconstruct_word_source(subscript);
            crate::sh_error_to!(
                shell,
                err,
                None,
                "{name}[{sub}]: cannot assign list to array member"
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
                if shell.fatal_pending() || shell.discard_pending() {
                    return Err(());
                }
            }
            None => {
                for field in glob_expand_word(&elem.value, shell, err)? {
                    map.insert(implicit, field);
                    implicit += 1;
                }
                if shell.fatal_pending() || shell.discard_pending() {
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

/// #167: true when the shell's stdin is a controlling terminal. Foreground job
/// paths gate `give_terminal_to`/`tcsetpgrp` on this so that `set -m` job
/// control under a pipe (no tty) still sets up process groups and waits on the
/// job's group, but never tries to hand terminal control to a job group when
/// there is no controlling tty. Interactive shells run on a tty, so their
/// terminal-handoff behavior is unchanged.
fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

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
/// Marks the shell as having blocked on a child (#418). Called wherever huck
/// waits, so the between-command pass knows a bash-equivalent reap point
/// happened and a pending job notice may be announced.
fn note_blocked_on_child(shell: &mut Shell) {
    shell.blocked_on_child = true;
}

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

/// Install a `ChildStdio` onto fds 0/1/2 inside a just-forked child, overlap-safe.
///
/// Returns the ORIGINAL raw source fd numbers (pre-move; `-1` for `Inherit`) so the
/// caller can exclude this child's own stdio sources when closing parent-held pipe
/// fds. async-signal-safe: `into_raw()` first (no `OwnedFd` Drop in the child), then
/// pure `fcntl`/`dup2`/`close`.
///
/// # Safety
/// Must be called only in the child after `fork()`, before any `OwnedFd` could drop.
unsafe fn install_child_stdio(stdio: ChildStdio) -> [RawFd; 3] {
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
    let original_raws: [RawFd; 3] = [
        plan[0].0.unwrap_or(-1),
        plan[1].0.unwrap_or(-1),
        plan[2].0.unwrap_or(-1),
    ];
    // Pass 1 (PRE-MOVE): move any owned source in 0..=2 up to >=3, so pass 2's
    // dup2 always has source != target (clears FD_CLOEXEC): the moved copy must
    // survive exec if its install no-ops.
    for (src, _) in plan.iter_mut() {
        if let Some(s) = *src
            && s <= 2
        {
            let moved = unsafe { libc::fcntl(s, libc::F_DUPFD, 3) };
            if moved >= 0 {
                unsafe { libc::close(s) };
                *src = Some(moved);
            }
        }
    }
    // Pass 2 (INSTALL): sources now all >=3 and pairwise distinct (except the
    // pathological case where a pass-1 F_DUPFD failed and left an owned source at
    // its own slot — a no-op dup2 we must NOT then close).
    for (src, slot) in plan {
        if let Some(s) = src {
            unsafe { libc::dup2(s, slot) };
            if s != slot {
                unsafe { libc::close(s) };
            }
        }
    }
    original_raws
}

/// Stage 1 (#197) SPIKE: run a command-substitution body by FORKING a real
/// subshell whose stdout is a pipe the parent drains — replacing the in-process
/// clone + `Capture` sink. The child writes to real fd 1 (the pipe), so an inner
/// `2>&1` is a real dup2 onto the pipe (fixes #353/#195 by construction). The
/// child is a transient foreground child in the shell's process group (not a
/// job, no `$!`). Returns (captured stdout, exit status).
// Used by `run_substitution` in non-test builds; the lib unit-test build routes
// comsub through the in-process capture instead (see `expand::capture_command_output`).
#[cfg_attr(test, allow(dead_code))]
pub fn capture_via_fork(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    use crate::child_fd::{ChildFd, ChildStdio};
    use std::io::Read;
    use std::os::fd::AsRawFd;

    let (read, write) = match crate::child_fd::make_pipe_owned(false) {
        Ok(p) => p,
        Err(e) => {
            crate::sh_error!(shell, None, "pipe: {}", crate::bash_io_error(&e));
            return (String::new(), 1);
        }
    };
    // The child needs the parent's read-end fd NUMBER for its close list (else
    // EOF never arrives). The parent keeps ownership of `read` (the OwnedFd).
    let read_raw = read.as_raw_fd();
    // The body runs in-process in the child via a BraceGroup (one fork total).
    let body = Command::BraceGroup(Box::new(seq.clone()));
    let stdio = ChildStdio::new(
        ChildFd::Inherit,     // stdin: child reads the shell's stdin
        ChildFd::from(write), // stdout: pipe write-end -> fd 1 (ownership -> child stdio)
        ChildFd::Inherit,     // stderr: terminal unless an inner 2>&1
    );
    // Child closes the parent's read end (else EOF never arrives).
    let pid =
        match fork_and_run_in_subshell(&body, shell, stdio, NO_PGROUP, &[read_raw], None, None) {
            Ok(pid) => pid,
            Err(e) => {
                // `write` was owned by the moved `ChildStdio` and is already
                // closed (RAII) by `fork_and_run_in_subshell` on its error
                // return; the parent-kept `read` OwnedFd drops here, closing the
                // read end — no manual close needed.
                crate::sh_error!(shell, None, "fork: {}", crate::bash_io_error(&e));
                return (String::new(), 1);
            }
        };
    // PARENT: `write` (the pipe write end) was owned by the moved `ChildStdio`
    // and is already closed by the fork helper's RAII drop in the parent, so the
    // read end below sees EOF when the child exits.
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut f = std::fs::File::from(read);
        let _ = f.read_to_end(&mut buf); // f drops -> closes the read end
    }
    let mut raw: libc::c_int = 0;
    unsafe {
        libc::waitpid(pid, &mut raw, 0);
    }
    let status = raw_status_to_exit_code(raw, shell);
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

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
    // #184: huck runs this subshell by forking WITHOUT exec — the child
    // continues in-process through `run_command`, which is memory-safe only in
    // a single-threaded process (see `exec_guard`). Panic loudly here rather
    // than let the forked child deadlock on a lock another thread holds.
    crate::exec_guard::assert_single_threaded_fork();
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // CHILD: async-signal-safe-ish operations only until we dive into
        // `run_command`. Safe because the single-threaded-execution invariant
        // (enforced by `exec_guard`, checked just above the fork) holds.
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
            // 3-5. Install stdio onto 0/1/2 (overlap-safe). Returns the original
            // raw source fds for the pass-3 close-exclusion below.
            let original_raws = install_child_stdio(stdio);
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
        // v324 (#257): under `set -T` (functrace), bash inherits DEBUG and
        // RETURN traps into a `( )` SUBSHELL (everything else still resets), so
        // the subshell's interior commands still fire DEBUG. Restrict this to a
        // real `Command::Subshell` fork — a PIPELINE STAGE also forks through
        // this helper, but bash already fired DEBUG for the stage parent-side
        // (spawn_pipeline), so preserving the trap here too would double-fire.
        // Signature of `clear_for_subshell` stays unchanged.
        let preserve_functrace =
            shell.shell_options.functrace && matches!(cmd, Command::Subshell { .. });
        let saved_debug = if preserve_functrace {
            shell.traps.get(&crate::traps::TrapSignal::Debug).cloned()
        } else {
            None
        };
        let saved_return = if preserve_functrace {
            shell.traps.get(&crate::traps::TrapSignal::Return).cloned()
        } else {
            None
        };
        crate::traps::clear_for_subshell(shell);
        if let Some(a) = saved_debug {
            shell.traps.insert(crate::traps::TrapSignal::Debug, a);
        }
        if let Some(a) = saved_return {
            shell.traps.insert(crate::traps::TrapSignal::Return, a);
        }
        // Mark this process as a forked subshell so its inner pipelines skip
        // interactive job-control process-grouping (deadlocks on a tty — M-104).
        shell.in_subshell = true;
        // 8. Run the body via the existing dispatcher.
        //    The child's stdout is now fd 1 (the dup2'd pipe end), so writes
        //    to the real fd table land at the right destination.
        // Anti-recursion guard: when a Command::Subshell is used as a
        // pipeline stage, the pipeline forks via this helper.  If we called
        // run_command here, it would fork AGAIN.  Instead, dispatch via
        // execute() so that body.background is honoured — `(cmd &)` inside a
        // subshell must background the inner command and let the subshell exit.
        // execute() calls execute_sequence_body when background is false
        // (the common case), preserving the single-fork invariant.
        let outcome = match cmd {
            Command::Subshell { body } => execute(body, shell, "(subshell)"),
            other => run_command(other, shell),
        };
        // 9. Translate outcome to an 8-bit exit status.
        let status: i32 = match outcome {
            ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
            ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
            ExecOutcome::FunctionReturn(n) => n,
            ExecOutcome::Interrupted(InterruptReason::Sigint) => 130,
            ExecOutcome::Interrupted(InterruptReason::Timeout) => 124,
            // #442: the body (or a trap it fired) asked to exit — that is this
            // CHILD's status.
            ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)) => n,
            // v312 (#3/#49): a discard reaching a subshell/child reducer decodes
            // to 1 (the driver normally handles it; defensive here).
            ExecOutcome::Interrupted(InterruptReason::DiscardCommand) => 1,
        };
        // #449: a child runs the EXIT trap it installed for ITSELF. The
        // inherited one was cleared by `clear_for_subshell` above, so anything
        // registered now belongs to this child. This is the only production
        // path where a forked child runs shell code, so it covers `( )`, `&`,
        // a pipeline stage and `$( )` alike.
        //
        // Fired BEFORE flush_stdout so the action's output reaches the
        // captured pipe (`x=$( trap "echo t" EXIT; echo b )` captures both
        // lines), and with the pending request CLEARED first — leaving it set
        // aborts the action at its first checkpoint, the same trap #442 hit at
        // the top-level exit path. The trap's own `exit N` then wins.
        let requested = shell.take_exit();
        crate::traps::fire_exit_trap(shell);
        let status = shell.take_exit().or(requested).unwrap_or(status);
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

/// Fork a "failed pipeline stage": a child that inherits this stage's stdio
/// (its inter-stage pipe ends), joins the job's process group, closes every
/// OTHER parent-held pipe fd, and immediately `_exit(1)`. It runs no command and
/// prints nothing — the redirect error was already reported by the parent. Its
/// exit closes the pipe ends it holds, giving downstream EOF and upstream
/// SIGPIPE, reproducing bash's per-stage redirect failure (#145). Unlike
/// `fork_and_run_in_subshell` it does NOT dup2 stdio to 0/1/2: the child never
/// reads or writes, it only has to hold its pipe ends open until exit.
fn spawn_failed_stage(
    shell: &mut Shell,
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    let _ = shell; // reserved for symmetry with the other spawners
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
            let ChildStdio {
                stdin,
                stdout,
                stderr,
            } = stdio;
            // Convert to raw so no OwnedFd Drop runs in the forked child; keep
            // the fds OPEN (they close on _exit -> the pipe topology reacts).
            let own: [RawFd; 3] = [
                stdin.into_raw().unwrap_or(-1),
                stdout.into_raw().unwrap_or(-1),
                stderr.into_raw().unwrap_or(-1),
            ];
            for &fd in parent_fds_to_close {
                if fd != own[0] && fd != own[1] && fd != own[2] {
                    libc::close(fd);
                }
            }
            libc::_exit(1);
        }
    }
    if pgid_target >= 0 {
        unsafe {
            libc::setpgid(pid, pgid_target);
        }
    }
    Ok(pid)
}

/// #78: fork a stand-in child for an external stage whose program can't be run.
/// It installs the stage's stdio + replays the stage's redirect plan (so fd 2
/// points wherever `2>&1`/`2>file`/the pipe put it), writes the bash-formatted
/// `diag` to fd 2, and `_exit`s `exit_code` (127 not-found / 126 not-executable).
/// This mirrors bash's child-side diagnostic and lets the pipeline continue with
/// a populated PIPESTATUS. `held` (the plan's opened redirect-target fds) is
/// inherited by the child and dropped in the parent after fork.
fn spawn_command_error_stage(
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    replay_ops: Vec<ChildRedirOp>,
    held: Vec<std::os::fd::OwnedFd>,
    diag: Vec<u8>,
    exit_code: i32,
) -> Result<i32, io::Error> {
    flush_stdout();
    // Compute the replay targets BEFORE fork so the child branch is strictly
    // alloc-free (async-signal-safety hygiene; matches the runnable path).
    let extra_targets: Vec<RawFd> = replay_ops
        .iter()
        .map(|op| match *op {
            ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target,
        })
        .collect();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
            // Install stdio onto 0/1/2 (overlap-safe); returns original sources
            // for the parent-fd close-exclusion.
            let original_raws = install_child_stdio(stdio);
            // Replay the stage's redirects (2>&1 / 2>file / fd>2 / close) in
            // source order, AFTER stdio install so 2>&1 sees the piped fd 1.
            let _ = replay_redir_ops(&replay_ops);
            // Close parent-held pipe fds except our own stdio sources and the
            // replay targets (extra_targets, computed pre-fork above).
            for &fd in parent_fds_to_close {
                if fd != original_raws[0]
                    && fd != original_raws[1]
                    && fd != original_raws[2]
                    && !extra_targets.contains(&fd)
                {
                    libc::close(fd);
                }
            }
            // Write the diagnostic to fd 2 (now redirected as the stage asked).
            if !diag.is_empty() {
                libc::write(2, diag.as_ptr() as *const libc::c_void, diag.len());
            }
            libc::_exit(exit_code);
        }
    }
    // PARENT: the child inherited its own copies of `held`; drop ours.
    drop(held);
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

/// Whether an external command's program can be run, and if not, the bash
/// diagnostic body + exit code (127 not-found, 126 found-but-not-executable).
#[derive(Debug)]
enum StageRunnability {
    Runnable,
    NotRunnable { body: String, code: i32 },
}

/// Classify a resolved program string the way bash's command search + `execve`
/// would, so an unrunnable command becomes a 126/127 diagnostic (the pipeline
/// path forks a diagnostic child, #78; the single-command path emits from a
/// `pre_exec` that `_exit`s, #172). Shared by both paths.
fn classify_command_runnability(program: &str, shell: &Shell) -> StageRunnability {
    if program.contains('/') {
        match std::fs::metadata(program) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StageRunnability::NotRunnable {
                body: format!("{program}: No such file or directory"),
                code: 127,
            },
            Err(e) => StageRunnability::NotRunnable {
                body: format!("{program}: {}", crate::bash_io_error(&e)),
                code: 126,
            },
            Ok(md) if md.is_dir() => StageRunnability::NotRunnable {
                body: format!("{program}: Is a directory"),
                code: 126,
            },
            Ok(_) => {
                // Executable bit? Use a real access(X_OK) so the errno text matches libc.
                let c = std::ffi::CString::new(program).unwrap_or_default();
                if unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0 {
                    StageRunnability::Runnable
                } else {
                    let e = std::io::Error::last_os_error();
                    StageRunnability::NotRunnable {
                        body: format!("{program}: {}", crate::bash_io_error(&e)),
                        code: 126,
                    }
                }
            }
        }
    } else {
        // Bare name: walk PATH. A non-executable regular file found in PATH is
        // 126 "Permission denied" (reported with the resolved path, matching
        // bash's first-match-in-PATH-order); nothing found is 127 (#172).
        match builtins::classify_path_search(program, shell) {
            builtins::PathClassify::Executable => StageRunnability::Runnable,
            builtins::PathClassify::NonExecutable(p) => StageRunnability::NotRunnable {
                body: format!("{}: Permission denied", p.display()),
                code: 126,
            },
            builtins::PathClassify::NotFound => StageRunnability::NotRunnable {
                body: format!("{program}: command not found"),
                code: 127,
            },
        }
    }
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
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    plan: ChildRedirPlan,
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
    // A zero-field program in a pipeline stage (#62 `Ok(None)`) keeps the prior
    // behavior here: an empty program flows to the NotRunnable diagnostic below.
    let resolved = resolve(exec, shell, &mut *err_writer())
        .map_err(|code| io::Error::other(format!("resolve failed with code {code}")))?
        .unwrap_or(ResolvedCommand {
            program: String::new(),
            args: Vec::new(),
            decl_args: None,
        });

    // #78: if the program can't be run, don't spawn — fork a diagnostic child
    // that prints `<name>: <reason>` to the stage's own (redirected) fd 2 and
    // exits 126/127, so the message routes correctly and PIPESTATUS is populated
    // (matching bash) instead of leaking a raw error and aborting the pipeline.
    if let StageRunnability::NotRunnable { body, code } =
        classify_command_runnability(&resolved.program, shell)
    {
        let mut diag: Vec<u8> = Vec::new();
        crate::emit_error_to(shell, &mut diag, None, format_args!("{body}"));
        return spawn_command_error_stage(
            stdio,
            pgid_target,
            parent_fds_to_close,
            plan.ops,
            plan.held,
            diag,
            code,
        );
    }

    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(
            xtrace_target_fd(shell),
            &format!(
                "{p4}{}",
                xtrace_command_line(&[], &resolved.program, &resolved.args)
            ),
        );
    }

    // External pipeline stages replay their full ordered ChildRedirPlan; no slot
    // dup-targets, no extra_ops.
    let stdout_dup_target: Option<i32> = None;
    let stderr_dup_target: Option<i32> = None;
    let replay_ops: Vec<ChildRedirOp> = plan.ops;
    let held: Vec<std::os::fd::OwnedFd> = plan.held;

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

    // Collect the target fds that replay_ops will set up in the child, so we
    // can exclude them from fds_to_close below (Fix B: a dup2 then close on
    // the same fd would silently defeat the redirect).
    let extra_targets: Vec<RawFd> = replay_ops
        .iter()
        .map(|op| match *op {
            ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target,
        })
        .collect();

    // Replay the extra (fd>2 / dup-in / close / ReadWrite) ops in source order,
    // AFTER the bridge stdio + dup-target pre_execs above. Pure dup2/close, so
    // async-signal-safe. Runs even when the bridge dup pre_exec is absent.
    if !replay_ops.is_empty() {
        let ops = replay_ops;
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
    // Exclude any fd that replay_ops already claimed as a redirect target: a
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
    drop(held);
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

#[cfg(test)]
mod heredoc_body_tests;
