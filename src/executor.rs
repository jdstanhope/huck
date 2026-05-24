use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{
    CaseClause, CaseItem, CaseTerminator, Command, Connector, ExecCommand, ForClause, IfClause,
    Pipeline, Redirect, Sequence, SimpleCommand, WhileClause,
};
use crate::expand::{expand, expand_assignment, expand_pattern, glob_expand_fields};
use crate::shell_state::Shell;

/// Where the terminal stage of a top-level pipeline sends its stdout when
/// there's no explicit `> file` redirect.
pub enum StdoutSink<'a> {
    Terminal,
    Capture(&'a mut Vec<u8>),
}

pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    // Backgrounding a compound command (if/while) is not supported;
    // fall through and run it synchronously.
    if seq.background
        && let Command::Pipeline(p) = &seq.first
    {
        // Parser guarantees rest.is_empty() when background is set.
        return run_background_sequence(p, shell, &mut sink, source);
    }
    execute_sequence_body(seq, shell, &mut sink)
}

/// Runs a sequence with stdout captured to a buffer. Used by command
/// substitution; the trailing `&` is ignored here because substitutions
/// must complete before their output is interpolated. Spawning real
/// background children whose pids the parent's JobTable doesn't track
/// would let them escape `wait`/`jobs` and litter the terminal.
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = {
        let mut sink = StdoutSink::Capture(&mut buf);
        execute_sequence_body(seq, shell, &mut sink)
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
        ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
        ExecOutcome::FunctionReturn(n) => n,
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

fn execute_sequence_body(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let mut status = run_command(&seq.first, shell, sink);
    if matches!(
        status,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
            | ExecOutcome::FunctionReturn(_)
    ) {
        return status;
    }
    for (connector, command) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_command(command, shell, sink);
            if matches!(
                status,
                ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_)
            ) {
                return status;
            }
        }
    }
    status
}

/// Dispatches a single sequence element.
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
        Command::For(clause) => run_for(clause, shell, sink),
        Command::Case(clause) => run_case(clause, shell, sink),
        Command::BraceGroup(seq) => execute_sequence_body(seq, shell, sink),
        Command::FunctionDef { name, body } => {
            shell.functions.insert(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
    }
}

/// Runs a `while`/`until` loop. The body runs while the condition's
/// exit status satisfies the loop's polarity. `break` ends the loop;
/// `continue` jumps to the next condition test; `exit` propagates; a
/// pending SIGINT (Ctrl-C) ends the loop with status 130.
fn run_while(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
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
        let cond = execute_sequence_body(&clause.condition, shell, sink);
        let keep_going = match cond {
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
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
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through — the loop re-tests the condition
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
    use std::sync::atomic::Ordering;

    // Expand the word list once — the same path command arguments take.
    let mut values: Vec<String> = Vec::new();
    for word in &clause.words {
        values.extend(glob_expand_fields(expand(word, shell)));
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
        shell.set(&clause.var, value);
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through — advance to the next value
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }
    }
    last
}

/// Matches `subject` against a `case` clause's `|`-patterns. A clause
/// matches if any pattern matches; an unparseable glob matches nothing.
fn case_item_matches(item: &CaseItem, subject: &str, shell: &mut Shell) -> bool {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    for pattern_word in &item.patterns {
        let pattern = expand_pattern(pattern_word, shell);
        if let Ok(p) = glob::Pattern::new(&pattern)
            && p.matches_with(subject, opts)
        {
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
        if !run_this {
            i += 1;
            continue;
        }
        match &item.body {
            None => last = ExecOutcome::Continue(0),
            Some(body) => match execute_sequence_body(body, shell, sink) {
                ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
                ExecOutcome::LoopBreak => return ExecOutcome::LoopBreak,
                ExecOutcome::LoopContinue => return ExecOutcome::LoopContinue,
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
    let cond = execute_sequence_body(&clause.condition, shell, sink);
    if matches!(
        cond,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
            | ExecOutcome::FunctionReturn(_)
    ) {
        return cond;
    }
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink);
    }
    for elif in &clause.elif_branches {
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink);
        if matches!(
            elif_cond,
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
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

fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0], shell, sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink)
    }
}

// ----- background pipeline --------------------------------------------------

