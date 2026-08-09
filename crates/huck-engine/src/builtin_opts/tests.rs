use super::*;
use crate::shell_state::Shell;

// The tuple is the scanner's three observable outputs (opts scanned, the
// rest-index, and the terminal error code); the brief specifies this exact
// shape, so factor-out is a spec deviation rather than a real simplification.
#[allow(clippy::type_complexity)]
fn scan(spec: &str, argv: &[&str]) -> (Vec<(char, Option<String>)>, usize, Option<i32>) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut sh = Shell::new();
    let mut err: Vec<u8> = Vec::new();
    let mut g = Getopt::new("alias", ArgView::Plain(&args), spec);
    let mut out = Vec::new();
    loop {
        match g.next_opt(&mut sh, &mut err) {
            Ok(Some(o)) => out.push((o.ch, o.value.clone())),
            Ok(None) => return (out, g.rest_index(), None),
            Err(code) => return (out, g.rest_index(), Some(code)),
        }
    }
}

#[test]
fn bundled_shorts_are_order_independent() {
    assert_eq!(scan("ap", &["-pa"]).0, vec![('p', None), ('a', None)]);
    assert_eq!(scan("ap", &["-ap"]).0, vec![('a', None), ('p', None)]);
}

#[test]
fn double_dash_terminates_and_is_consumed() {
    let (opts, rest, err) = scan("p", &["-p", "--", "-a"]);
    assert_eq!(opts, vec![('p', None)]);
    assert_eq!(rest, 2, "`--` is consumed; operands start after it");
    assert!(err.is_none());
}

#[test]
fn lone_dash_is_an_operand_not_an_option() {
    let (opts, rest, err) = scan("p", &["-"]);
    assert!(opts.is_empty());
    assert_eq!(rest, 0, "a lone `-` is left for the builtin as an operand");
    assert!(err.is_none());
}

#[test]
fn value_attaches_or_separates() {
    assert_eq!(scan("d:", &["-d", "x"]).0, vec![('d', Some("x".into()))]);
    assert_eq!(scan("d:", &["-dx"]).0, vec![('d', Some("x".into()))]);
}

#[test]
fn scanning_stops_at_the_first_non_option() {
    let (opts, rest, err) = scan("p", &["-p", "name", "-a"]);
    assert_eq!(opts, vec![('p', None)]);
    assert_eq!(
        rest, 1,
        "`-a` after an operand is an OPERAND, not an option"
    );
    assert!(err.is_none());
}

#[test]
fn unknown_option_reports_two_and_stops() {
    let (_, _, err) = scan("p", &["-Q"]);
    assert_eq!(err, Some(2));
}

#[test]
fn colon_value_marker_is_never_itself_an_accepted_option() {
    // The spec's ':' is a VALUE marker, not an option character. A spec
    // that contains ':' (e.g. because it has a value-taking option) must
    // still reject a literal `-:` as invalid, not hand back `Opt { ch: ':'
    // }` — every call site's `match` has no arm for ':' and panics via
    // `_ => unreachable!("spec and match must agree")` (the bug this test
    // pins: `accepts(':')` used to return true because ':' appears in the
    // spec string for an unrelated reason).
    let (opts, _, err) = scan("lrp:dt", &["-:"]);
    assert!(opts.is_empty(), "`-:` must not be scanned as an option");
    assert_eq!(err, Some(2), "`-:` must take the invalid-option path");
}

#[test]
fn missing_value_message_is_bashs_shape_not_getopt3s() {
    // bash: `hash: -p: option requires an argument` — NOT the getopt(3)
    // `hash: option requires an argument -- p` shape. A prior version of
    // this scanner emitted the getopt(3) shape; `hash -p` (the first
    // `:`-spec builtin converted, #496 Task 5) caught it.
    let args: Vec<String> = vec!["-p".to_string()];
    let mut sh = Shell::new();
    let mut err: Vec<u8> = Vec::new();
    let mut g = Getopt::new("hash", ArgView::Plain(&args), "p:");
    match g.next_opt(&mut sh, &mut err) {
        Err(2) => {}
        other => panic!(
            "expected Err(2), got a value that isn't Debug-comparable here: {}",
            other.is_err()
        ),
    }
    let text = String::from_utf8(err).unwrap();
    assert!(
        text.contains("hash: -p: option requires an argument"),
        "got: {text:?}"
    );
    assert!(
        !text.contains("-- p"),
        "must not use the getopt(3) shape: {text:?}"
    );
}

#[test]
fn usage_is_keyed_on_the_invoked_name_not_the_implementation() {
    // readarray/mapfile and typeset/declare share an implementation but must
    // NOT share a usage string — bash names the builtin the user invoked.
    assert!(usage_for("readarray").starts_with("readarray "));
    assert!(usage_for("mapfile").starts_with("mapfile "));
    assert!(usage_for("typeset").starts_with("typeset "));
    assert!(usage_for("declare").starts_with("declare "));
    assert_ne!(usage_for("readarray"), usage_for("mapfile"));
    assert_ne!(usage_for("typeset"), usage_for("declare"));
}

#[test]
fn every_builtin_with_a_scanner_has_a_usage_string() {
    for name in [
        "unset",
        "readonly",
        "read",
        "type",
        "hash",
        "declare",
        "typeset",
        "printf",
        "command",
        "mapfile",
        "readarray",
        "help",
        "complete",
        "compgen",
        "compopt",
        "jobs",
        "trap",
        "alias",
        "unalias",
        "builtin",
        "export",
        "cd",
        "wait",
        "history",
        "local",
        "getopts",
        "shopt",
        "disown",
        "umask",
        "ulimit",
        "pwd",
        "enable",
    ] {
        assert!(!usage_for(name).is_empty(), "no usage string for {name}");
    }
}
