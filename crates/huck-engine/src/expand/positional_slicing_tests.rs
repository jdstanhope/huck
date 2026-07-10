//! Task 3 closes v33's `${@:o:l}` / `${*:o:l}` deferral. These
//! tests drive the slice helper through the lex→expand pipeline.

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

fn shell_with_posargs() -> Shell {
    let mut s = Shell::new();
    s.positional_args = vec!["a".into(), "b".into(), "c".into(), "d".into()];
    s
}

#[test]
fn at_slice_positive() {
    let mut s = shell_with_posargs();
    let words = expand_to_word_list_for_test(&mut s, r#""${@:2:2}""#);
    assert_eq!(words, vec!["b", "c"]);
}

#[test]
fn at_slice_negative_offset() {
    let mut s = shell_with_posargs();
    let words = expand_to_word_list_for_test(&mut s, r#""${@: -2}""#);
    assert_eq!(words, vec!["c", "d"]);
}

#[test]
fn star_slice_joins_by_ifs() {
    let mut s = shell_with_posargs();
    let out = expand_for_test(&mut s, r#""${*:1:3}""#);
    assert_eq!(out, "a b c");
}

#[test]
fn at_slice_offset_zero_includes_dollar_zero() {
    let mut s = shell_with_posargs();
    s.shell_argv0 = "huck".to_string();
    let words = expand_to_word_list_for_test(&mut s, r#""${@:0:2}""#);
    // Bash returns "huck a" for ${@:0:2} when $0 is "huck" and positionals are [a,b,c,d].
    assert_eq!(words, vec!["huck", "a"]);
}

#[test]
fn at_slice_negative_length_indexes_from_end() {
    let mut s = shell_with_posargs();
    let words = expand_to_word_list_for_test(&mut s, r#""${@:1:-1}""#);
    // Bash: ${@:1:-1} starts at $1, ends one-before-last. Returns ["a", "b", "c"].
    assert_eq!(words, vec!["a", "b", "c"]);
}
