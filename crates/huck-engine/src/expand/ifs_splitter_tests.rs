//! POSIX § 2.6.5 field-splitting unit tests for `emit_split_fields`.
//! These tests drive the splitter directly, not the lex→expand
//! pipeline, so they isolate the IFS classification logic.

use super::*;

fn run(value: &str, ifs: &str) -> Vec<String> {
    let mut current = Field::default();
    let mut result: Vec<Field> = Vec::new();
    let mut has_emitted = false;
    emit_split_fields(value, ifs, &mut current, &mut result, &mut has_emitted);
    if has_emitted {
        result.push(current);
    }
    result.into_iter().map(|f| f.chars).collect()
}

#[test]
fn default_ifs_collapses_whitespace_runs() {
    assert_eq!(run("a  b\tc", " \t\n"), vec!["a", "b", "c"]);
}

#[test]
fn colon_ifs_preserves_empty_between() {
    assert_eq!(run("a::b", ":"), vec!["a", "", "b"]);
}

#[test]
fn colon_ifs_leading_produces_empty() {
    assert_eq!(run(":a", ":"), vec!["", "a"]);
}

#[test]
fn colon_ifs_trailing_no_empty() {
    // POSIX: trailing non-ws IFS does NOT add a trailing empty field.
    assert_eq!(run("a:", ":"), vec!["a"]);
}

#[test]
fn mixed_ifs_ws_collapses_around_nonws() {
    // IFS=" :", value "a : b" → 2 fields (colon plus adjacent
    // spaces collapse to one separator).
    assert_eq!(run("a : b", " :"), vec!["a", "b"]);
}

#[test]
fn empty_ifs_no_split() {
    assert_eq!(run("a b c", ""), vec!["a b c"]);
}

#[test]
fn whitespace_only_value_yields_no_fields() {
    let empty: Vec<String> = Vec::new();
    assert_eq!(run("   ", " \t\n"), empty);
}

#[test]
fn mixed_consecutive_nonws_yields_empty_field() {
    // IFS=":,", value "a:,b" → a/""/"b"
    assert_eq!(run("a:,b", ":,"), vec!["a", "", "b"]);
}

#[test]
fn single_nonws_only_yields_empty_field() {
    // IFS=":", value ":" → 1 empty field
    assert_eq!(run(":", ":"), vec![""]);
}

#[test]
fn leading_nonws_then_value() {
    assert_eq!(run(":x", ":"), vec!["", "x"]);
}

#[test]
fn ws_only_ifs_pure_whitespace_collapses() {
    assert_eq!(run(" a b ", " "), vec!["a", "b"]);
}

#[test]
fn nonws_ifs_with_ws_value_no_split() {
    // IFS=":" (no whitespace), value "a b" → 1 field "a b".
    assert_eq!(run("a b", ":"), vec!["a b"]);
}

#[test]
fn empty_value_emits_nothing() {
    let empty: Vec<String> = Vec::new();
    assert_eq!(run("", ":"), empty);
    assert_eq!(run("", " \t\n"), empty);
}

#[test]
fn current_field_continuation() {
    // If `current` already has text, the first split fragment
    // continues it rather than starting a new field.
    let mut current = Field::default();
    current.push_str("prefix-", false);
    let mut result: Vec<Field> = Vec::new();
    let mut has_emitted = true;
    emit_split_fields(
        "a b c",
        " \t\n",
        &mut current,
        &mut result,
        &mut has_emitted,
    );
    result.push(current);
    let words: Vec<String> = result.into_iter().map(|f| f.chars).collect();
    assert_eq!(words, vec!["prefix-a", "b", "c"]);
}
