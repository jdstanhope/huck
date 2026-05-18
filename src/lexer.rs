#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
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
    Background,     // &
}

#[derive(Debug, PartialEq, Eq)]
pub enum TildeSpec {
    Home,
    User(String),
    Pwd,
    OldPwd,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
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
                        Some('`') => {
                            // Backtick substitution inside double quotes (quoted: true).
                            if !current.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut current)));
                            }
                            let sequence = scan_backtick_substitution(&mut chars)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
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
            '~' if !has_token => {
                if let Some(spec) = try_parse_tilde(&mut chars) {
                    has_token = true;
                    parts.push(WordPart::Tilde(spec));
                } else {
                    // Fall through: treat '~' as literal.
                    current.push('~');
                    has_token = true;
                }
            }
            '`' => {
                has_token = true;
                if !current.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut current)));
                }
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
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
                    tokens.push(Token::Op(Operator::Background));
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
        Some('(') => {
            chars.next(); // consume '('
            let sequence = scan_paren_substitution(chars)?;
            parts.push(WordPart::CommandSub { sequence, quoted });
        }
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

/// Reads the body of a `$(...)` substitution. The opening `$(` is already
/// consumed; this function consumes through the matching `)` at depth 0.
/// Tracks quote and escape state so that `)` inside `'...'`, `"..."`, or
/// after `\` does not close the substitution, and nested `$(...)` increments
/// the depth.
fn scan_paren_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    let mut depth: usize = 0;
    while let Some(c) = chars.next() {
        match c {
            ')' if depth == 0 => {
                return parse_substitution_body(&body);
            }
            ')' => {
                depth -= 1;
                body.push(c);
            }
            '(' => {
                // Bare `(` is just a character. huck has no subshell
                // `(cmd)` syntax — only `$(` increments depth (handled in
                // the `$` arm below).
                body.push(c);
            }
            '\\' => {
                body.push(c);
                if let Some(next) = chars.next() {
                    body.push(next);
                } else {
                    return Err(LexError::UnterminatedSubstitution);
                }
            }
            '\'' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('\'') => {
                            body.push('\'');
                            break;
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '"' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('"') => {
                            body.push('"');
                            break;
                        }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(next) = chars.next() {
                                body.push(next);
                            } else {
                                return Err(LexError::UnterminatedSubstitution);
                            }
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '$' => {
                body.push(c);
                if let Some(&next) = chars.peek() {
                    if next == '(' {
                        chars.next();
                        body.push('(');
                        depth += 1;
                    }
                }
            }
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}

/// Tokenizes and parses a substitution body, wrapping any errors with the
/// substitution-context `LexError` variants. Empty bodies (whitespace only)
/// produce an empty `Sequence`.
fn parse_substitution_body(body: &str) -> Result<crate::command::Sequence, LexError> {
    let tokens = tokenize(body).map_err(|e| LexError::SubstitutionLexError(Box::new(e)))?;
    let parsed = crate::command::parse(tokens).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}

/// Reads the body of a `` `...` `` substitution. The opening backtick is
/// already consumed; this function consumes through the matching unescaped
/// backtick. Applies bash's backtick escape rules:
/// - `\` + `` ` `` -> literal `` ` `` in the body
/// - `\` + `\` -> literal `\` in the body
/// - `\` + `$` -> literal `$` in the body
/// - `\` + any other char `c` -> both `\` and `c` are preserved verbatim
fn scan_backtick_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                return parse_substitution_body(&body);
            }
            '\\' => match chars.next() {
                Some('`') => body.push('`'),
                Some('\\') => body.push('\\'),
                Some('$') => body.push('$'),
                Some(other) => {
                    body.push('\\');
                    body.push(other);
                }
                None => return Err(LexError::UnterminatedSubstitution),
            },
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}

