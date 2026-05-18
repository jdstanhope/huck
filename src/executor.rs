use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{
    Connector, ExecCommand, Pipeline, Redirect, Sequence, SimpleCommand,
};
use crate::expand::{expand, expand_assignment};
use crate::shell_state::Shell;

/// Where the terminal stage of a top-level pipeline sends its stdout when
/// there's no explicit `> file` redirect.
pub enum StdoutSink<'a> {
    Terminal,
    Capture(&'a mut Vec<u8>),
}

pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        // Parser guarantees rest.is_empty() when background is set.
        return run_background_sequence(&seq.first, shell, &mut sink, source);
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
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

fn execute_sequence_body(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first, shell, sink);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, pipeline) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_pipeline(pipeline, shell, sink);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
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
            ExecOutcome::Exit(_) => unreachable!(),
        };
        let id = shell.jobs.add_synthetic_done(display, exit);
        eprintln!("[{id}] Done");
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
        let id = shell.jobs.add_synthetic_done(display, 0);
        eprintln!("[{id}] Done");
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
    let fields = expand(word, shell);
    if fields.len() == 1 {
        Ok(fields.into_iter().next().unwrap())
    } else {
        eprintln!("huck: ambiguous redirect");
        Err(())
    }
}

fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = expand(&cmd.program, shell);
    if prog_fields.is_empty() {
        eprintln!("huck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();
    for word in &cmd.args {
        args.extend(expand(word, shell));
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
            .write(true)
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

fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };
    let files = match open_stage_files(&resolved) {
        Ok(f) => f,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&resolved.program) {
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
                if let StdoutSink::Capture(buf) = sink {
                    if let Some(mut child_stdout) = child.stdout.take() {
                        if let Err(e) = io::copy(&mut child_stdout, *buf) {
                            copy_err = Some(e);
                        }
                    }
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

        if let Some(bytes) = pending_input {
            if let Some(mut child_stdin) = child.stdin.take() {
                let _ = child_stdin.write_all(&bytes);
            }
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
        } else if want_terminal_capture {
            if let StdoutSink::Capture(buf) = sink {
                if let Some(mut child_stdout) = child.stdout.take() {
                    if let Err(e) = io::copy(&mut child_stdout, *buf) {
                        eprintln!("huck: {}: {e}", cmd.program);
                    }
                }
            }
        }

        stages.push(Stage::Process(child));
    }

    // Give the terminal to the pipeline's process group if interactive.
    if interactive {
        if let Some(pgid) = first_pid {
            give_terminal_to(pgid);
        }
    }

    let mut last_status = 0;
    for stage in stages {
        match stage {
            Stage::Done(code) => last_status = code,
            Stage::Process(mut child) => {
                if interactive {
                    let pid = child.id() as i32;
                    match wait_with_untraced(pid) {
                        Ok((raw_status, true)) => {
                            // Pipeline was stopped (Ctrl-Z).
                            let sig = libc::WSTOPSIG(raw_status);
                            let pgid = first_pid.unwrap_or(pid);
                            let display = format!("(pipeline pid {pgid})");
                            let job_id = shell.jobs.add(pgid, stage_pids.clone(), display.clone());
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
                            return ExecOutcome::Continue(128 + sig);
                        }
                        Ok((raw_status, false)) => {
                            last_status = if libc::WIFEXITED(raw_status) {
                                libc::WEXITSTATUS(raw_status)
                            } else if libc::WIFSIGNALED(raw_status) {
                                128 + libc::WTERMSIG(raw_status)
                            } else {
                                1
                            };
                            std::mem::forget(child);
                        }
                        Err(()) => {
                            last_status = 1;
                            std::mem::forget(child);
                        }
                    }
                } else {
                    last_status = match child.wait() {
                        Ok(status) => status_code(&status),
                        Err(e) => {
                            eprintln!("huck: {e}");
                            1
                        }
                    };
                }
            }
        }
    }

    if interactive {
        give_terminal_to(shell.shell_pgid);
    }
    ExecOutcome::Continue(last_status)
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
    use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
    use crate::lexer::{Word, WordPart};

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
            first: Pipeline { commands: vec![cmd] },
            rest: vec![],
            background: false,
        }
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
            first: Pipeline {
                commands: vec![exec("echo", &["first"]), exec("echo", &["second"])],
            },
            rest: vec![],
            background: false,
        };
        let mut shell = Shell::new();
        let (out, status) = execute_capturing(&seq, &mut shell);
        assert_eq!(out, "second\n");
        assert_eq!(status, 0);
    }

    use crate::jobs::JobState;

    #[test]
    fn background_pure_builtin_runs_synchronously_and_registers_done_job() {
        let seq = Sequence {
            first: Pipeline {
                commands: vec![exec("echo", &["hi"])],
            },
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let outcome = execute(&seq, &mut shell, "echo hi &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let jobs: Vec<_> = shell.jobs.iter().collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].command, "echo hi");
        assert!(matches!(jobs[0].state, JobState::Done(0)));
        assert!(jobs[0].pids.is_empty()); // synthetic — no real pids
    }

    #[test]
    fn execute_capturing_ignores_background_flag_runs_synchronously() {
        // `$(cmd &)` must wait and capture, not spawn an escaped bg job.
        let seq = Sequence {
            first: Pipeline {
                commands: vec![exec("echo", &["captured"])],
            },
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
            first: Pipeline {
                commands: vec![SimpleCommand::Assign {
                    name: "HUCK_TEST_BG_ASSIGN".to_string(),
                    value: lit_word("v"),
                }],
            },
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
}
