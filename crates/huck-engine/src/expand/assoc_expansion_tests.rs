//! v72 task 2: read paths for associative arrays. Mirrors the
//! indexed-array test module but exercises string-key semantics
//! and insertion-order iteration.

use super::*;
use crate::command::{Command, SimpleCommand};
use crate::shell_state::Shell;

fn first_arg_word(input: &str) -> Word {
    let src = format!("echo {input}");
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        &src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse")
    .expect("non-empty");
    let pipeline = match seq.first {
        Command::Pipeline(p) => p,
        other => panic!("expected Pipeline, got {other:?}"),
    };
    match &pipeline.commands[0] {
        Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
        other => panic!("expected SimpleCommand::Exec, got {other:?}"),
    }
}

fn expand_for_test(shell: &mut Shell, input: &str) -> String {
    let w = first_arg_word(input);
    let fields = expand(&w, shell);
    let parts: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    parts.join(" ")
}

fn expand_to_word_list_for_test(shell: &mut Shell, input: &str) -> Vec<String> {
    let w = first_arg_word(input);
    let fields = expand(&w, shell);
    fields.into_iter().map(|f| f.chars).collect()
}

fn shell_with_m() -> Shell {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "first".into(), "x".into())
        .unwrap();
    s.set_associative_element("m", "second".into(), "y".into())
        .unwrap();
    s.set_associative_element("m", "third".into(), "z".into())
        .unwrap();
    s
}

#[test]
fn read_element_by_string_key() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m[second]}");
    assert_eq!(out, "y");
}

#[test]
fn missing_key_is_empty() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m[nope]}");
    assert_eq!(out, "");
}

#[test]
fn quoted_at_yields_values_in_insertion_order() {
    let mut s = shell_with_m();
    let words = expand_to_word_list_for_test(&mut s, r#""${m[@]}""#);
    assert_eq!(words, vec!["x", "y", "z"]);
}

#[test]
fn quoted_star_joins_values_in_insertion_order() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, r#""${m[*]}""#);
    assert_eq!(out, "x y z");
}

#[test]
fn count_returns_pair_count() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${#m[@]}");
    assert_eq!(out, "3");
}

#[test]
fn keys_list_returns_string_keys_in_insertion_order() {
    let mut s = shell_with_m();
    let words = expand_to_word_list_for_test(&mut s, r#""${!m[@]}""#);
    assert_eq!(words, vec!["first", "second", "third"]);
}

#[test]
fn quoted_star_keys_joins_by_ifs() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, r#""${!m[*]}""#);
    assert_eq!(out, "first second third");
}

#[test]
fn element_length_for_associative() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "k".into(), "hello".into())
        .unwrap();
    let out = expand_for_test(&mut s, "${#m[k]}");
    assert_eq!(out, "5");
}

#[test]
fn slicing_returns_values_in_insertion_order() {
    let mut s = shell_with_m();
    let words = expand_to_word_list_for_test(&mut s, r#""${m[@]:1:1}""#);
    assert_eq!(words, vec!["y"]);
}

#[test]
fn bare_name_returns_empty_for_associative() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m}");
    assert_eq!(out, "");
}

#[test]
fn variable_subscript_expands_as_string() {
    let mut s = shell_with_m();
    s.set("k", "second".into());
    let out = expand_for_test(&mut s, "${m[$k]}");
    assert_eq!(out, "y");
}

#[test]
fn nounset_on_missing_key_fires_pe_error() {
    let mut s = shell_with_m();
    s.shell_options.nounset = true;
    let _ = expand_for_test(&mut s, "${m[nope]}");
    assert!(s.pending_fatal_status.is_some());
}

#[test]
fn modifier_on_missing_key_uses_default() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m[nope]:-fallback}");
    assert_eq!(out, "fallback");
}

// v73 regression: ${m[nope]-fallback} (no colon) MUST also substitute
// the default when the key is missing — previously fell through to
// scalar_view (which for associative is "" → tested non-null only
// for colon variant → returned "" instead of fallback).
#[test]
fn modifier_no_colon_on_missing_key_uses_default() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m[nope]-fallback}");
    assert_eq!(out, "fallback");
}

// v73 regression: ${m[k]:?msg} on a missing key should fire the error,
// not fall through to scalar_view.
#[test]
fn error_if_unset_on_missing_associative_key_fires() {
    let mut s = shell_with_m();
    let _ = expand_for_test(&mut s, "${m[nope]:?missing}");
    assert!(s.pending_fatal_status.is_some());
}

// v73 regression: ${m[k]:+alt} on a missing key returns empty (the
// alternative branch only fires when the value is set+non-null).
#[test]
fn alternative_value_on_missing_key_is_empty() {
    let mut s = shell_with_m();
    let out = expand_for_test(&mut s, "${m[nope]:+ALT}");
    assert_eq!(out, "");
}