fn run_background_sequence(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);

    if pipeline_is_pure_builtin(pipeline) {
        // Run synchronously in the parent shell. Side effects (cd, exports,
        // exit) take effect on the parent — documented divergence from bash,
        // which would fork a subshell.
        let outcome = run_pipeline(pipeline, shell, sink);
        if matches!(outcome, ExecOutcome::Exit(_)) {
            return outcome;
        }
        let exit = match outcome {
            ExecOutcome::Continue(c) => c,
            ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
            ExecOutcome::FunctionReturn(n) => n,
            ExecOutcome::Exit(_) => unreachable!(),
        };
        shell.jobs.add_synthetic_done(display, exit);
        // Route through the normal notification path so the line is formatted
        // with notification_line (includes `Exit N` for non-zero exits, the
        // command text, and the trailing `&`) and marked notified — preventing
        // the duplicate "[N] Done" line that would otherwise fire at the next
        // reap_and_notify pass.
        crate::jobs::reap_and_notify(shell);
        return ExecOutcome::Continue(0);
    }

    // Spawn each stage with process_group. The first stage gets
    // process_group(0) to become its own pg leader; subsequent stages join
    // that pg via process_group(first_pid). The first stage's stdin
    // defaults to /dev/null (so background commands don't fight the shell
    // for the terminal); explicit `< file` redirects override this.
    let n = pipeline.commands.len();
    let mut all_resolved: Vec<Option<ResolvedCommand>> = Vec::with_capacity(n);
    for cmd in &pipeline.commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                all_resolved.push(None);
            }
            SimpleCommand::Exec(exec) => match resolve(exec, shell) {
                Ok(r) => all_resolved.push(Some(r)),
                Err(code) => {
                    // Failed to expand; print the [N] line for the failed
                    // job so the user can see what happened, and bail.
                    return ExecOutcome::Continue(code);
                }
            },
        }
    }

    let mut spawned_pids: Vec<i32> = Vec::with_capacity(n);
    let mut first_pid: Option<i32> = None;
    let mut carry: Option<ChildStdout> = None;
    let mut children: Vec<Child> = Vec::with_capacity(n);

    for (i, resolved) in all_resolved.iter().enumerate() {
        let is_last = i == n - 1;
        let Some(cmd) = resolved else {
            // Assign stage in a background pipeline: no-op stage. The carry
            // input from the previous stage is dropped; the next stage will
            // get an empty pipe (Stdio::null instead of stdin from prev).
            carry = None;
            continue;
        };

        let files = match open_stage_files(cmd) {
            Ok(f) => f,
            Err(()) => {
                cleanup_partial_pipeline(first_pid, children);
                return ExecOutcome::Continue(1);
            }
        };

        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);
        process.env_clear();
        process.envs(shell.exported_env());

        // Reset job-control signals to SIG_DFL in the child before exec.
        use std::os::unix::process::CommandExt;
        unsafe { process.pre_exec(reset_job_control_signals_in_child); }

        // Process-group: first stage = own pg leader; rest join.
        let pgid_target = first_pid.unwrap_or(0);
        process.process_group(pgid_target);

        // Stdin: explicit redirect wins; otherwise carry from prev stage if
        // any; otherwise /dev/null for the first stage.
        if let Some(file) = files.stdin {
            process.stdin(Stdio::from(file));
        } else if let Some(child_stdout) = carry.take() {
            process.stdin(Stdio::from(child_stdout));
        } else {
            process.stdin(Stdio::null());
        }

        // Stdout: explicit redirect wins; otherwise pipe onward if not last;
        // otherwise inherit terminal.
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if !is_last {
            process.stdout(Stdio::piped());
        }

        if let Some(file) = files.stderr {
            process.stderr(Stdio::from(file));
        }

        let mut child = match process.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("huck: command not found: {}", cmd.program);
                cleanup_partial_pipeline(first_pid, children);
                return ExecOutcome::Continue(127);
            }
            Err(e) => {
                eprintln!("huck: {}: {e}", cmd.program);
                cleanup_partial_pipeline(first_pid, children);
                return ExecOutcome::Continue(1);
            }
        };

        let pid = child.id() as i32;
        spawned_pids.push(pid);
        if first_pid.is_none() {
            first_pid = Some(pid);
            // Close the setpgid race: Rust's `process_group` only sets the
            // pg in the child (pre-exec), so subsequent stages may try to
            // join `pid`'s group before the child has run setpgid. The
            // standard fix is to also call setpgid in the parent — it's
            // idempotent with the child's call.
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

        if !is_last {
            carry = child.stdout.take();
        }

        children.push(child);
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

    // Forget the Child structs so the OS doesn't try to reap them as
    // zombies via Drop — we own reaping via waitpid.
    for child in children {
        std::mem::forget(child);
    }

    let last_pid = *spawned_pids.last().unwrap();
    let id = shell.jobs.add(pgid, spawned_pids, display);
    eprintln!("[{id}] {last_pid}");
    ExecOutcome::Continue(0)
}

