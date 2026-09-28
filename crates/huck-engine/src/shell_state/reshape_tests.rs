//! `reshape_to` / `reshape_in_frame_to` — bash's `-a`/`-A` conversion table
//! (#347, #697, #734). Every expectation here was measured against bash
//! 5.2.21 and is pinned byte-for-byte by
//! `tests/scripts/array_conversion_diff_check.sh`; these tests exist so a
//! regression in one cell names the cell instead of failing a shell fragment.

use super::*;
use crate::shell_state::DeclareErr;

/// How a variable starts out, for the table below.
enum Start {
    Absent,
    Unset(Shape),
    Scalar(&'static str),
    Indexed(&'static [(usize, &'static str)]),
    Assoc(&'static [(&'static str, &'static str)]),
}

fn seed(start: &Start) -> Shell {
    let mut sh = Shell::new();
    match start {
        Start::Absent => {}
        Start::Unset(shape) => {
            sh.vars.insert("v".to_string(), Variable::unset(*shape));
        }
        Start::Scalar(s) => sh.set("v", s.to_string()),
        Start::Indexed(elems) => {
            let mut m = BTreeMap::new();
            for (i, s) in *elems {
                m.insert(*i, s.to_string());
            }
            sh.replace_indexed("v", m).unwrap();
        }
        Start::Assoc(pairs) => {
            sh.replace_associative(
                "v",
                pairs
                    .iter()
                    .map(|(k, s)| (k.to_string(), s.to_string()))
                    .collect(),
            )
            .unwrap();
        }
    }
    sh
}

/// The full table, once per target shape. `expect` is the `declare -p` body
/// bash prints afterwards, or the refusal.
#[test]
fn reshape_to_is_bashs_conversion_table() {
    // (label, start, target, Ok(declare-line) | Err(kind))
    type Row = (&'static str, Start, Shape, Result<&'static str, DeclareErr>);
    let rows: Vec<Row> = vec![
        // An absent or valueless name just records the shape (#600).
        (
            "absent -> a",
            Start::Absent,
            Shape::Indexed,
            Ok("declare -a v"),
        ),
        (
            "absent -> A",
            Start::Absent,
            Shape::Associative,
            Ok("declare -A v"),
        ),
        (
            "unset scalar -> a",
            Start::Unset(Shape::Scalar),
            Shape::Indexed,
            Ok("declare -a v"),
        ),
        (
            "unset scalar -> A",
            Start::Unset(Shape::Scalar),
            Shape::Associative,
            Ok("declare -A v"),
        ),
        (
            "unset indexed -> a",
            Start::Unset(Shape::Indexed),
            Shape::Indexed,
            Ok("declare -a v"),
        ),
        (
            "unset assoc -> A",
            Start::Unset(Shape::Associative),
            Shape::Associative,
            Ok("declare -A v"),
        ),
        // A materialised scalar is PROMOTED to element/key 0 (#697) — huck
        // used to refuse the associative direction outright.
        (
            "scalar -> a promotes",
            Start::Scalar("s"),
            Shape::Indexed,
            Ok("declare -a v=([0]=\"s\")"),
        ),
        (
            "scalar -> A promotes",
            Start::Scalar("s"),
            Shape::Associative,
            Ok("declare -A v=([0]=\"s\" )"),
        ),
        // Same shape twice keeps the value.
        (
            "indexed -> a keeps",
            Start::Indexed(&[(0, "1"), (1, "2")]),
            Shape::Indexed,
            Ok("declare -a v=([0]=\"1\" [1]=\"2\")"),
        ),
        (
            "assoc -> A keeps",
            Start::Assoc(&[("k", "1")]),
            Shape::Associative,
            Ok("declare -A v=([k]=\"1\" )"),
        ),
        // The two refusals — in bash the data is never silently discarded,
        // whether the existing variable has a value or only a shape (#347).
        (
            "indexed -> A refuses",
            Start::Indexed(&[(0, "1")]),
            Shape::Associative,
            Err(DeclareErr::IndexedExists),
        ),
        (
            "unset indexed -> A refuses",
            Start::Unset(Shape::Indexed),
            Shape::Associative,
            Err(DeclareErr::IndexedExists),
        ),
        (
            "assoc -> a refuses",
            Start::Assoc(&[("k", "1")]),
            Shape::Indexed,
            Err(DeclareErr::AssociativeExists),
        ),
        (
            "unset assoc -> a refuses",
            Start::Unset(Shape::Associative),
            Shape::Indexed,
            Err(DeclareErr::AssociativeExists),
        ),
    ];

    for (label, start, target, expect) in rows {
        let before = match &start {
            Start::Absent => None,
            _ => Some(crate::builtins::format_declare_line(
                "v",
                &seed(&start).vars["v"],
            )),
        };
        let mut sh = seed(&start);
        let got = sh.reshape_to("v", target);
        match expect {
            Ok(line) => {
                assert!(got.is_ok(), "{label}: expected Ok, got {got:?}");
                assert_eq!(
                    crate::builtins::format_declare_line("v", &sh.vars["v"]),
                    line,
                    "{label}"
                );
            }
            Err(kind) => {
                assert_eq!(
                    format!("{got:?}"),
                    format!("{:?}", Err::<(), DeclareErr>(kind)),
                    "{label}"
                );
                // A refusal leaves the variable EXACTLY as it was — bash
                // reports the conversion error instead of discarding data.
                assert_eq!(
                    Some(crate::builtins::format_declare_line("v", &sh.vars["v"])),
                    before,
                    "{label}: refusal must not modify the variable"
                );
            }
        }
    }
}

