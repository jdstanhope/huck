#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
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
    let mut parts: Vec<WordPart> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current);
                tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
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
                            Some('$') => current.push('$'), // `\$` -> literal $
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            if !current.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut current)));
                            }
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
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
            '$' => {
                // Expansion outside any quotes (quoted: false).
                has_token = true;
                if !current.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut current)));
                }
                read_dollar_expansion(&mut chars, &mut parts, false)?;
            }
            '~' if !has_token && tilde_at_word_start(&chars) => {
                has_token = true;
                parts.push(WordPart::Tilde);
            }
            '|' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
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
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
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
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
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
        flush_literal(&mut parts, &mut current);
        tokens.push(Token::Word(Word(parts)));
    }
    Ok(tokens)
}

fn flush_literal(parts: &mut Vec<WordPart>, current: &mut String) {
    if !current.is_empty() {
        parts.push(WordPart::Literal(std::mem::take(current)));
    } else if parts.is_empty() {
        // The token exists (e.g. from `""`) but no literal text has accumulated.
        // Push an empty Literal so expansion's `has_emitted` fires.
        parts.push(WordPart::Literal(String::new()));
    }
}

/// Reads what follows a `$`. Pushes the resulting WordPart onto `parts` or
/// (for an unrecognized form) pushes a literal `$` and lets the caller
/// continue tokenizing.
fn read_dollar_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('{') => {
            chars.next();
            let name = read_braced_var_name(chars)?;
            parts.push(WordPart::Var { name, quoted });
        }
        Some('?') => {
            chars.next();
            parts.push(WordPart::LastStatus { quoted });
        }
        Some(c) if is_name_start(c) => {
            let name = read_var_name(chars);
            parts.push(WordPart::Var { name, quoted });
        }
        _ => {
            parts.push(WordPart::Literal("$".to_string()));
        }
    }
    Ok(())
}

fn read_var_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if is_name_cont(c) {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    name
}

fn read_braced_var_name(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut name = String::new();
    let first = chars.next().ok_or(LexError::UnterminatedBrace)?;
    if !is_name_start(first) {
        // Drain until '}' so the error is recoverable on the REPL.
        if first != '}' {
            loop {
                match chars.next() {
                    Some('}') => break,
                    Some(_) => continue,
                    None => return Err(LexError::UnterminatedBrace),
                }
            }
        }
        return Err(LexError::InvalidVarName);
    }
    name.push(first);
    loop {
        match chars.next() {
            Some('}') => return Ok(name),
            Some(c) if is_name_cont(c) => name.push(c),
            Some(_) => {
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(_) => continue,
                        None => return Err(LexError::UnterminatedBrace),
                    }
                }
                return Err(LexError::InvalidVarName);
            }
            None => return Err(LexError::UnterminatedBrace),
        }
    }
}

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_name_cont(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// True iff a `~` would expand here: next char is `/`, whitespace, an
/// operator metachar (`|`, `<`, `>`, `&`, `;`), or end of input.
fn tilde_at_word_start(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    match chars.clone().peek() {
        None => true,
        Some(&c) => {
            c == '/'
                || c.is_whitespace()
                || matches!(c, '|' | '<' | '>' | '&' | ';')
        }
    }
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

    fn vword_unquoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: false,
        }]))
    }

    fn vword_quoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: true,
        }]))
    }

    #[test]
    fn tokenize_dollar_var_unquoted() {
        assert_eq!(tokenize("$FOO").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_braced() {
        assert_eq!(tokenize("${FOO}").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_double_quotes_is_quoted() {
        assert_eq!(tokenize("\"$FOO\"").unwrap(), vec![vword_quoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_single_quotes_is_literal() {
        assert_eq!(tokenize("'$FOO'").unwrap(), words(&["$FOO"]));
    }

    #[test]
    fn tokenize_last_status() {
        assert_eq!(
            tokenize("$?").unwrap(),
            vec![Token::Word(Word(vec![WordPart::LastStatus {
                quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_then_digit_is_literal_dollar() {
        assert_eq!(
            tokenize("$5").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("$".to_string()),
                WordPart::Literal("5".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_double_dollar_is_two_literal_dollars() {
        assert_eq!(
            tokenize("$$").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("$".to_string()),
                WordPart::Literal("$".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_alone() {
        assert_eq!(
            tokenize("~").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde]))]
        );
    }

    #[test]
    fn tokenize_tilde_slash_path() {
        assert_eq!(
            tokenize("~/foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde,
                WordPart::Literal("/foo".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_mid_word_is_literal() {
        assert_eq!(tokenize("a~b").unwrap(), words(&["a~b"]));
    }

    #[test]
    fn tokenize_tilde_followed_by_name_is_literal() {
        assert_eq!(tokenize("~foo").unwrap(), words(&["~foo"]));
    }

    #[test]
    fn tokenize_tilde_in_quotes_is_literal() {
        assert_eq!(tokenize("\"~\"").unwrap(), words(&["~"]));
    }

    #[test]
    fn tokenize_braced_var_invalid_name() {
        assert_eq!(tokenize("${1foo}").unwrap_err(), LexError::InvalidVarName);
    }

    #[test]
    fn tokenize_braced_var_empty_name() {
        assert_eq!(tokenize("${}").unwrap_err(), LexError::InvalidVarName);
    }

    #[test]
    fn tokenize_unterminated_brace() {
        assert_eq!(tokenize("${FOO").unwrap_err(), LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_var_concatenates_with_literal() {
        assert_eq!(
            tokenize("a$FOOb").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal("a".to_string()),
                WordPart::Var { name: "FOOb".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_braced_var_separates_from_following_word() {
        assert_eq!(
            tokenize("${FOO}bar").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Literal("bar".to_string()),
            ]))]
        );
    }
}
