use std::env;
use std::io::Write;
use std::path::Path;

use crate::shell_state::Shell;
use libc;

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
}

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "exit" | "pwd" | "echo" | "export" | "unset" | "jobs" | "wait" | "fg" | "bg"
    )
}

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true. `out` is the
/// destination for any stdout the builtin produces (`echo`, `pwd`); `cd` and
/// `exit` produce no stdout and ignore it.
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        "jobs" => builtin_jobs(args, out, shell),
        "wait" => builtin_wait(args, out, shell),
        "fg" => builtin_fg(args, shell),
        "bg" => builtin_bg(args, out, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                eprintln!("shuck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    match env::set_current_dir(Path::new(&target)) {
        Ok(()) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: cd: {target}: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_pwd(out: &mut dyn Write) -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            if let Err(e) = writeln!(out, "{}", path.display()) {
                eprintln!("shuck: pwd: {e}");
                return ExecOutcome::Continue(1);
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String], out: &mut dyn Write) -> ExecOutcome {
    if let Err(e) = writeln!(out, "{}", args.join(" ")) {
        eprintln!("shuck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code),
            Err(_) => {
                eprintln!("shuck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false; };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn builtin_export(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut entries: Vec<(String, String)> = shell
            .exported_env()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        entries.sort();
        for (name, value) in entries {
            if let Err(e) = writeln!(out, "export {name}={value}") {
                eprintln!("shuck: export: {e}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_error = false;
    for arg in args {
        match arg.find('=') {
            Some(idx) => {
                let name = &arg[..idx];
                let value = &arg[idx + 1..];
                if !is_valid_name(name) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export_set(name, value.to_string());
            }
            None => {
                if !is_valid_name(arg) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export(arg);
            }
        }
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

fn builtin_unset(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut any_error = false;
    for arg in args {
        if !is_valid_name(arg) {
            eprintln!("shuck: unset: '{arg}': not a valid identifier");
            any_error = true;
            continue;
        }
        shell.unset(arg);
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

fn builtin_jobs(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: jobs: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    let (current, previous) = shell.jobs.current_and_previous();
    for job in shell.jobs.iter() {
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        if let Err(e) = writeln!(out, "{}", crate::jobs::notification_line(job, flag)) {
            eprintln!("shuck: jobs: {e}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

fn builtin_wait(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    match args.len() {
        0 => wait_all(shell),
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "wait", shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            wait_for_job(id, shell)
        }
        1 => match args[0].parse::<i32>() {
            Ok(pid) if pid > 0 => wait_for_pid(pid, shell),
            _ => {
                eprintln!("shuck: wait: usage: wait [%job | pid]");
                ExecOutcome::Continue(2)
            }
        },
        _ => {
            eprintln!("shuck: wait: usage: wait [%job | pid]");
            ExecOutcome::Continue(2)
        }
    }
}

fn wait_all(shell: &mut Shell) -> ExecOutcome {
    while shell.jobs.has_pending() {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    // Print Done lines for anything that just transitioned during the wait.
    crate::jobs::reap_and_notify(shell);
    ExecOutcome::Continue(0)
}

fn wait_for_job(id: u32, shell: &mut Shell) -> ExecOutcome {
    loop {
        // Check terminal state first — handles already-Done jobs.
        let terminal = shell.jobs.iter()
            .find(|j| j.id == id)
            .and_then(|j| match j.state {
                crate::jobs::JobState::Done(c) => Some(c),
                crate::jobs::JobState::Signaled(s) => Some(128 + s),
                _ => None,
            });
        if let Some(code) = terminal {
            return ExecOutcome::Continue(code);
        }
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn wait_for_pid(pid: i32, shell: &mut Shell) -> ExecOutcome {
    let mut first = true;
    loop {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if libc::WIFSTOPPED(status) {
                // Still alive; keep polling.
                first = false;
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                1
            };
            return ExecOutcome::Continue(code);
        }
        if r < 0 {
            // ECHILD: not a child (or already reaped). On the first call,
            // surface as "not a child." On a subsequent call, treat as a
            // race we can't recover from.
            if first {
                eprintln!("shuck: wait: pid {pid} is not a child of this shell");
                return ExecOutcome::Continue(127);
            }
            return ExecOutcome::Continue(1);
        }
        first = false;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn check_sigint(shell: &Shell) -> bool {
    if shell.sigint_flag
        .compare_exchange(
            true,
            false,
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_ok()
    {
        eprintln!();
        true
    } else {
        false
    }
}

/// Parses `arg` as a job spec and resolves it to a job id. On parse or
/// resolution failure, prints a `shuck: <builtin>: ...` error to stderr
/// and returns `Err(ExecOutcome::Continue(1))` so the caller can `?` it.
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        eprintln!("shuck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    shell.jobs.resolve(&spec).ok_or_else(|| {
        eprintln!("shuck: {builtin}: {arg}: no such job");
        ExecOutcome::Continue(1)
    })
}

fn builtin_fg(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                eprintln!("shuck: fg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => match resolve_spec_or_error(&args[0], "fg", shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        _ => {
            eprintln!("shuck: fg: usage: fg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, pids, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.pids.clone(), job.command.clone())
        } else {
            eprintln!("shuck: fg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    eprintln!("{command}");

    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
        libc::killpg(pgid, libc::SIGCONT);
    }

    let mut last_status = 0;
    let mut stopped_sig: Option<i32> = None;
    let mut completed = 0;
    let total = pids.len();
    loop {
        if completed == total { break; }
        let mut status: libc::c_int = 0;
        // Wait for any child in this pgrp. -pgid means "any pid whose pgid == pgid".
        let r = unsafe { libc::waitpid(-pgid, &mut status, libc::WUNTRACED) };
        if r < 0 {
            // ECHILD — SIGCHLD reaper got ahead of us. Stop the loop; the
            // job will be cleaned up by the next prompt's notify cycle.
            last_status = 1;
            break;
        }
        if libc::WIFSTOPPED(status) {
            stopped_sig = Some(libc::WSTOPSIG(status));
            break;
        }
        if libc::WIFEXITED(status) {
            last_status = libc::WEXITSTATUS(status);
        } else if libc::WIFSIGNALED(status) {
            last_status = 128 + libc::WTERMSIG(status);
        } else {
            last_status = 1;
        }
        completed += 1;
    }

    unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, shell.shell_pgid); }

    if let Some(sig) = stopped_sig {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Stopped(sig);
            job.notified = true;
        }
        let line = shell.jobs.iter()
            .find(|j| j.id == id)
            .map(|j| crate::jobs::notification_line(j, '+'))
            .unwrap_or_default();
        eprintln!("\n{line}");
        return ExecOutcome::Continue(128 + sig);
    }

    // Only remove from the job table if all pids completed successfully.
    // If the wait loop exited early (ECHILD race), leave the job for the
    // prompt-time reaper to handle.
    if completed == total {
        shell.jobs.jobs_mut().retain(|j| j.id != id);
    }
    ExecOutcome::Continue(last_status)
}

fn builtin_bg(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_stopped_id() {
            Some(id) => id,
            None => {
                eprintln!("shuck: bg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "bg", shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            // Verify the resolved job is actually Stopped.
            let is_stopped = shell.jobs.iter()
                .find(|j| j.id == id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
                .unwrap_or(false);
            if !is_stopped {
                eprintln!("shuck: bg: job %{id} already running");
                return ExecOutcome::Continue(1);
            }
            id
        }
        _ => {
            eprintln!("shuck: bg: usage: bg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.command.clone())
        } else {
            eprintln!("shuck: bg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    unsafe { libc::killpg(pgid, libc::SIGCONT); }

    eprintln!("[{id}]+ {command} &");
    ExecOutcome::Continue(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognizes_builtins() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("pwd"));
        assert!(is_builtin("echo"));
        assert!(is_builtin("export"));
        assert!(is_builtin("unset"));
        assert!(!is_builtin("ls"));
    }

    #[test]
    fn exit_with_no_args() {
        assert!(matches!(builtin_exit(&[]), ExecOutcome::Exit(0)));
    }

    #[test]
    fn exit_with_code() {
        assert!(matches!(
            builtin_exit(&["3".to_string()]),
            ExecOutcome::Exit(3)
        ));
    }

    #[test]
    fn exit_with_bad_code_continues() {
        assert!(matches!(
            builtin_exit(&["abc".to_string()]),
            ExecOutcome::Continue(_)
        ));
    }

    #[test]
    fn echo_writes_args_joined_by_spaces() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_echo(&["hello".to_string(), "world".to_string()], &mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(out, b"hello world\n");
    }

    #[test]
    fn echo_with_no_args_writes_a_blank_line() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[], &mut out);
        assert_eq!(out, b"\n");
    }

    #[test]
    fn pwd_writes_the_current_directory() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_pwd(&mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let written = String::from_utf8(out).unwrap();
        let expected = env::current_dir().unwrap();
        assert_eq!(written.trim_end(), expected.to_str().unwrap());
    }

    #[test]
    fn export_marks_existing() {
        let mut shell = Shell::new();
        shell.set("SHUCK_EXP", "v".to_string());
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP");
        assert!(in_exported);
    }

    #[test]
    fn export_name_only_creates_empty_exported() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_NEW_VAR".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_NEW_VAR"), Some(""));
        assert!(shell.exported_env().any(|(k, _)| k == "SHUCK_NEW_VAR"));
    }

    #[test]
    fn export_sets_and_exports() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP2=hello".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_EXP2"), Some("hello"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP2");
        assert!(in_exported);
    }

    #[test]
    fn export_invalid_name_continues_with_error() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(
            &["1BAD=x".to_string(), "GOOD=y".to_string()],
            &mut out,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.get("1BAD"), None);
        assert_eq!(shell.get("GOOD"), Some("y"));
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("SHUCK_RM", "v".to_string());
        let outcome = builtin_unset(&["SHUCK_RM".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_RM"), None);
    }

    #[test]
    fn unset_invalid_name_is_error() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["1BAD".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unset_unknown_name_is_silent_ok() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["NEVER_SET_SHUCK_XYZ".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn jobs_with_empty_table_prints_nothing_and_returns_zero() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(out.is_empty());
    }

    #[test]
    fn jobs_lists_synthetic_done_entry() {
        let mut shell = Shell::new();
        let _ = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[1]"));
        assert!(s.contains("Done"));
        assert!(s.contains("echo hi"));
    }

    #[test]
    fn jobs_with_args_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&["-l".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn jobs_lists_stopped_without_ampersand_suffix() {
        let mut shell = Shell::new();
        shell.jobs.add(100, vec![100], "sleep 100".to_string());
        shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Stopped"), "got: {out:?}");
        assert!(!out.trim_end().ends_with('&'), "Stopped line must NOT end with &; got: {out:?}");
    }

    #[test]
    fn wait_with_no_jobs_returns_zero_immediately() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_wait(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn is_builtin_recognizes_jobs_and_wait() {
        assert!(is_builtin("jobs"));
        assert!(is_builtin("wait"));
    }
}

#[cfg(test)]
mod fg_bg_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn fg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_on_running_job_returns_no_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn is_builtin_recognizes_fg_and_bg() {
        assert!(is_builtin("fg"));
        assert!(is_builtin("bg"));
    }

    #[test]
    fn fg_with_bad_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_non_percent_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn fg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "fg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn bg_with_bad_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_running_spec_errors_already_running() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "bg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_with_no_such_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_unparseable_pid_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_done_spec_returns_decoded_status_immediately() {
        let mut shell = Shell::new();
        // Synthetic Done job — wait should see it's already terminal and
        // return decode(0) → 0 without blocking.
        shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn wait_with_done_spec_returns_nonzero_for_exit_n() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("false".to_string(), 1);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }
}
