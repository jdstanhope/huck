//! Task 3 (v71) end-to-end array expansion + positional slicing.
//!
//! Tests drive the full lex→Word→expand pipeline so the lexer's
//! subscript handling and the new `expand_array_param` /
//! `slice_word_list` paths are exercised together.

use super::*;
use crate::command::{Command, SimpleCommand};
use crate::shell_state::Shell;

/// Lex the input as `echo <input>` and return the first argument
/// Word. Avoids constructing `WordPart::ParamExpansion` literals by
/// hand and keeps the tests aligned with what the lexer actually
/// produces (matters for the lexer-touching `${!a[@]}` shape).
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

/// Run `expand` on the lexed input and return a single string
/// formed by joining the resulting fields with a space. Used for
/// tests that expect a single conceptual "string" result (e.g.
/// `${a[i]}` reads, `${#a[@]}` counts, `${!a[@]}` keys, `${a[*]}`).
fn expand_for_test(shell: &mut Shell, input: &str) -> String {
    let w = first_arg_word(input);
    let fields = expand(&w, shell);
    let parts: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    parts.join(" ")
}

/// Run `expand` and return the field list directly (each field's
/// `chars` value as a Vec<String>). Used for tests that expect
/// multiple separate words from a quoted `${a[@]}` form.
fn expand_to_word_list_for_test(shell: &mut Shell, input: &str) -> Vec<String> {
    let w = first_arg_word(input);
    let fields = expand(&w, shell);
    fields.into_iter().map(|f| f.chars).collect()
}

fn shell_with_a() -> Shell {
    let mut s = Shell::new();
    s.seed_array_for_tests("a", &[(0, "x"), (1, "y"), (2, "z")]);
    s
}

#[test]
fn read_element_returns_value() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[1]}");
    assert_eq!(out, "y");
}

#[test]
fn out_of_range_element_is_empty() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[99]}");
    assert_eq!(out, "");
}

#[test]
fn quoted_at_yields_separate_words() {
    let mut s = shell_with_a();
    let words = expand_to_word_list_for_test(&mut s, r#""${a[@]}""#);
    assert_eq!(words, vec!["x", "y", "z"]);
}

#[test]
fn quoted_star_joins_by_ifs() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, r#""${a[*]}""#);
    assert_eq!(out, "x y z");
}

#[test]
fn count_returns_element_count_not_max_index() {
    let mut s = Shell::new();
    s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
    let out = expand_for_test(&mut s, "${#a[@]}");
    assert_eq!(out, "2");
}

#[test]
fn keys_list_returns_subscripts() {
    let mut s = Shell::new();
    s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
    let out = expand_for_test(&mut s, "${!a[@]}");
    assert_eq!(out, "2 5");
}

#[test]
fn element_length() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${#a[0]}");
    assert_eq!(out, "1");
}

#[test]
fn indirect_unset_positional_is_empty() {
    // v195: `${!1}` with no positional parameters expands to empty (like
    // bash), not a fatal "invalid indirect expansion". `:-` sees it unset.
    let mut s = Shell::new();
    assert_eq!(expand_for_test(&mut s, "${!1}"), "");
    assert_eq!(expand_for_test(&mut s, "${!9}"), "");
    assert_eq!(expand_for_test(&mut s, "${!1:-DEF}"), "DEF");
    // A SET positional still indirects through its value.
    s.positional_args = vec!["HOME".into()];
    assert_eq!(
        expand_for_test(&mut s, "${!1}"),
        std::env::var("HOME").unwrap_or_default()
    );
}

#[test]
fn slicing_positive_offset_and_length() {
    let mut s = shell_with_a();
    let words = expand_to_word_list_for_test(&mut s, r#""${a[@]:1:1}""#);
    assert_eq!(words, vec!["y"]);
}

#[test]
fn slicing_negative_offset_counts_from_end() {
    let mut s = shell_with_a();
    let words = expand_to_word_list_for_test(&mut s, r#""${a[@]: -1}""#);
    assert_eq!(words, vec!["z"]);
}

#[test]
fn bare_name_returns_element_zero() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a}");
    assert_eq!(out, "x");
}

#[test]
fn negative_subscript_wraps() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[-1]}");
    assert_eq!(out, "z");
}

#[test]
fn nounset_on_unset_element_fires_pe_error() {
    let mut s = shell_with_a();
    s.shell_options.nounset = true;
    let _ = expand_for_test(&mut s, "${a[99]}");
    assert!(s.pending_fatal_status.is_some());
}

#[test]
fn slicing_negative_length_indexes_from_end() {
    let mut s = shell_with_a();
    let words = expand_to_word_list_for_test(&mut s, r#""${a[@]:1:-1}""#);
    // Bash: ${a[@]:1:-1} starts at index 1, ends one-before-last. Returns ["y"].
    assert_eq!(words, vec!["y"]);
}

#[test]
fn length_of_element_at_bad_subscript_errors() {
    // ${#nonexistent[-1]} — negative subscript on an unset array
    // cannot wrap (no max index), so eval_subscript returns Err.
    // The fix to (PM::Length, SK::Index) must propagate that error
    // rather than silently using idx 0.
    let mut s = Shell::new();
    let _ = expand_for_test(&mut s, "${#nonexistent[-1]}");
    assert!(s.pending_fatal_status.is_some());
}

// v73 regression: ${a[i]:-default} on a missing index must substitute
// the default, not fall through to scalar_view (element 0). Pre-v73
// bug: get_raw saw override_value=None and consulted shell.get(name)
// which returned a[0] — so ${a[99]:-X} returned "x" (a[0]) instead of "X".
#[test]
fn modifier_on_missing_index_uses_default() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[99]:-fallback}");
    assert_eq!(out, "fallback");
}

// v73 regression: ${a[i]-default} (no colon) on a missing index also
// substitutes the default.
#[test]
fn modifier_no_colon_on_missing_index_uses_default() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[99]-fallback}");
    assert_eq!(out, "fallback");
}

// v73 regression: ${a[i]:?msg} on a missing index fires the fatal error
// rather than silently returning a[0].
#[test]
fn error_if_unset_on_missing_index_fires() {
    let mut s = shell_with_a();
    let _ = expand_for_test(&mut s, "${a[99]:?missing}");
    assert!(s.pending_fatal_status.is_some());
}

// v73 regression: ${a[i]:+alt} on a missing index returns empty (the
// alternative branch only fires when the value is set+non-null).
#[test]
fn alternative_value_on_missing_index_is_empty() {
    let mut s = shell_with_a();
    let out = expand_for_test(&mut s, "${a[99]:+ALT}");
    assert_eq!(out, "");
}

// v73 regression: ${a[i]:-default} on an existing element returns the
// element (not the default). Pin the happy path.
#[test]
fn modifier_on_existing_index_returns_element() {
    let mut s = shell_with_a(); // a=[(0,"x"),(1,"y"),(2,"z")]
    let out = expand_for_test(&mut s, "${a[1]:-fallback}");
    assert_eq!(out, "y");
}
