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
#[allow(dead_code)]
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
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
                tokens.push(Token::Word(Word(vec![WordPart::Literal(
                    std::mem::take(&mut current),
                )])));
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
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
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
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
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
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    tokens.push(Token::Word(Word(vec![WordPart::Literal(
                        std::mem::take(&mut current),
                    )])));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
            }
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
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
        tokens.push(Token::Word(Word(vec![WordPart::Literal(current)])));
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a Token that holds a single-Literal Word.
    fn w(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal(s.to_string())]))
    }

    /// Builds a Vec<Token> of all-Literal words.
    fn words(parts: &[&str]) -> Vec<Token> {
        parts.iter().map(|s| w(s)).collect()
    }

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
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![w("cat"), Token::Op(Operator::RedirIn), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErr), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![w("x2"), Token::Op(Operator::RedirOut), w("f")]
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
                w("a"),
                Token::Op(Operator::RedirIn),
                w("in"),
                Token::Op(Operator::Pipe),
                w("b"),
                Token::Op(Operator::RedirOut),
                w("out"),
            ]
        );
    }

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
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
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
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
                w("a"),
                Token::Op(Operator::And),
                w("b"),
                Token::Op(Operator::Or),
                w("c"),
                Token::Op(Operator::Semi),
                w("d"),
            ]
        );
    }
}