fn empty_sequence() -> crate::command::Sequence {
    crate::command::Sequence {
        first: crate::command::Pipeline { commands: Vec::new() },
        rest: Vec::new(),
        background: false,
    }
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

/// Tries to consume a tilde construct starting just after the `~`.
/// On success, returns the `TildeSpec` (consuming any extra chars, e.g.
/// the `+` in `~+`). On failure, leaves the iterator untouched and
/// returns `None` (the caller treats `~` as a literal).
fn try_parse_tilde(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Option<TildeSpec> {
    match chars.peek().copied() {
        // Bare ~ at end of word.
        None => Some(TildeSpec::Home),
        Some(c) if is_tilde_terminator(c) => Some(TildeSpec::Home),
        // ~+, ~- — must be followed by terminator (or nothing).
        Some('+') => {
            let mut lookahead = chars.clone();
            lookahead.next(); // consume the +
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::Pwd) }
                Some(c) if is_tilde_terminator(c) => { chars.next(); Some(TildeSpec::Pwd) }
                _ => None,
            }
        }
        Some('-') => {
            let mut lookahead = chars.clone();
            lookahead.next();
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::OldPwd) }
                Some(c) if is_tilde_terminator(c) => { chars.next(); Some(TildeSpec::OldPwd) }
                _ => None,
            }
        }
        Some(c) if is_user_name_start(c) => {
            // Scan a maximal identifier; the tail after must be a terminator.
            let mut lookahead = chars.clone();
            let mut name = String::new();
            while let Some(&nc) = lookahead.peek() {
                if is_user_name_continue(nc) {
                    name.push(nc);
                    lookahead.next();
                } else {
                    break;
                }
            }
            let tail_ok = match lookahead.peek().copied() {
                None => true,
                Some(c) => is_tilde_terminator(c),
            };
            if tail_ok && !name.is_empty() {
                // Consume the scanned chars from the real iterator.
                // Safe: is_user_name_start/continue only accept ASCII, so
                // name.len() (bytes) equals the char count.
                for _ in 0..name.len() {
                    chars.next();
                }
                Some(TildeSpec::User(name))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_tilde_terminator(c: char) -> bool {
    c == '/'
        || c.is_whitespace()
        || matches!(c, '|' | '<' | '>' | '&' | ';')
}

fn is_user_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_user_name_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
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
    fn tokenize_bare_ampersand_is_background_op() {
        assert_eq!(
            tokenize("a & b").unwrap(),
            vec![w("a"), Token::Op(Operator::Background), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_background_op() {
        assert_eq!(
            tokenize("a &").unwrap(),
            vec![w("a"), Token::Op(Operator::Background)]
        );
    }

    #[test]
    fn tokenize_double_ampersand_still_and_op() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_two_separate_backgrounds() {
        assert_eq!(
            tokenize("a & &").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]
        );
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
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Home)]))]
        );
    }

    #[test]
    fn tokenize_tilde_slash_path() {
        assert_eq!(
            tokenize("~/foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::Home),
                WordPart::Literal("/foo".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_mid_word_is_literal() {
        assert_eq!(tokenize("a~b").unwrap(), words(&["a~b"]));
    }

    #[test]
    fn tokenize_tilde_followed_by_name_is_user_form() {
        assert_eq!(
            tokenize("~foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("foo".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_alone() {
        assert_eq!(
            tokenize("~alice").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_slash_path() {
        assert_eq!(
            tokenize("~alice/bin").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
                WordPart::Literal("/bin".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_with_underscore_and_digits() {
        assert_eq!(
            tokenize("~alice_123").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice_123".to_string())),
            ]))]
        );
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

    #[test]
    fn tokenize_escaped_dollar_in_double_quotes_is_literal() {
        assert_eq!(tokenize(r#""\$FOO""#).unwrap(), words(&["$FOO"]));
    }

    #[test]
    fn tokenize_two_adjacent_vars() {
        assert_eq!(
            tokenize("$FOO$BAR").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Var { name: "BAR".to_string(), quoted: false },
            ]))]
        );
    }

    fn sub_word(parts: Vec<WordPart>) -> Token {
        Token::Word(Word(parts))
    }

    fn echo_seq(args: &[&str]) -> crate::command::Sequence {
        use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: Word(vec![WordPart::Literal("echo".to_string())]),
                    args: args
                        .iter()
                        .map(|a| Word(vec![WordPart::Literal(a.to_string())]))
                        .collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn tokenize_command_sub_basic() {
        assert_eq!(
            tokenize("$(echo hi)").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_quoted_in_double_quotes() {
        assert_eq!(
            tokenize("\"$(echo hi)\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'$(echo hi)'").unwrap(),
            words(&["$(echo hi)"])
        );
    }

    #[test]
    fn tokenize_command_sub_empty() {
        assert_eq!(
            tokenize("$()").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: crate::command::Sequence {
                    first: crate::command::Pipeline { commands: vec![] },
                    rest: vec![],
                    background: false,
                },
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_with_paren_inside_double_quotes() {
        // The `)` inside `"..."` does not close the substitution.
        assert_eq!(
            tokenize("$(echo \")\")").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&[")"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_nested() {
        // Outer body is `echo $(echo hi)`; inner is `echo hi`.
        let inner = echo_seq(&["hi"]);
        let inner_word = Word(vec![WordPart::CommandSub {
            sequence: inner,
            quoted: false,
        }]);
        let outer = {
            use crate::command::{ExecCommand, Pipeline, Sequence, SimpleCommand};
            Sequence {
                first: Pipeline {
                    commands: vec![SimpleCommand::Exec(ExecCommand {
                        program: Word(vec![WordPart::Literal("echo".to_string())]),
                        args: vec![inner_word],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                    })],
                },
                rest: vec![],
                background: false,
            }
        };
        assert_eq!(
            tokenize("$(echo $(echo hi))").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: outer,
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_unterminated() {
        assert_eq!(
            tokenize("$(echo").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_command_sub_inner_lex_error() {
        // `${1foo}` inside a substitution → InvalidVarName, wrapped.
        let err = tokenize("$(echo ${1foo})").unwrap_err();
        match err {
            LexError::SubstitutionLexError(inner) => {
                assert_eq!(*inner, LexError::InvalidVarName);
            }
            other => panic!("expected SubstitutionLexError, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_inner_parse_error() {
        // `echo |` inside the body → MissingCommand from the parser, wrapped.
        let err = tokenize("$(echo |)").unwrap_err();
        match err {
            LexError::SubstitutionParseError(inner) => {
                assert_eq!(inner, crate::command::ParseError::MissingCommand);
            }
            other => panic!("expected SubstitutionParseError, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_as_program() {
        // `$(echo ls) -la` — the program word is itself a CommandSub.
        let tokens = tokenize("$(echo ls) -la").unwrap();
        assert_eq!(tokens.len(), 2);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
        assert_eq!(tokens[1], w("-la"));
    }

    #[test]
    fn tokenize_command_sub_concatenates_with_literal() {
        // `pre$(echo x)post` → one Word with three parts: Literal, CommandSub, Literal
        let tokens = tokenize("pre$(echo x)post").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], WordPart::Literal(ref s) if s == "pre"));
                assert!(matches!(parts[1], WordPart::CommandSub { .. }));
                assert!(matches!(parts[2], WordPart::Literal(ref s) if s == "post"));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_in_redirect_target() {
        let tokens = tokenize("cat > $(echo /tmp/f)").unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], w("cat"));
        assert_eq!(tokens[1], Token::Op(Operator::RedirOut));
        match &tokens[2] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_basic() {
        assert_eq!(
            tokenize("`echo hi`").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_in_double_quotes_is_quoted() {
        assert_eq!(
            tokenize("\"`echo hi`\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_escape_dollar() {
        // `\$FOO` inside backticks → inner body is `$FOO` (the `\$` unescapes
        // before the inner tokenizer sees it). So the inner Sequence has a
        // single command whose first arg expands $FOO.
        let tokens = tokenize("`echo \\$FOO`").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    WordPart::CommandSub { sequence, quoted: false } => {
                        // Inner: echo $FOO → second word's first part is a Var
                        let inner_cmd = &sequence.first.commands[0];
                        match inner_cmd {
                            crate::command::SimpleCommand::Exec(e) => {
                                assert_eq!(e.args.len(), 1);
                                match &e.args[0].0[0] {
                                    WordPart::Var { name, quoted: false } => {
                                        assert_eq!(name, "FOO");
                                    }
                                    other => panic!("expected Var(FOO), got {other:?}"),
                                }
                            }
                            other => panic!("expected Exec, got {other:?}"),
                        }
                    }
                    other => panic!("expected CommandSub, got {other:?}"),
                }
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_escape_backslash() {
        // `\\` inside backticks → inner body is `\`. Inner tokenize sees
        // a trailing backslash; treats it as a literal.
        let tokens = tokenize("`echo \\\\`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    match &sequence.first.commands[0] {
                        crate::command::SimpleCommand::Exec(e) => {
                            // Inner body was `echo \` — backslash at end is literal.
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal(s) => assert_eq!(s, "\\"),
                                other => panic!("expected Literal(\\\\), got {other:?}"),
                            }
                        }
                        other => panic!("expected Exec, got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unescaped_other_backslash_preserved() {
        // `\n` inside backticks → body has `\n` (backslash + n), which the
        // inner tokenize treats as an escape (literal `n`).
        let tokens = tokenize("`echo \\n`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    match &sequence.first.commands[0] {
                        crate::command::SimpleCommand::Exec(e) => {
                            // Inner body `echo \n` — outer tokenizer's `\n` becomes `n`
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal(s) => assert_eq!(s, "n"),
                                other => panic!("expected Literal(n), got {other:?}"),
                            }
                        }
                        other => panic!("expected Exec, got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unterminated() {
        assert_eq!(
            tokenize("`echo hi").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_backtick_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'`echo hi`'").unwrap(),
            words(&["`echo hi`"])
        );
    }

    #[test]
    fn tokenize_tilde_plus_alone() {
        assert_eq!(
            tokenize("~+").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Pwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_alone() {
        assert_eq!(
            tokenize("~-").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_plus_slash_path() {
        assert_eq!(
            tokenize("~+/x").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::Pwd),
                WordPart::Literal("/x".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_slash_path() {
        assert_eq!(
            tokenize("~-/x").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::OldPwd),
                WordPart::Literal("/x".to_string()),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_plus_followed_by_letter_is_literal() {
        // ~+abc is not a valid form; falls back to literal.
        assert_eq!(tokenize("~+abc").unwrap(), words(&["~+abc"]));
    }
}
