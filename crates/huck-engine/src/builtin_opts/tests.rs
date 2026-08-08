use super::*;
use crate::shell_state::Shell;

fn scan(spec: &str, argv: &[&str]) -> (Vec<(char, Option<String>)>, usize, Option<i32>) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut sh = Shell::new();
    let mut err: Vec<u8> = Vec::new();
    let mut g = Getopt::new("t", ArgView::Plain(&args), spec);
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
