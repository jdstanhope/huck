use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("set", &args_owned, &mut buf, &mut std::io::stderr(), shell);
    (outcome, String::from_utf8(buf).unwrap())
}

#[test]
fn set_e_enables_errexit() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-e"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.errexit);
}

#[test]
fn set_plus_e_disables() {
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    let (oc, _) = run(&["+e"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.errexit);
}

#[test]
fn set_o_errexit_long_form() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "errexit"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.errexit);
}

#[test]
fn set_plus_o_errexit_disables() {
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    let (oc, _) = run(&["+o", "errexit"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.errexit);
}

#[test]
fn set_dollar_dash_reflects_flags() {
    let mut shell = Shell::new();
    // No flags set, not interactive by default in tests.
    let dash = shell.lookup_var("-").unwrap_or_default();
    assert!(dash.is_empty() || dash == "i");
    // Enable errexit.
    run(&["-e"], &mut shell);
    let dash = shell.lookup_var("-").unwrap_or_default();
    assert!(dash.contains('e'));
    // Enable nounset.
    run(&["-u"], &mut shell);
    let dash = shell.lookup_var("-").unwrap_or_default();
    assert!(dash.contains('e'));
    assert!(dash.contains('u'));
}

#[test]
fn set_invalid_o_name_errors() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "nope_no_such_opt"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}

#[test]
fn set_v_short_flag_toggles_verbose() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-v"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.verbose);
    let (oc, _) = run(&["+v"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.verbose);
}

#[test]
fn set_o_verbose_long_form_enables() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "verbose"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.verbose);
}

#[test]
fn set_x_short_flag_toggles_xtrace() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-x"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.xtrace);
    let (oc, _) = run(&["+x"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.xtrace);
}

#[test]
fn set_o_xtrace_long_form_enables() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "xtrace"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.xtrace);
}

#[test]
fn option_set_xtrace_round_trips() {
    let mut shell = Shell::new();
    assert!(option_set(&mut shell, "xtrace", true).is_ok());
    assert_eq!(option_get(&shell, "xtrace"), Some(true));
    assert!(option_set(&mut shell, "xtrace", false).is_ok());
    assert_eq!(option_get(&shell, "xtrace"), Some(false));
}

#[test]
fn set_posix_option_is_accepted_as_noop_via_option_set() {
    let mut shell = Shell::new();
    assert!(option_set(&mut shell, "posix", true).is_ok());
    assert!(option_set(&mut shell, "posix", false).is_ok());
}

#[test]
fn option_get_posix_returns_table_default() {
    let shell = Shell::new();
    // SETO_TABLE default for posix is `false`.
    assert_eq!(option_get(&shell, "posix"), Some(false));
}

#[test]
fn set_o_listing_shows_state() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-o"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(out.lines().any(|l| l.starts_with("errexit")));
    assert!(out.lines().any(|l| l.starts_with("nounset")));
}

#[test]
fn set_plus_o_listing_reinput_form() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["+o"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // Both off by default.
    assert!(out.lines().any(|l| l == "set +o errexit"));
    assert!(out.lines().any(|l| l == "set +o nounset"));
}

#[test]
fn set_eu_cluster() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-eu"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.errexit);
    assert!(shell.shell_options.nounset);
}