/// Cleans up children spawned during a background pipeline before it could be
/// fully started. Signals the whole process group (catching any double-forked
/// grandchildren), then reaps each child so we don't leave zombies.
fn cleanup_partial_pipeline(pgid: Option<i32>, children: Vec<Child>) {
    if let Some(pg) = pgid {
        unsafe {
            libc::killpg(pg, libc::SIGKILL);
        }
    }
    for mut c in children {
        let _ = c.wait();
    }
}

/// True iff every stage in the pipeline is a builtin (or an Assign).
fn pipeline_is_pure_builtin(pipeline: &Pipeline) -> bool {
    pipeline.commands.iter().all(|cmd| match cmd {
        SimpleCommand::Exec(e) => match e.program.0.first() {
            Some(crate::lexer::WordPart::Literal { text: name, .. }) => builtins::is_builtin(name),
            _ => false,
        },
        SimpleCommand::Assign { .. } => true,
    })
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

// ----- resolved command (post-expansion) ------------------------------------

struct ResolvedCommand {
    program: String,
    args: Vec<String>,
    stdin: Option<String>,
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

fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
    if prog_fields.is_empty() {
        eprintln!("huck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();
    for word in &cmd.args {
        args.extend(glob_expand_fields(expand(word, shell)));
    }
    let stdin = match &cmd.stdin {
        Some(word) => Some(expand_single(word, shell).map_err(|()| 1)?),
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(Redirect::Truncate(w)) => {
            Some(ResolvedRedirect::Truncate(expand_single(w, shell).map_err(|()| 1)?))
        }
        Some(Redirect::Append(w)) => {
            Some(ResolvedRedirect::Append(expand_single(w, shell).map_err(|()| 1)?))
        }
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(Redirect::Truncate(w)) => {
            Some(ResolvedRedirect::Truncate(expand_single(w, shell).map_err(|()| 1)?))
        }
        Some(Redirect::Append(w)) => {
            Some(ResolvedRedirect::Append(expand_single(w, shell).map_err(|()| 1)?))
        }
        None => None,
    };
    Ok(ResolvedCommand { program, args, stdin, stdout, stderr })
}

// ----- redirect file handling -----------------------------------------------

struct StageFiles {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

fn open_stage_files(cmd: &ResolvedCommand) -> Result<StageFiles, ()> {
    let stdin = match &cmd.stdin {
        Some(path) => match File::open(path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("huck: {path}: {e}");
                return Err(());
            }
        },
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

fn run_single(cmd: &SimpleCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell, sink),
        SimpleCommand::Assign { name, value } => {
            let v = expand_assignment(value, shell);
            shell.set(name, v);
            ExecOutcome::Continue(0)
        }
    }
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
/// converts to a normal `Continue(n)`; `Exit`/`LoopBreak`/`LoopContinue`
/// propagate unchanged so `break` inside a function targets the
/// caller's enclosing loop (matching bash).
fn call_function(
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    let result = run_command(&body, shell, sink);
    shell.positional_args = saved;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };

    // 1. Control builtins always win — they cannot be shadowed by functions.
    // 2. User-defined function lookup.
    // 3. Regular builtin.
    // 4. PATH-exec.
    if is_control_builtin(&resolved.program) {
        let files = match open_stage_files(&resolved) {
            Ok(f) => f,
            Err(()) => return ExecOutcome::Continue(1),
        };
        match files.stdout {
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
        }
    } else if let Some(body) = shell.functions.get(&resolved.program).cloned() {
        call_function(body, resolved.args, shell, sink)
    } else if builtins::is_builtin(&resolved.program) {
        let files = match open_stage_files(&resolved) {
            Ok(f) => f,
            Err(()) => return ExecOutcome::Continue(1),
        };
        match files.stdout {
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
        }
    } else {
        let files = match open_stage_files(&resolved) {
            Ok(f) => f,
            Err(()) => return ExecOutcome::Continue(1),
        };
        run_subprocess(&resolved, files, shell, sink)
    }
}

fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &mut Shell,
    sink: &mut StdoutSink,
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

    if interactive {
        process.process_group(0);
    }

    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    let want_capture = matches!(sink, StdoutSink::Capture(_));
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    } else if want_capture {
        process.stdout(Stdio::piped());
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    match process.spawn() {
        Ok(mut child) => {
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

enum Carry {
    None,
    ChildStdout(ChildStdout),
    Buffer(Vec<u8>),
}

enum Stage {
    Done(i32),
    Process(Child),
}

fn run_multi_stage(
    commands: &[SimpleCommand],
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let interactive = matches!(sink, StdoutSink::Terminal);
    let mut first_pid: Option<i32> = None;

    let mut resolved_stages: Vec<Option<ResolvedCommand>> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                resolved_stages.push(None);
            }
            SimpleCommand::Exec(exec) => match resolve(exec, shell) {
                Ok(r) => resolved_stages.push(Some(r)),
                Err(code) => return ExecOutcome::Continue(code),
            },
        }
    }
    let mut all_files: Vec<Option<StageFiles>> = Vec::with_capacity(resolved_stages.len());
    for r in &resolved_stages {
        match r {
            None => all_files.push(None),
            Some(r) => match open_stage_files(r) {
                Ok(f) => all_files.push(Some(f)),
                Err(()) => return ExecOutcome::Continue(1),
            },
        }
    }

    let n = resolved_stages.len();
    let mut stages: Vec<Stage> = Vec::with_capacity(n);
    let mut stage_pids: Vec<i32> = Vec::with_capacity(n);
    let mut carry = Carry::None;

    for (i, (resolved, files)) in resolved_stages.iter().zip(all_files).enumerate() {
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

        let cmd = match resolved {
            Some(r) => r,
            None => {
                drop(incoming);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                let _ = files;
                continue;
            }
        };
        let files = files.expect("non-Assign stage must have StageFiles");

        if builtins::is_builtin(&cmd.program) {
            drop(incoming);

            if cmd.program == "cd" || cmd.program == "exit" {
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                continue;
            }

            let mut buffer: Vec<u8> = Vec::new();
            let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer, shell);
            let mut status = match outcome {
                ExecOutcome::Continue(code) => code,
                ExecOutcome::Exit(code) => code,
                ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
                ExecOutcome::FunctionReturn(n) => n,
            };
            match files.stdout {
                Some(mut file) => {
                    if let Err(e) = file.write_all(&buffer) {
                        eprintln!("huck: {}: {e}", cmd.program);
                        status = 1;
                    }
                    if !is_last {
                        carry = Carry::Buffer(Vec::new());
                    }
                }
                None => {
                    if is_last {
                        match sink {
                            StdoutSink::Terminal => {
                                if let Err(e) = io::stdout().write_all(&buffer) {
                                    eprintln!("huck: {}: {e}", cmd.program);
                                    status = 1;
                                }
                            }
                            StdoutSink::Capture(buf) => {
                                buf.extend_from_slice(&buffer);
                            }
                        }
                    } else {
                        carry = Carry::Buffer(buffer);
                    }
                }
            }
            stages.push(Stage::Done(status));
            continue;
        }

        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);
        process.env_clear();
        process.envs(shell.exported_env());

        // Reset job-control signals to SIG_DFL in every child.
        use std::os::unix::process::CommandExt;
        unsafe { process.pre_exec(reset_job_control_signals_in_child); }
        if interactive {
            let pgid_target = first_pid.unwrap_or(0);
            process.process_group(pgid_target);
        }

        let mut pending_input: Option<Vec<u8>> = None;
        if let Some(file) = files.stdin {
            process.stdin(Stdio::from(file));
        } else {
            match incoming {
                Carry::None => {}
                Carry::ChildStdout(child_stdout) => {
                    process.stdin(Stdio::from(child_stdout));
                }
                Carry::Buffer(bytes) => {
                    process.stdin(Stdio::piped());
                    pending_input = Some(bytes);
                }
            }
        }

        let pipe_onward = !is_last && cmd.stdout.is_none();
        let want_terminal_capture =
            is_last && cmd.stdout.is_none() && matches!(sink, StdoutSink::Capture(_));
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward || want_terminal_capture {
            process.stdout(Stdio::piped());
        }

        if let Some(file) = files.stderr {
            process.stderr(Stdio::from(file));
        }

        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("huck: command not found: {}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(127));
                continue;
            }
            Err(e) => {
                eprintln!("huck: {}: {e}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(1));
                continue;
            }
        };

        if let Some(bytes) = pending_input
            && let Some(mut child_stdin) = child.stdin.take()
        {
            let _ = child_stdin.write_all(&bytes);
        }

        // Track pid for interactive job-control; setpgid in parent to
        // close the race with the child's setpgid (via process_group).
        let pid = child.id() as i32;
        stage_pids.push(pid);
        if interactive && first_pid.is_none() {
            first_pid = Some(pid);
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

        if pipe_onward {
            carry = Carry::ChildStdout(child.stdout.take().expect("stdout was set to piped"));
        } else if !is_last {
            carry = Carry::Buffer(Vec::new());
        } else if want_terminal_capture
            && let StdoutSink::Capture(buf) = sink
            && let Some(mut child_stdout) = child.stdout.take()
            && let Err(e) = io::copy(&mut child_stdout, *buf)
        {
            eprintln!("huck: {}: {e}", cmd.program);
        }

        stages.push(Stage::Process(child));
    }

    // Give the terminal to the pipeline's process group if interactive.
    if interactive
        && let Some(pgid) = first_pid
    {
        give_terminal_to(pgid);
    }

    let last_status = if interactive {
        match wait_pgrp_pipeline(stages, &stage_pids, first_pid, shell) {
            PipelineWait::AllExited(status) => {
                give_terminal_to(shell.shell_pgid);
                status
            }
            PipelineWait::Stopped(sig) => {
                give_terminal_to(shell.shell_pgid);
                return ExecOutcome::Continue(128 + sig);
            }
        }
    } else {
        let mut status = 0;
        for stage in stages {
            match stage {
                Stage::Done(code) => status = code,
                Stage::Process(mut child) => {
                    status = match child.wait() {
                        Ok(s) => status_code(&s),
                        Err(e) => {
                            eprintln!("huck: {e}");
                            1
                        }
                    };
                }
            }
        }
        status
    };

    ExecOutcome::Continue(last_status)
}

enum PipelineWait {
    /// Every process stage was reaped; pipeline exit status follows POSIX
    /// (the last stage's status).
    AllExited(i32),
    /// At least one stage was stopped (Ctrl-Z); the job has already been
    /// added to the table and the stop notification emitted.
    Stopped(i32),
}

/// Waits on the entire process group for an interactive pipeline. Uses
/// `waitpid(-pgid, …, WUNTRACED)` so a stop event on ANY stage surfaces
/// immediately — fixing the wedge that per-pid waiting created when the
/// loop was blocked on one stage while a sibling stopped. On a stop, the
/// pipeline is registered as a Stopped job (so `fg` can resume it) and
/// `Stopped(sig)` is returned. Consumes the `Stage::Process` children via
/// `mem::forget`, since reaping has already been done via `libc::waitpid`.
fn wait_pgrp_pipeline(
    stages: Vec<Stage>,
    stage_pids: &[i32],
    first_pid: Option<i32>,
    shell: &mut Shell,
) -> PipelineWait {
    let pid_per_stage: Vec<Option<i32>> = stages
        .iter()
        .map(|s| match s {
            Stage::Done(_) => None,
            Stage::Process(child) => Some(child.id() as i32),
        })
        .collect();
    let mut stage_status: Vec<Option<i32>> = stages
        .iter()
        .map(|s| match s {
            Stage::Done(code) => Some(*code),
            Stage::Process(_) => None,
        })
        .collect();
    let mut remaining: std::collections::HashSet<i32> =
        pid_per_stage.iter().filter_map(|p| *p).collect();

    if let Some(pgid) = first_pid {
        while !remaining.is_empty() {
            let mut raw: libc::c_int = 0;
            let r = loop {
                let r = unsafe { libc::waitpid(-pgid, &mut raw, libc::WUNTRACED) };
                if r < 0 {
                    let errno = std::io::Error::last_os_error()
                        .raw_os_error()
                        .unwrap_or(0);
                    if errno == libc::EINTR {
                        continue;
                    }
                }
                break r;
            };
            if r < 0 {
                // ECHILD — none of our pgrp children remain visible. Fill
                // unfilled slots with status 1 so we still produce a
                // deterministic result rather than hanging.
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
                eprintln!("\n{line}");
                forget_process_children(stages);
                return PipelineWait::Stopped(sig);
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

    forget_process_children(stages);
    PipelineWait::AllExited(stage_status.last().copied().flatten().unwrap_or(0))
}

/// Consumes `stages` and `mem::forget`s every `Stage::Process` child so
/// Drop doesn't try to re-reap pids we've already collected via
/// `libc::waitpid`. `Stage::Done` variants drop normally (no resources).
fn forget_process_children(stages: Vec<Stage>) {
    for stage in stages {
        if let Stage::Process(child) = stage {
            std::mem::forget(child);
        }
    }
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
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("echo"),
                    args: vec![ww(word)],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
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
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("test"),
                    args: vec![ww(lhs), ww("-eq"), ww("0")],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        }
    }

    fn lit_word(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    fn exec(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            program: lit_word(program),
            args: args.iter().map(|a| lit_word(a)).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline { commands: vec![cmd] }),
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
                commands: vec![exec("echo", &["first"]), exec("echo", &["second"])],
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
    fn background_pure_builtin_runs_synchronously_and_notifies_done() {
        // The synthetic Done job is added then immediately notified via the
        // normal reap_and_notify path (which formats via notification_line and
        // marks notified), so remove_notified drops it on the same sweep —
        // leaving the table empty. This prevents the duplicate `[N] Done` line
        // that would otherwise fire when the next prompt calls reap_and_notify.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![exec("echo", &["hi"])],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let outcome = execute(&seq, &mut shell, "echo hi &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn background_pure_builtin_nonzero_exit_runs_and_notifies() {
        // `test -z hi` returns 1 (non-empty). The notification path must surface
        // `Exit 1` (via render_state) rather than `Done`.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![exec("test", &["-z", "hi"])],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let outcome = execute(&seq, &mut shell, "test -z hi &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
        // Format coverage is in jobs::tests::notification_line_for_nonzero_exit.
    }

    #[test]
    fn execute_capturing_ignores_background_flag_runs_synchronously() {
        // `$(cmd &)` must wait and capture, not spawn an escaped bg job.
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![exec("echo", &["captured"])],
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
    fn background_pure_builtin_assignment_runs_in_parent() {
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Assign {
                    name: "HUCK_TEST_BG_ASSIGN".to_string(),
                    value: lit_word("v"),
                }],
            }),
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let _ = execute(&seq, &mut shell, "HUCK_TEST_BG_ASSIGN=v &");
        // The assignment ran in the parent (pure-builtin path).
        assert_eq!(shell.get("HUCK_TEST_BG_ASSIGN"), Some("v"));
    }

    #[test]
    fn give_terminal_to_silently_succeeds_on_non_tty() {
        // cargo test runs without a controlling terminal; tcsetpgrp returns
        // ENOTTY. The helper must swallow it.
        give_terminal_to(1); // bogus pgid; we only care that we don't panic
    }

    #[test]
    fn stray_break_at_top_level_is_harmless() {
        // `break` with no enclosing loop: the sequence stops, status 0.
        use crate::command::{ExecCommand, Pipeline};
        use crate::lexer::{Word, WordPart};
        let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
        let seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("break"),
                    args: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
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
                commands: vec![SimpleCommand::Assign {
                    name: "BG_X".to_string(),
                    value: Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }]),
                }],
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
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: vec![Word(vec![WordPart::Var { name: var.to_string(), quoted: false }])],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        }
    }

    /// A one-pipeline Sequence running the `continue` builtin.
    fn continue_seq() -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: Word(vec![WordPart::Literal { text: "continue".to_string(), quoted: false }]),
                    args: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
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
            body: echo_seq("hi"),
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
        assert_eq!(out.trim(), "");
        assert_eq!(status, 0);
    }

    #[test]
    fn for_variable_holds_last_value_after_loop() {
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
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
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                args: vec![Word(vec![WordPart::Literal { text: "NOPE".to_string(), quoted: false }])],
                stdin: None,
                stdout: None,
                stderr: None,
            })],
        });
        let mut body = continue_seq();
        body.rest.push((crate::command::Connector::Semi, echo_nope));
        let clause = ForClause {
            var: "x".to_string(),
            words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
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
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("break"),
                    args: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
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
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }]),
                    args: vec![Word(vec![WordPart::Literal { text: "hi".into(), quoted: false }])],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
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
}
