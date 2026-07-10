use super::*;
use crate::shell_state::Shell;

#[test]
fn pipefail_option_round_trips() {
    let mut sh = Shell::new();
    assert_eq!(option_get(&sh, "pipefail"), Some(false));
    option_set(&mut sh, "pipefail", true).unwrap();
    assert_eq!(option_get(&sh, "pipefail"), Some(true));
    assert!(sh.shell_options.pipefail);
    option_set(&mut sh, "pipefail", false).unwrap();
    assert_eq!(option_get(&sh, "pipefail"), Some(false));
}

#[test]
fn pipefail_not_in_dollar_dash() {
    // pipefail has no short flag, so it must never appear in `$-`.
    let mut sh = Shell::new();
    option_set(&mut sh, "pipefail", true).unwrap();
    assert!(
        !sh.dollar_dash_value().contains('p'),
        "$- must not include pipefail"
    );
}

#[test]
fn pipefail_listed_in_shell_options() {
    assert!(
        SETO_TABLE
            .iter()
            .any(|o| o.name == "pipefail" && !o.default)
    );
}

#[test]
fn shopt_bare_lists_all_57() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let oc = builtin_shopt(&[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.lines().count(), 57);
    assert_eq!(out.lines().next().unwrap(), "autocd         \toff");
    assert!(out.lines().any(|l| l == "checkwinsize   \ton"));
    assert!(out.lines().any(|l| l == "assoc_expand_once\toff")); // long name, no pad
}

#[test]
fn shopt_o_lists_27() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let oc = builtin_shopt(&["-o".into()], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(String::from_utf8(buf).unwrap().lines().count(), 27);
}
