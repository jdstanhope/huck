use super::*;
use crate::shell_state::Shell;

#[test]
fn is_builtin_recognizes_kill() {
    assert!(is_builtin("kill"));
}

#[test]
fn kill_no_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("kill", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn kill_sig_flag_with_no_targets_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-TERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn kill_invalid_signal_name_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-ABC".to_string(), "%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_invalid_signal_number_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-9999".to_string(), "%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_unparseable_target_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_no_such_job_spec_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["%99".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn signal_by_name_table_recognizes_common_signals() {
    assert_eq!(signal_by_name("HUP"), Some(libc::SIGHUP));
    assert_eq!(signal_by_name("SIGHUP"), Some(libc::SIGHUP));
    assert_eq!(signal_by_name("hup"), Some(libc::SIGHUP));
    assert_eq!(signal_by_name("sighup"), Some(libc::SIGHUP));
    assert_eq!(signal_by_name("INT"), Some(libc::SIGINT));
    assert_eq!(signal_by_name("KILL"), Some(libc::SIGKILL));
    assert_eq!(signal_by_name("TERM"), Some(libc::SIGTERM));
    assert_eq!(signal_by_name("STOP"), Some(libc::SIGSTOP));
    assert_eq!(signal_by_name("CONT"), Some(libc::SIGCONT));
    assert_eq!(signal_by_name("USR1"), Some(libc::SIGUSR1));
    assert_eq!(signal_by_name("USR2"), Some(libc::SIGUSR2));
    assert_eq!(signal_by_name("TSTP"), Some(libc::SIGTSTP));
    assert_eq!(signal_by_name("PIPE"), Some(libc::SIGPIPE));
    assert_eq!(signal_by_name("ALRM"), Some(libc::SIGALRM));
    assert_eq!(signal_by_name("CHLD"), Some(libc::SIGCHLD));
    assert_eq!(signal_by_name("TTIN"), Some(libc::SIGTTIN));
    assert_eq!(signal_by_name("TTOU"), Some(libc::SIGTTOU));
    assert_eq!(signal_by_name("ABC"), None);
    assert_eq!(signal_by_name(""), None);
}

#[test]
fn kill_signal_zero_is_accepted_as_valid_numeric() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // No targets after the signal → usage(2) — but the signal itself
    // must parse without "invalid signal number" status 1.
    let outcome = run_builtin(
        "kill",
        &["-0".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(
        matches!(outcome, ExecOutcome::Continue(2)),
        "kill -0 (no targets) should reach usage check, not signal check"
    );
}

#[test]
fn kill_l_no_args_lists_all_standard_signals() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    // Common signals that were already listed before v189.
    assert!(s.contains("KILL"), "output missing KILL: {s}");
    assert!(s.contains("TERM"), "output missing TERM: {s}");
    assert!(s.contains("WINCH"), "output missing WINCH: {s}");
    // The point of v189: the listing must now include the newly-added
    // standard signals by name (bare-name format at this stage).
    for sig in ["ABRT", "SEGV", "BUS", "FPE", "ILL"] {
        assert!(s.contains(sig), "kill -l listing missing {sig}: {s}");
    }
}

#[test]
fn kill_l_listing_matches_bash_format() {
    let mut buf = Vec::new();
    print_killable_table(&mut buf);
    let s = String::from_utf8(buf).unwrap();
    // bash: ` 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP\n…`
    let first = s.lines().next().unwrap();
    assert_eq!(
        first,
        " 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP"
    );
    // SIG prefix everywhere, 5 columns per full row
    assert!(s.contains("SIGABRT"), "missing SIGABRT: {s}");
    assert!(s.contains("11) SIGSEGV"));
}

#[test]
fn kill_l_with_name_returns_number() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "TERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.trim(), libc::SIGTERM.to_string());
}

#[test]
fn kill_l_with_sig_prefix_returns_number() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "SIGTERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.trim(), libc::SIGTERM.to_string());
}

#[test]
fn kill_l_lowercase_name_returns_number() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "term".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.trim(), libc::SIGTERM.to_string());
}

#[test]
fn kill_l_with_number_returns_name() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), libc::SIGTERM.to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.trim(), "TERM");
}

#[test]
fn kill_l_status_decode() {
    let arg = (128 + libc::SIGKILL).to_string();
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), arg],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    assert_eq!(s.trim(), "KILL");
}

#[test]
fn kill_l_unknown_name_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "xyz".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_l_invalid_number_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "99".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_l_multiple_args_decodes_each() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &[
            "-l".to_string(),
            libc::SIGHUP.to_string(),
            libc::SIGKILL.to_string(),
            libc::SIGTERM.to_string(),
        ],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines, vec!["HUP", "KILL", "TERM"]);
}

#[test]
fn signal_by_name_resolves_winch() {
    assert_eq!(signal_by_name("WINCH"), Some(libc::SIGWINCH));
    assert_eq!(signal_by_name("SIGWINCH"), Some(libc::SIGWINCH));
    assert_eq!(signal_by_name("winch"), Some(libc::SIGWINCH));
}

#[test]
fn kill_s_with_name_resolves_and_dispatches() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let pid = unsafe { libc::getpid() }.to_string();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string(), "WINCH".to_string(), pid],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn kill_s_with_sig_prefix_resolves() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let pid = unsafe { libc::getpid() }.to_string();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string(), "SIGWINCH".to_string(), pid],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn kill_s_lowercase_name_resolves() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let pid = unsafe { libc::getpid() }.to_string();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string(), "winch".to_string(), pid],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn kill_s_missing_arg_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn kill_s_invalid_name_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string(), "BOGUS".to_string(), "99999".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_s_no_targets_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string(), "TERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn kill_n_with_number_resolves_and_dispatches() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let pid = unsafe { libc::getpid() }.to_string();
    let outcome = run_builtin(
        "kill",
        &["-n".to_string(), libc::SIGWINCH.to_string(), pid],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn kill_n_missing_arg_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-n".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn kill_n_invalid_number_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-n".to_string(), "99".to_string(), "12345".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn kill_dash_sig_short_form_still_works_after_refactor() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let pid = unsafe { libc::getpid() }.to_string();
    let outcome = run_builtin(
        "kill",
        &["-WINCH".to_string(), pid],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}