/// readonly guards the VALUE, not the SHAPE (#734): bash performs
/// `readonly R=1; declare -a R` and prints `declare -ar R=([0]="1")`, so
/// `reshape_to` deliberately bypasses the readonly check that `assign` owns.
/// The guard is still there for a real assignment afterwards.
#[test]
fn reshape_to_ignores_readonly_but_assign_still_refuses() {
    let mut sh = Shell::new();
    sh.set("v", "1".to_string());
    sh.mark_readonly("v");

    assert!(sh.reshape_to("v", Shape::Indexed).is_ok());
    assert_eq!(
        crate::builtins::format_declare_line("v", &sh.vars["v"]),
        "declare -ar v=([0]=\"1\")"
    );

    let mut m = BTreeMap::new();
    m.insert(0, "7".to_string());
    assert!(
        sh.replace_indexed("v", m).is_err(),
        "the readonly guard on the VALUE must survive the reshape"
    );
}

/// A shape change on a name that is already local in the CURRENT frame
/// DISCARDS the value, where the same change at global scope promotes:
/// `f(){ local v=s; local -a v; }` gives `declare -a v=()` in bash.
#[test]
fn reshape_in_frame_discards_a_scalar_but_only_on_a_real_change() {
    let mut sh = Shell::new();
    sh.set("v", "s".to_string());
    assert!(sh.reshape_in_frame_to("v", Shape::Indexed).is_ok());
    assert_eq!(
        crate::builtins::format_declare_line("v", &sh.vars["v"]),
        "declare -a v=()"
    );

    // Re-declaring the SAME shape is not a change — the value survives.
    let mut sh = Shell::new();
    let mut m = BTreeMap::new();
    m.insert(0, "1".to_string());
    sh.replace_indexed("v", m).unwrap();
    assert!(sh.reshape_in_frame_to("v", Shape::Indexed).is_ok());
    assert_eq!(
        crate::builtins::format_declare_line("v", &sh.vars["v"]),
        "declare -a v=([0]=\"1\")"
    );

    // And a refusal still wins over the discard.
    let mut sh = Shell::new();
    sh.replace_associative("v", vec![("k".to_string(), "1".to_string())])
        .unwrap();
    assert!(matches!(
        sh.reshape_in_frame_to("v", Shape::Indexed),
        Err(DeclareErr::AssociativeExists)
    ));
    assert_eq!(
        crate::builtins::format_declare_line("v", &sh.vars["v"]),
        "declare -A v=([k]=\"1\" )"
    );
}
