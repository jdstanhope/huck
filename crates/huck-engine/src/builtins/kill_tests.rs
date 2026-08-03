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

/// #402: a missing option argument is `sh_needarg` + EXECUTION_FAILURE in
/// bash, NOT the usage status 2.
#[test]
fn kill_s_missing_arg_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-s".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
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
fn kill_n_missing_arg_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-n".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
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

/// #4: a negative or zero target is handed to `kill(2)` unchanged — `0` is the
/// caller's own process group, `-N` the group `N`. Signal 0 keeps these pure
/// existence probes (the test process is IN the group it names).
#[test]
fn kill_zero_and_negative_targets_reach_kill_syscall() {
    let own_pgid = unsafe { libc::getpgrp() };
    for target in ["0", "-0", &format!("-{own_pgid}")] {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-0".to_string(), target.to_string()],
            &mut buf,
            &mut std::io::stderr(),
            &mut shell,
        );
        assert!(
            matches!(outcome, ExecOutcome::Continue(0)),
            "kill -0 {target} should probe the process group, got {outcome:?}"
        );
    }
}

/// A target too large for the pid type is still a bad target, not a group.
#[test]
fn kill_overflowing_negative_target_is_a_bad_target() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-0".to_string(), "-1234567890123".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

/// A sigspec is only recognised as the FIRST word; a later `-N` is a group.
#[test]
fn kill_sigspec_is_only_the_first_word() {
    let own_pgid = unsafe { libc::getpgrp() };
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &[
            "-0".to_string(),
            unsafe { libc::getpid() }.to_string(),
            format!("-{own_pgid}"),
        ],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

/// `--` ends option processing so `kill -- -$pgid` signals a group with the
/// default SIGTERM instead of parsing `-$pgid` as a sigspec.
#[test]
fn kill_dash_dash_makes_the_next_word_a_target() {
    let own_pgid = unsafe { libc::getpgrp() };
    // `kill -- -$pgid` with no explicit signal would send SIGTERM to our own
    // group, so exercise the parse (not the delivery) with an explicit -0.
    for args in [
        vec!["--".to_string(), "-99999".to_string()],
        vec!["-0".to_string(), "--".to_string(), format!("-{own_pgid}")],
        vec![
            "-s".to_string(),
            "WINCH".to_string(),
            "--".to_string(),
            unsafe { libc::getpid() }.to_string(),
        ],
        vec![
            "-n".to_string(),
            libc::SIGWINCH.to_string(),
            "--".to_string(),
            unsafe { libc::getpid() }.to_string(),
        ],
    ] {
        let expected = if args[0] == "--" { 1 } else { 0 }; // -99999: ESRCH
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &args, &mut buf, &mut std::io::stderr(), &mut shell);
        assert!(
            matches!(outcome, ExecOutcome::Continue(n) if n == expected),
            "kill {args:?} expected Continue({expected}), got {outcome:?}"
        );
    }
}

/// Only ONE leading `--` is consumed; a second is an ordinary (bad) target.
#[test]
fn kill_second_dash_dash_is_a_bad_target() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["--".to_string(), "--".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

/// `--` with nothing after it is a usage error, like a bare `kill`.
#[test]
fn kill_dash_dash_with_no_targets_returns_usage_status_2() {
    for args in [
        vec!["--".to_string()],
        vec!["-0".to_string(), "--".to_string()],
        vec!["-s".to_string(), "TERM".to_string(), "--".to_string()],
        vec![
            "-n".to_string(),
            libc::SIGTERM.to_string(),
            "--".to_string(),
        ],
    ] {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &args, &mut buf, &mut std::io::stderr(), &mut shell);
        assert!(
            matches!(outcome, ExecOutcome::Continue(2)),
            "kill {args:?} should be a usage error, got {outcome:?}"
        );
    }
}

/// #406: the empty target is neither a spec nor a pid, and bash names it in
/// backquotes with its own message.
#[test]
fn kill_empty_target_has_its_own_message() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut errbuf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-0".to_string(), String::new()],
        &mut buf,
        &mut errbuf,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    let s = String::from_utf8(errbuf).unwrap();
    assert!(s.contains("kill: `': not a pid or valid job spec"), "{s:?}");
}

