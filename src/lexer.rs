#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
    And,            // &&
    Or,             // ||
    Semi,           // ;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Op(Operator),
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                tokens.push(Token::Word(std::mem::take(&mut current)));
                has_token = false;
            }
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\')) => current.push(esc),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                has_token = true;
                match chars.next() {
                    Some(ch) => current.push(ch),
                    None => current.push('\\'),
                }
            }
            '|' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                }
            }
            '&' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    return Err(LexError::BareAmpersand);
                }
            }
            ';' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
            }
            // `2>` / `2>>` — only when the `2` would otherwise start a new
            // word (no current word being built). A `2` inside or appended to
            // a word, e.g. `x2>f`, is ordinary text.
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next(); // consume the '>'
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        tokens.push(Token::Word(current));
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an expected token list made entirely of words.
    fn words(parts: &[&str]) -> Vec<Token> {
        parts.iter().map(|w| Token::Word(w.to_string())).collect()
    }

    // ----- existing v2 lexer tests (must keep passing) -----

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), words(&["ls", "-la"]));
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_single_quotes() {
        assert_eq!(
            tokenize("echo 'hello world'").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quotes() {
        assert_eq!(
            tokenize("echo \"hello world\"").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), words(&["echo", "a\"b"]));
    }

    #[test]
    fn tokenize_backslash_escape_outside_quotes() {
        assert_eq!(tokenize(r"echo a\ b").unwrap(), words(&["echo", "a b"]));
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize(r"echo a\").unwrap(), words(&["echo", r"a\"]));
    }

    #[test]
    fn tokenize_adjacent_runs_concatenate() {
        assert_eq!(tokenize(r#"foo"bar baz""#).unwrap(), words(&["foobar baz"]));
    }

    #[test]
    fn tokenize_single_quotes_preserve_backslash() {
        assert_eq!(tokenize(r"echo 'a\b'").unwrap(), words(&["echo", r"a\b"]));
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        assert_eq!(tokenize("''").unwrap(), words(&[""]));
    }

    #[test]
    fn tokenize_unterminated_single_quote() {
        assert_eq!(
            tokenize("echo 'oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_unterminated_double_quote() {
        assert_eq!(
            tokenize("echo \"oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_pipe_with_spaces() {
        assert_eq!(
            tokenize("a | b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirAppend),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![
                Token::Word("cat".to_string()),
                Token::Op(Operator::RedirIn),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![
                Token::Word("cmd".to_string()),
                Token::Op(Operator::RedirErr),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![
                Token::Word("cmd".to_string()),
                Token::Op(Operator::RedirErrAppend),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![
                Token::Word("x2".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_two_not_followed_by_redirect_is_a_word() {
        assert_eq!(tokenize("2 foo").unwrap(), words(&["2", "foo"]));
    }

    #[test]
    fn tokenize_quoted_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "|" ">""#).unwrap(),
            words(&["echo", "|", ">"])
        );
    }

    #[test]
    fn tokenize_escaped_operators_stay_words() {
        assert_eq!(tokenize(r"echo \| \>").unwrap(), words(&["echo", "|", ">"]));
    }

    #[test]
    fn tokenize_pipeline_with_redirects() {
        assert_eq!(
            tokenize("a < in | b > out").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::RedirIn),
                Token::Word("in".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("out".to_string()),
            ]
        );
    }

    // ----- new: sequencing operators -----

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Or),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Or),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_is_error() {
        assert_eq!(tokenize("a & b").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_error() {
        assert_eq!(tokenize("a &").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_semicolon_with_spaces() {
        assert_eq!(
            tokenize("a ; b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_quoted_sequencing_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "&&" "||" ";""#).unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_escaped_sequencing_operators_stay_words() {
        // `\&` is just `&` (literal); two `\&` make `&&` (the literal word, not the op).
        assert_eq!(
            tokenize(r"echo \&\& \|\| \;").unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_combined_sequencing_operators() {
        assert_eq!(
            tokenize("a && b || c ; d").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
                Token::Op(Operator::Or),
                Token::Word("c".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("d".to_string()),
            ]
        );
    }
}
