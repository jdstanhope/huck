//! Alias expansion. Runs after tokenize, before parse. Substitutes
//! aliases at command position with cycle protection and the bash
//! trailing-space rule.

use std::collections::{HashMap, HashSet};

use crate::lexer::{LexError, Operator, Token, Word, WordPart};

/// Walks `tokens`, substituting alias definitions at command
/// position. Recursive substitution is cycle-protected via a
/// per-input `active` set. The trailing-space rule applies: if an
/// alias body ends with whitespace, the token immediately following
/// the expansion is itself alias-eligible.
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    let mut out: Vec<Token> = Vec::new();
    let mut next_eligible = true;
    let mut active: HashSet<String> = HashSet::new();
    for token in tokens {
        next_eligible = process_token(token, &mut out, next_eligible, aliases, &mut active)?;
    }
    Ok(out)
}

fn process_token(
    token: Token,
    out: &mut Vec<Token>,
    eligible: bool,
    aliases: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> Result<bool, LexError> {
    match &token {
        Token::Word(w) => {
            if eligible
                && let Some(name) = simple_word_text(w)
                && !active.contains(&name)
                && let Some(body) = aliases.get(&name).cloned()
            {
                active.insert(name.clone());
                let inner_tokens = crate::lexer::tokenize(&body)?;
                let mut inner_eligible = true;
                for inner in inner_tokens {
                    inner_eligible = process_token(
                        inner,
                        out,
                        inner_eligible,
                        aliases,
                        active,
                    )?;
                }
                active.remove(&name);
                let trailing = body
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_whitespace());
                return Ok(trailing);
            }
            out.push(token);
            Ok(false)
        }
        Token::Op(op) => {
            let separator = matches!(
                op,
                Operator::Pipe
                    | Operator::And
                    | Operator::Or
                    | Operator::Semi
                    | Operator::Background
                    | Operator::LParen
            );
            out.push(token);
            Ok(separator)
        }
        Token::Newline => {
            out.push(token);
            Ok(true)
        }
        _ => {
            out.push(token);
            Ok(eligible)
        }
    }
}

/// Returns the concatenated literal text of a Word iff every part is
/// an unquoted Literal. Returns None for any quoted, Var, Arith,
/// CommandSub, or Tilde part — aliases only expand from plain
/// unquoted identifiers.
fn simple_word_text(w: &Word) -> Option<String> {
    let mut text = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text: t, quoted: false } => text.push_str(t),
            _ => return None,
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn make_aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Compare two token streams by re-tokenizing the expected source
    /// (avoids hand-constructing complex Token::Word values).
    fn assert_tokens_eq(actual: &[Token], expected_source: &str) {
        let expected = tokenize(expected_source).expect("expected source must tokenize");
        assert_eq!(actual, &expected[..], "actual:\n  {:?}\nexpected:\n  {:?}", actual, expected);
    }

    #[test]
    fn simple_expansion() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls -l");
    }

    #[test]
    fn no_expansion_outside_command_position() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("echo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "echo ll");
    }

    #[test]
    fn recursive_expansion() {
        let aliases = make_aliases(&[("ls", "ls --color"), ("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls --color -l");
    }

    #[test]
    fn cycle_protection() {
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("ls").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // Only one substitution — the inner `ls` is in `active` and
        // does not re-expand.
        assert_tokens_eq(&out, "ls --color");
    }

    #[test]
    fn expansion_after_pipe() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("cat | ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "cat | ls -l");
    }

    #[test]
    fn expansion_after_semi() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("true; ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "true; ls -l");
    }

    #[test]
    fn trailing_space_chains_expansion() {
        // Note the trailing space in the `sudo` body.
        let aliases = make_aliases(&[("sudo", "sudo "), ("ll", "ls -l")]);
        let toks = tokenize("sudo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "sudo ls -l");
    }

    #[test]
    fn quoted_word_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("'ll'").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // `'ll'` is a quoted Literal — `simple_word_text` returns None
        // because `quoted: true`. So no expansion fires.
        assert_eq!(out, tokenize("'ll'").unwrap());
    }
}