/// #406: `kill -l` swallows ONE leading `-` word (the sigspec slot `-l` makes
/// irrelevant); a second one is an operand and is decoded.
#[test]
fn kill_l_swallows_one_leading_option_word() {
    // `-x` swallowed, `TERM` decoded.
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "-x".to_string(), "TERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(
        String::from_utf8(buf).unwrap().trim(),
        libc::SIGTERM.to_string()
    );

    // Only the first: `-3` here is an operand, and an invalid one.
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "-x".to_string(), "-3".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));

    // Swallowed word with nothing left → the full listing.
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-l".to_string(), "-TERM".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(String::from_utf8(buf).unwrap().contains("SIGHUP"));
}

/// #402: every rejected sigspec form shares bash's one wording.
#[test]
fn kill_invalid_sigspec_wording_is_uniform() {
    for args in [
        vec!["-123".to_string(), "1".to_string()],
        vec!["-FOO".to_string(), "1".to_string()],
        vec!["-s".to_string(), "BOGUS".to_string(), "1".to_string()],
        vec!["-n".to_string(), "99".to_string(), "1".to_string()],
        vec!["-l".to_string(), "xyz".to_string()],
    ] {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let mut errbuf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &args, &mut buf, &mut errbuf, &mut shell);
        assert!(
            matches!(outcome, ExecOutcome::Continue(1)),
            "kill {args:?} should fail with status 1, got {outcome:?}"
        );
        let s = String::from_utf8(errbuf).unwrap();
        assert!(
            s.contains("invalid signal specification"),
            "kill {args:?} wording: {s:?}"
        );
    }
}

/// #402: the usage text is bash's, verbatim.
#[test]
fn kill_usage_text_matches_bash() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut errbuf: Vec<u8> = Vec::new();
    let outcome = run_builtin("kill", &[], &mut buf, &mut errbuf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
    assert_eq!(
        String::from_utf8(errbuf).unwrap(),
        "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]\n"
    );
}

/// #405: one decoder for every sigspec position — number or name, `SIG`
/// prefix optional, case-insensitive, plus the name-only pseudo-signal EXIT.
#[test]
fn decode_signal_takes_numbers_and_names_alike() {
    for (spec, want) in [
        ("9", Some(libc::SIGKILL)),
        (" 9 ", Some(libc::SIGKILL)), // numbers go through legal_number
        ("0", Some(0)),
        ("64", Some(64)), // any number kill(2) may be handed
        ("65", None),
        ("-1", None),
        ("TERM", Some(libc::SIGTERM)),
        ("SIGTERM", Some(libc::SIGTERM)),
        ("term", Some(libc::SIGTERM)),
        ("EXIT", Some(0)),
        ("exit", Some(0)),
        ("SIGEXIT", None), // EXIT takes no SIG prefix in bash
        (" EXIT", None),   // names are not whitespace-tolerant
        ("BOGUS", None),
        ("", None),
    ] {
        assert_eq!(
            decode_signal(spec),
            want,
            "decode_signal({spec:?}) should be {want:?}"
        );
    }
}

/// #402: a numeric target follows bash's `legal_number()` whitespace rules —
/// strtol's leading-whitespace set, then trailing spaces/tabs only, with the
/// whole string consumed.
#[test]
fn parse_legal_number_matches_bash() {
    for (input, want) in [
        ("12", Some(12)),
        (" 12", Some(12)),
        ("12 ", Some(12)),
        ("\t12\t", Some(12)),
        ("\n12", Some(12)), // strtol skips \n as leading whitespace
        ("12\n", None),     // legal_number's trailing set is space+tab
        (" -99999 ", Some(-99999)),
        ("+12", Some(12)),
        ("-0", Some(0)),
        ("0x10", None),
        ("12abc", None),
        ("1 2", None),
        (" ", None),
        ("", None),
        ("1234567890123", None), // beyond pid_t
    ] {
        assert_eq!(
            parse_legal_number(input),
            want,
            "parse_legal_number({input:?}) should be {want:?}"
        );
    }
}

/// Failure diagnostics carry bash's bare strerror text, with no Rust
/// `(os error N)` tail.
#[test]
fn kill_error_text_has_no_rust_os_error_suffix() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut errbuf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "kill",
        &["-0".to_string(), "-99999".to_string()],
        &mut buf,
        &mut errbuf,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    let s = String::from_utf8(errbuf).unwrap();
    assert!(s.contains("kill: (-99999) - No such process"), "got {s:?}");
    assert!(!s.contains("os error"), "leaked Rust errno suffix: {s:?}");
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