#[test]
fn set_dash_dash_resets_positional() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-e", "--", "a", "b", "c"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.errexit);
    assert_eq!(
        shell.positional_args,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn set_dash_eo_cluster_consumes_next_arg_as_name() {
    // Regression: bash treats `-eo NAME` as enabling -e then
    // -o NAME (the o-in-cluster consumes the next arg as the
    // option name). Previously huck rejected the o-in-cluster
    // as "not yet supported".
    let mut shell = Shell::new();
    let (oc, _) = run(&["-eo", "nounset"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.errexit, "expected errexit on");
    assert!(shell.shell_options.nounset, "expected nounset on");
}

#[test]
fn set_plus_eo_cluster_consumes_next_arg_as_name() {
    // Symmetric: `+eo NAME` disables -e then -o NAME.
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    shell.shell_options.nounset = true;
    let (oc, _) = run(&["+eo", "nounset"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.errexit, "expected errexit off");
    assert!(!shell.shell_options.nounset, "expected nounset off");
}

#[test]
fn set_o_lists_full_27_name_table_tab_format() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-o"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        27,
        "set -o must list all 27 names; got {lines:?}"
    );
    // bash format: name left-justified in 15, a TAB, then on/off.
    assert_eq!(lines[0], "allexport      \toff");
    assert_eq!(lines[3], "errexit        \toff");
    // long name (>=15 chars): no padding, just name + TAB + value.
    assert!(lines.contains(&"interactive-comments\ton"));
    assert!(lines.contains(&"braceexpand    \ton"));
    assert!(lines.contains(&"hashall        \ton"));
}

#[test]
fn set_o_reflects_real_state_for_implemented() {
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    let (_, out) = run(&["-o"], &mut shell);
    assert!(out.lines().any(|l| l == "errexit        \ton"));
}

#[test]
fn set_o_accepts_all_bash_options() {
    // v270: bash accepts every valid `set -o` name in a script (most are
    // interactive-only toggles that are inert non-interactively). huck now
    // accepts + stores them all (rc 0), replacing the old "not yet supported".
    for name in [
        "allexport",
        "braceexpand",
        "hashall",
        "histexpand",
        "history",
        "ignoreeof",
        "interactive-comments",
        "keyword",
        "monitor",
        "notify",
        "onecmd",
        "functrace",
        "errtrace",
        "emacs",
        "vi",
        "nolog",
        "privileged",
    ] {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", name], &mut shell);
        assert!(
            matches!(oc, ExecOutcome::Continue(0)),
            "-o {name} should be accepted"
        );
        assert_eq!(
            option_get(&shell, name),
            Some(true),
            "-o {name} should be stored on"
        );
    }
}

#[test]
fn set_single_char_flags_accepted() {
    // bash single-char aliases: -a allexport, -b notify, -h hashall,
    // -k keyword, -m monitor, -t onecmd, -B braceexpand, -E errtrace,
    // -H histexpand, -P physical, -T functrace, -p privileged.
    let cases = [
        ("-a", "allexport"),
        ("-b", "notify"),
        ("-t", "onecmd"),
        ("-k", "keyword"),
        ("-m", "monitor"),
        ("-E", "errtrace"),
        ("-H", "histexpand"),
        ("-P", "physical"),
        ("-T", "functrace"),
        ("-p", "privileged"),
    ];
    for (flag, name) in cases {
        let mut shell = Shell::new();
        let (oc, _) = run(&[flag], &mut shell);
        assert!(
            matches!(oc, ExecOutcome::Continue(0)),
            "{flag} should be accepted"
        );
        assert_eq!(
            option_get(&shell, name),
            Some(true),
            "{flag} should turn {name} on"
        );
        let (oc2, _) = run(&[&flag.replace('-', "+")], &mut shell);
        assert!(
            matches!(oc2, ExecOutcome::Continue(0)),
            "+{} should be accepted",
            &flag[1..]
        );
        assert_eq!(
            option_get(&shell, name),
            Some(false),
            "+{} should turn {name} off",
            &flag[1..]
        );
    }
    // -h hashall / -B braceexpand default ON: verify +h/+B turn them off then -h/-B on.
    for (flag, name) in [("h", "hashall"), ("B", "braceexpand")] {
        let mut shell = Shell::new();
        let (oc, _) = run(&[&format!("+{flag}")], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(option_get(&shell, name), Some(false));
        let (oc, _) = run(&[&format!("-{flag}")], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(option_get(&shell, name), Some(true));
    }
}

#[test]
fn set_o_enable_unknown_name_is_invalid() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-o", "nope_no_such_opt"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
    // bash wording: `set: <name>: invalid option name`.
    assert!(
        out.is_empty(),
        "error goes to stderr, not the captured stdout: {out:?}"
    );
}

// `set -a` enabling the flag is tested here; the auto-export *behavior*
// it gates (assignments become exported) lives in the executor and is
// covered byte-for-byte against bash by set_o_options_diff_check.sh.
#[test]
fn set_dash_a_enables_allexport() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-a"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.allexport);
    assert_eq!(option_get(&shell, "allexport"), Some(true));
}

#[test]
fn set_dash_c_enables_noclobber() {
    let mut shell = Shell::new();
    let _ = run(&["-C"], &mut shell);
    assert!(shell.shell_options.noclobber);
    assert_eq!(option_get(&shell, "noclobber"), Some(true));
}

#[test]
fn set_plus_c_disables_noclobber() {
    let mut shell = Shell::new();
    let _ = run(&["-C"], &mut shell);
    let _ = run(&["+C"], &mut shell);
    assert!(!shell.shell_options.noclobber);
}

#[test]
fn set_o_noclobber_enables() {
    let mut shell = Shell::new();
    let _ = run(&["-o", "noclobber"], &mut shell);
    assert_eq!(option_get(&shell, "noclobber"), Some(true));
}
