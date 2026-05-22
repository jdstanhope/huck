use crate::lexer::{Operator, Token, Word, WordPart};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Keyword {
    If,
    Then,
    Elif,
    Else,
    Fi,
    While,
    Until,
    Do,
    Done,
    For,
    In,
}

impl Keyword {
    fn name(self) -> &'static str {
        match self {
            Keyword::If => "if",
            Keyword::Then => "then",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::Fi => "fi",
            Keyword::While => "while",
            Keyword::Until => "until",
            Keyword::Do => "do",
            Keyword::Done => "done",
            Keyword::For => "for",
            Keyword::In => "in",
        }
    }
}

/// Returns the keyword a token represents, or `None`. A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.
fn keyword_of(token: &Token) -> Option<Keyword> {
    let Token::Word(Word(parts)) = token else { return None };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &parts[0] else {
        return None;
    };
    match text.as_str() {
        "if" => Some(Keyword::If),
        "then" => Some(Keyword::Then),
        "elif" => Some(Keyword::Elif),
        "else" => Some(Keyword::Else),
        "fi" => Some(Keyword::Fi),
        "while" => Some(Keyword::While),
        "until" => Some(Keyword::Until),
        "do" => Some(Keyword::Do),
        "done" => Some(Keyword::Done),
        "for" => Some(Keyword::For),
        "in" => Some(Keyword::In),
        _ => None,
    }
}

/// If `word` looks like `NAME=value` (a leading `Literal` whose text begins
/// with a valid identifier followed by `=`), returns `Ok((name, value))`
/// where `value` is a `Word` containing the rest of the prefix Literal
/// followed by the remaining original parts (moved, not cloned). Otherwise
/// returns `Err(word)` handing the original back unchanged.
fn try_split_assignment(
    word: crate::lexer::Word,
) -> Result<(String, crate::lexer::Word), crate::lexer::Word> {
    use crate::lexer::WordPart;
    let first = match word.0.first() {
        Some(p) => p,
        None => return Err(word),
    };
    let text = match first {
        WordPart::Literal { text, .. } => text,
        _ => return Err(word),
    };
    let Some(eq) = text.find('=') else {
        return Err(word);
    };
    let name_slice = &text[..eq];
    if name_slice.is_empty() {
        return Err(word);
    }
    let mut name_chars = name_slice.chars();
    let Some(first_ch) = name_chars.next() else {
        return Err(word);
    };
    if !(first_ch == '_' || first_ch.is_ascii_alphabetic()) {
        return Err(word);
    }
    if !name_chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(word);
    }

    // Validation passed — destructure the word, moving parts into the value.
    let crate::lexer::Word(mut parts) = word;
    let first_part = parts.remove(0);
    let text = match first_part {
        WordPart::Literal { text, quoted } => {
            debug_assert!(
                !quoted,
                "assignment-eligible first Literal must be unquoted; lexer's `=` arm only fires while accumulating unquoted text"
            );
            text
        }
        _ => unreachable!("checked above"),
    };
    let (name, rest_of_first) = (text[..eq].to_string(), text[eq + 1..].to_string());
    let mut value_parts: Vec<WordPart> = Vec::with_capacity(parts.len() + 1);
    value_parts.push(WordPart::Literal { text: rest_of_first, quoted: false });
    value_parts.extend(parts);
    Ok((name, crate::lexer::Word(value_parts)))
}

fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    stdin: Option<crate::lexer::Word>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
) -> SimpleCommand {
    if args.is_empty() && stdin.is_none() && stdout.is_none() && stderr.is_none() {
        match try_split_assignment(program) {
            Ok((name, value)) => return SimpleCommand::Assign { name, value },
            Err(restored) => {
                return SimpleCommand::Exec(ExecCommand {
                    program: restored,
                    args,
                    stdin,
                    stdout,
                    stderr,
                });
            }
        }
    }
    SimpleCommand::Exec(ExecCommand {
        program,
        args,
        stdin,
        stdout,
        stderr,
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Redirect {
    Truncate(Word),
    Append(Word),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecCommand {
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SimpleCommand {
    Assign { name: String, value: Word },
    Exec(ExecCommand),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi,
    And,
    Or,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IfClause {
    pub condition: Sequence,
    pub then_body: Sequence,
    pub elif_branches: Vec<ElifBranch>,
    pub else_body: Option<Sequence>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WhileClause {
    pub condition: Sequence,
    pub body: Sequence,
    pub until: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ForClause {
    /// The loop variable name — a validated identifier.
    pub var: String,
    /// The unexpanded `in` word list. Empty for the no-`in` form.
    pub words: Vec<Word>,
    /// The do…done body.
    pub body: Sequence,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ElifBranch {
    pub condition: Sequence,
    pub body: Sequence,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Sequence {
    pub first: Command,
    pub rest: Vec<(Connector, Command)>,
    pub background: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    BackgroundedMultiPipelineSequence,
    UnterminatedIf,
    UnexpectedKeyword(String),
    UnterminatedLoop,
    UnexpectedToken,
    ForVariable,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut iter = tokens.into_iter().peekable();
    skip_newlines(&mut iter);
    if iter.peek().is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(&mut iter, &[])?;
    if iter.peek().is_some() {
        // A stray terminator (`;;`/`;&`/`;;&`) left after the top-level
        // sequence — `parse_sequence` peek-breaks on those (see below).
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

/// Parses commands joined by `;` / `&&` / `||` (and an optional trailing
/// `&` at top level only). Stops — without consuming — when the next
/// token is a keyword in `stop_at`. `stop_at` is empty only at the top
/// level; a non-empty `stop_at` means we are inside a compound command.
fn parse_sequence<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
) -> Result<Sequence, ParseError> {
    let at_top_level = stop_at.is_empty();
    let first = parse_command(iter)?;
    let mut rest = Vec::new();
    let mut background = false;

    loop {
        match iter.peek() {
            None => break,
            Some(Token::Op(
                Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
            )) => break,
            Some(tok) => {
                if let Some(kw) = keyword_of(tok) {
                    if stop_at.contains(&kw) {
                        break;
                    }
                }
            }
        }
        let token = iter.next().unwrap();
        match token {
            Token::Op(Operator::Background) => {
                if !at_top_level {
                    return Err(ParseError::UnexpectedBackground);
                }
                if !rest.is_empty() {
                    return Err(ParseError::BackgroundedMultiPipelineSequence);
                }
                if iter.peek().is_some() {
                    return Err(ParseError::UnexpectedBackground);
                }
                background = true;
                break;
            }
            Token::Op(Operator::Semi) | Token::Newline => {
                skip_newlines(iter);
                match iter.peek() {
                    None => break,
                    Some(tok) => {
                        if keyword_of(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) {
                            break;
                        }
                    }
                }
                rest.push((Connector::Semi, parse_command(iter)?));
            }
            Token::Op(Operator::And) => {
                skip_newlines(iter);
                rest.push((Connector::And, parse_command(iter)?));
            }
            Token::Op(Operator::Or) => {
                skip_newlines(iter);
                rest.push((Connector::Or, parse_command(iter)?));
            }
            other => {
                if let Some(kw) = keyword_of(&other) {
                    return Err(ParseError::UnexpectedKeyword(kw.name().to_string()));
                }
                // A non-keyword, non-connector token after a command —
                // e.g. a stray word or `|` after a closed `if`/`while`.
                return Err(ParseError::UnexpectedToken);
            }
        }
    }

    Ok(Sequence { first, rest, background })
}

/// Parses a single sequence element: an `if` clause or a pipeline.
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    skip_newlines(iter);
    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok(Command::If(Box::new(parse_if(iter)?))),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok(Command::While(Box::new(parse_while(iter)?)))
        }
        Some(Keyword::For) => Ok(Command::For(Box::new(parse_for(iter)?))),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => Ok(Command::Pipeline(parse_pipeline(iter)?)),
    }
}

/// Consumes a run of `Newline` tokens. Newlines are soft separators —
/// they are skipped wherever a command is expected but not yet present.
fn skip_newlines<I: Iterator<Item = Token>>(iter: &mut std::iter::Peekable<I>) {
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }
}

/// Consumes one token and checks it is the expected keyword.
fn expect_keyword<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    match iter.next() {
        Some(ref t) if keyword_of(t) == Some(expected) => Ok(()),
        _ => Err(on_missing),
    }
}

/// Runs `parse_sequence` for a compound command's condition or body.
/// If it fails with `MissingCommand` because input simply ran out
/// (the iterator is exhausted), the failure is the compound command
/// being unterminated — report `unterminated` instead. A
/// `MissingCommand` with tokens still pending is a genuine error and
/// passes through unchanged.
///
/// Known edge case: a compound section consisting of a bare leading
/// `|` (e.g. `if |`) also yields `MissingCommand` with an exhausted
/// iterator and is mis-remapped to `unterminated`. This is harmless in
/// practice — the REPL's completeness classifier intercepts any buffer
/// ending in a bare `|`/`&&`/`||` before `parse` is reached, so this
/// path is unreachable through the shell.
fn parse_compound_section<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
    unterminated: ParseError,
) -> Result<Sequence, ParseError> {
    match parse_sequence(iter, stop_at) {
        Err(ParseError::MissingCommand) if iter.peek().is_none() => Err(unterminated),
        other => other,
    }
}

/// Parses `if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`.
fn parse_if<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<IfClause, ParseError> {
    expect_keyword(iter, Keyword::If, ParseError::UnterminatedIf)?;
    let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
    expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
    let then_body = parse_compound_section(
        iter,
        &[Keyword::Elif, Keyword::Else, Keyword::Fi],
        ParseError::UnterminatedIf,
    )?;

    let mut elif_branches = Vec::new();
    while iter.peek().and_then(keyword_of) == Some(Keyword::Elif) {
        iter.next(); // consume `elif`
        let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek().and_then(keyword_of) == Some(Keyword::Else) {
        iter.next(); // consume `else`
        Some(parse_compound_section(iter, &[Keyword::Fi], ParseError::UnterminatedIf)?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    Ok(IfClause { condition, then_body, elif_branches, else_body })
}

/// Returns the loop-variable name if `token` is a single, unquoted
/// `Literal` `Word` whose text is a valid identifier and not a reserved
/// keyword. Otherwise `None`.
fn for_variable_name(token: &Token) -> Option<String> {
    if keyword_of(token).is_some() {
        return None;
    }
    let Token::Word(Word(parts)) = token else {
        return None;
    };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &parts[0] else {
        return None;
    };
    let mut chars = text.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(text.clone())
}

/// Parses `for NAME [in WORD...] sep do LIST done`. The caller has
/// peeked the leading `for`.
fn parse_for<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ForClause, ParseError> {
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Loop variable. End-of-input means the command is incomplete (the
    // v19 classifier maps UnterminatedLoop to "read more"); a present
    // but invalid token is a genuine error.
    let var = match iter.next() {
        None => return Err(ParseError::UnterminatedLoop),
        Some(tok) => for_variable_name(&tok).ok_or(ParseError::ForVariable)?,
    };

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter);

    // Optional `in` plus the word list.
    let mut words: Vec<Word> = Vec::new();
    if iter.peek().and_then(keyword_of) == Some(Keyword::In) {
        iter.next(); // consume `in`
        loop {
            match iter.peek() {
                None | Some(Token::Newline) | Some(Token::Op(Operator::Semi)) => break,
                Some(tok) => {
                    if keyword_of(tok) == Some(Keyword::Do) {
                        break;
                    }
                    match iter.next() {
                        Some(Token::Word(w)) => words.push(w),
                        Some(Token::Op(_)) => return Err(ParseError::UnexpectedToken),
                        _ => unreachable!("peek already ruled out Newline/Semi/None here"),
                    }
                }
            }
        }
    }

    // Skip `;`/newline separators, then `do`.
    while matches!(
        iter.peek(),
        Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
    ) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(ForClause { var, words, body })
}

/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// The caller has already peeked the leading `while`/`until`.
fn parse_while<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<WhileClause, ParseError> {
    let until = match iter.next().as_ref().and_then(keyword_of) {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    let condition = parse_compound_section(iter, &[Keyword::Do], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(WhileClause { condition, body, until })
}

fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<SimpleCommand> = Vec::new();

    let mut program: Option<Word> = None;
    let mut args: Vec<Word> = Vec::new();
    let mut stdin: Option<Word> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    while let Some(token) = iter.peek() {
        if matches!(
            token,
            Token::Op(
                Operator::Semi
                    | Operator::And
                    | Operator::Or
                    | Operator::Background
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp
            ) | Token::Newline
        ) {
            break;
        }
        let token = iter.next().unwrap();
        match token {
            Token::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            Token::Newline => {
                // Unreachable: the peek-break above stops the loop on a
                // Newline before it is ever consumed here, and a Newline
                // directly after `|` is consumed by skip_newlines in the
                // `Pipe` arm. This arm only keeps the match exhaustive.
                unreachable!("Newline terminates the pipeline via the peek-break above");
            }
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(finalize_stage(
                    prog,
                    std::mem::take(&mut args),
                    stdin.take(),
                    stdout.take(),
                    stderr.take(),
                ));
                skip_newlines(iter);
            }
            Token::Op(Operator::LParen | Operator::RParen) => {
                // A `(` or `)` outside a `case` pattern list is a syntax error.
                return Err(ParseError::UnexpectedToken);
            }
            Token::Op(op) => {
                let target = match iter.next() {
                    Some(Token::Word(word)) => word,
                    Some(Token::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(Token::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                };
                match op {
                    Operator::RedirIn => stdin = Some(target),
                    Operator::RedirOut => stdout = Some(Redirect::Truncate(target)),
                    Operator::RedirAppend => stdout = Some(Redirect::Append(target)),
                    Operator::RedirErr => stderr = Some(Redirect::Truncate(target)),
                    Operator::RedirErrAppend => stderr = Some(Redirect::Append(target)),
                    Operator::Pipe
                    | Operator::And
                    | Operator::Or
                    | Operator::Semi
                    | Operator::Background
                    | Operator::LParen
                    | Operator::RParen
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp => {
                        unreachable!("handled in the outer arms or peek-break");
                    }
                }
            }
        }
    }

    let prog = program.ok_or(ParseError::MissingCommand)?;
    commands.push(finalize_stage(prog, args, stdin, stdout, stderr));

    Ok(Pipeline { commands })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::WordPart;

    fn w_tok(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]))
    }

    fn ww(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    /// Builds a SimpleCommand::Exec with no redirections, all-Literal Words.
    fn plain(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            program: ww(program),
            args: args.iter().map(|a| ww(a)).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline { commands }),
            rest: vec![],
            background: false,
        }
    }

    /// Reaches through `Command::Pipeline` for tests that inspect the first
    /// element as a pipeline.
    fn first_pipeline(seq: &Sequence) -> &Pipeline {
        match &seq.first {
            Command::Pipeline(p) => p,
            Command::If(_) => panic!("expected a pipeline, got an if"),
            Command::While(_) => panic!("expected a pipeline, got a while"),
            Command::For(_) => panic!("expected a pipeline, got a for"),
        }
    }

    fn exec_stdout(seq: &Sequence) -> &Option<Redirect> {
        match &first_pipeline(seq).commands[0] {
            SimpleCommand::Exec(e) => &e.stdout,
            _ => panic!("expected Exec"),
        }
    }

    fn exec_stdin(seq: &Sequence) -> &Option<Word> {
        match &first_pipeline(seq).commands[0] {
            SimpleCommand::Exec(e) => &e.stdin,
            _ => panic!("expected Exec"),
        }
    }

    fn exec_stderr(seq: &Sequence) -> &Option<Redirect> {
        match &first_pipeline(seq).commands[0] {
            SimpleCommand::Exec(e) => &e.stderr,
            _ => panic!("expected Exec"),
        }
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w_tok("ls")]),
            Ok(Some(one_pipeline(vec![plain("ls", &[])])))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w_tok("ls"), w_tok("-la"), w_tok("/tmp")]),
            Ok(Some(one_pipeline(vec![plain("ls", &["-la", "/tmp"])])))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Truncate(ww("f"))));
    }

    #[test]
    fn parse_redirect_append() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirAppend), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Append(ww("f"))));
    }

    #[test]
    fn parse_redirect_in() {
        let seq = parse(vec![w_tok("cat"), Token::Op(Operator::RedirIn), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdin(&seq), &Some(ww("f")));
    }

    #[test]
    fn parse_redirect_stderr() {
        let seq = parse(vec![w_tok("cmd"), Token::Op(Operator::RedirErr), w_tok("e")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Truncate(ww("e"))));
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let seq = parse(vec![
            w_tok("cmd"),
            Token::Op(Operator::RedirErrAppend),
            w_tok("e"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Append(ww("e"))));
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Pipe), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![plain("a", &[]), plain("b", &[])]);
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let seq = parse(vec![
            w_tok("a"),
            Token::Op(Operator::Pipe),
            w_tok("b"),
            Token::Op(Operator::Pipe),
            w_tok("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 3);
    }

    #[test]
    fn parse_leading_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Pipe), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::Pipe)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::Pipe),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_program_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::RedirOut), w_tok("f")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_target_is_error() {
        assert_eq!(
            parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut)]),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_is_operator_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }

    #[test]
    fn parse_semicolon_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![plain("a", &[])]);
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
    }

    #[test]
    fn parse_and_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::And), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_or_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Or), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::Or);
    }

    #[test]
    fn parse_trailing_semicolon_is_allowed() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi)])
            .unwrap()
            .unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![plain("a", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_trailing_and_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::And)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_leading_semicolon_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Semi), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_sequencing_op_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::And),
                Token::Op(Operator::And),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    fn assignment(name: &str, value: Word) -> SimpleCommand {
        SimpleCommand::Assign { name: name.to_string(), value }
    }

    #[test]
    fn parse_simple_assignment() {
        let seq = parse(vec![w_tok("FOO=bar")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![assignment("FOO", ww("bar"))]);
    }

    #[test]
    fn parse_empty_value_assignment() {
        let seq = parse(vec![w_tok("FOO=")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![assignment("FOO", ww(""))]);
    }

    #[test]
    fn parse_assignment_with_expansion_in_value() {
        let var_part = WordPart::Var { name: "BAR".to_string(), quoted: false };
        let prog = Token::Word(Word(vec![
            WordPart::Literal { text: "FOO=".to_string(), quoted: false },
            var_part,
        ]));
        let seq = parse(vec![prog]).unwrap().unwrap();
        let expected_value = Word(vec![
            WordPart::Literal { text: "".to_string(), quoted: false },
            WordPart::Var { name: "BAR".to_string(), quoted: false },
        ]);
        assert_eq!(first_pipeline(&seq).commands, vec![assignment("FOO", expected_value)]);
    }

    #[test]
    fn parse_assignment_invalid_name_is_exec() {
        let seq = parse(vec![w_tok("1FOO=bar")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![plain("1FOO=bar", &[])]);
    }

    #[test]
    fn parse_assignment_with_arg_is_exec() {
        let seq = parse(vec![w_tok("FOO=bar"), w_tok("baz")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![plain("FOO=bar", &["baz"])]);
    }

    #[test]
    fn parse_assignment_with_redirect_is_exec() {
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::RedirOut),
            w_tok("f"),
        ])
        .unwrap()
        .unwrap();
        match &first_pipeline(&seq).commands[0] {
            SimpleCommand::Exec(e) => {
                assert_eq!(e.program, ww("FOO=bar"));
                assert_eq!(e.stdout, Some(Redirect::Truncate(ww("f"))));
            }
            _ => panic!("expected Exec"),
        }
    }

    #[test]
    fn parse_assignment_in_pipeline_stage() {
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::Pipe),
            w_tok("cat"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 2);
        assert_eq!(first_pipeline(&seq).commands[0], assignment("FOO", ww("bar")));
        assert_eq!(first_pipeline(&seq).commands[1], plain("cat", &[]));
    }

    #[test]
    fn parse_assignment_with_command_sub_value_moves_parts() {
        // Simulates lexer output for `FOO=$(echo bar)`: one Word with two
        // parts — Literal("FOO=") and CommandSub. The parser's
        // try_split_assignment must MOVE the CommandSub into the value
        // (it can't be cloned without Clone on Sequence). Asserts the
        // resulting Assign carries a value Word [Literal(""), CommandSub].
        use crate::lexer::WordPart;
        let inner_seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("echo"),
                    args: vec![ww("bar")],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        };
        let program_word = Word(vec![
            WordPart::Literal { text: "FOO=".to_string(), quoted: false },
            WordPart::CommandSub { sequence: inner_seq, quoted: false },
        ]);
        let seq = parse(vec![Token::Word(program_word)]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 1);
        match &first_pipeline(&seq).commands[0] {
            SimpleCommand::Assign { name, value } => {
                assert_eq!(name, "FOO");
                assert_eq!(value.0.len(), 2);
                match &value.0[0] {
                    WordPart::Literal { text, .. } => assert_eq!(text, ""),
                    other => panic!("expected Literal(\"\"), got {other:?}"),
                }
                assert!(matches!(&value.0[1], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn parse_command_with_background() {
        let seq = parse(vec![w_tok("sleep"), w_tok("1"), Token::Op(Operator::Background)])
            .unwrap()
            .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(first_pipeline(&seq).commands, vec![plain("sleep", &["1"])]);
    }

    #[test]
    fn parse_pipeline_backgrounded() {
        // cmd1 | cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::Pipe),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(first_pipeline(&seq).commands.len(), 2);
    }

    #[test]
    fn parse_background_alone_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Background)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_background_mid_sequence_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::Background),
                w_tok("cmd2"),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_two_backgrounds_is_unexpected() {
        assert_eq!(
            parse(vec![
                w_tok("cmd"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_background_after_andor_is_unsupported() {
        // cmd1 && cmd2 &
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }

    #[test]
    fn parse_background_after_semi_is_unsupported() {
        // cmd1 ; cmd2 &
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::Semi),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }

    #[test]
    fn parse_background_mid_sequence_after_andor_prefers_multipipeline_error() {
        // cmd1 && cmd2 & cmd3 — both errors apply; the more specific
        // BackgroundedMultiPipelineSequence wins.
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
                w_tok("cmd3"),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }

    /// A bare unquoted keyword token (same shape as an ordinary word).
    fn kw(s: &str) -> Token {
        w_tok(s)
    }

    /// Extracts the IfClause from a sequence whose first command is an If.
    fn first_if(seq: &Sequence) -> &IfClause {
        match &seq.first {
            Command::If(c) => c,
            Command::Pipeline(_) => panic!("expected an if, got a pipeline"),
            Command::While(_) => panic!("expected an if, got a while"),
            Command::For(_) => panic!("expected an if, got a for"),
        }
    }

    /// Extracts the WhileClause from a sequence whose first command is a While.
    fn first_while(seq: &Sequence) -> &WhileClause {
        match &seq.first {
            Command::While(c) => c,
            other => panic!("expected a while, got {other:?}"),
        }
    }

    /// Extracts the ForClause from a sequence whose first command is a For.
    fn first_for(seq: &Sequence) -> &ForClause {
        match &seq.first {
            Command::For(c) => c,
            other => panic!("expected a for, got {other:?}"),
        }
    }

    #[test]
    fn parse_simple_for() {
        // for x in a b c ; do echo ; done
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"),
            w_tok("a"), w_tok("b"), w_tok("c"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_for(&seq);
        assert_eq!(clause.var, "x");
        assert_eq!(clause.words.len(), 3);
        assert_eq!(
            clause.body.first,
            Command::Pipeline(Pipeline { commands: vec![plain("echo", &[])] })
        );
    }

    #[test]
    fn parse_for_multiline_matches_singleline() {
        let multiline = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("echo"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let singleline = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(multiline, singleline);
    }

    #[test]
    fn parse_for_no_in_has_empty_words() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_for(&seq).words.is_empty());
    }

    #[test]
    fn parse_for_empty_in_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_for(&seq).words.is_empty());
    }

    #[test]
    fn parse_for_do_terminates_word_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), w_tok("b"),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_for(&seq).words.len(), 2);
    }

    #[test]
    fn parse_for_keyword_words_in_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("then"), w_tok("else"),
            Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_for(&seq).words.len(), 2);
    }

    #[test]
    fn parse_for_in_on_next_line() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), Token::Newline,
            kw("in"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("echo"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_for(&seq);
        assert_eq!(clause.var, "x");
        assert_eq!(clause.words.len(), 1);
    }

    #[test]
    fn parse_for_invalid_variable_name_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), w_tok("2x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::ForVariable)
        );
    }

    #[test]
    fn parse_for_keyword_as_variable_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::ForVariable)
        );
    }

    #[test]
    fn parse_for_unterminated_is_unterminated_loop() {
        assert_eq!(
            parse(vec![kw("for"), w_tok("x"), kw("in"), w_tok("a")]),
            Err(ParseError::UnterminatedLoop)
        );
        assert_eq!(parse(vec![kw("for")]), Err(ParseError::UnterminatedLoop));
    }

    #[test]
    fn parse_for_operator_in_word_list_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), w_tok("x"), kw("in"), w_tok("a"),
                Token::Op(Operator::Pipe), w_tok("b"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn parse_simple_if() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
        assert_eq!(c.then_body.first, Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] }));
        assert!(c.elif_branches.is_empty());
        assert!(c.else_body.is_none());
    }

    #[test]
    fn parse_if_else() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("else"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert!(first_if(&seq).else_body.is_some());
    }

    #[test]
    fn parse_if_elif_else() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("elif"), w_tok("c"), Token::Op(Operator::Semi),
            kw("then"), w_tok("d"), Token::Op(Operator::Semi),
            kw("else"), w_tok("e"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.elif_branches.len(), 1);
        assert!(c.else_body.is_some());
    }

    #[test]
    fn parse_if_with_andor_condition() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
            Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.condition.rest.len(), 1);
        assert_eq!(c.condition.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_if_multi_command_body() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
            Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert_eq!(first_if(&seq).then_body.rest.len(), 1);
    }

    #[test]
    fn parse_if_followed_by_command() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi), w_tok("echo"),
        ]).unwrap().unwrap();
        assert!(matches!(seq.first, Command::If(_)));
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
        assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
    }

    #[test]
    fn parse_if_joined_with_and() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::And), w_tok("echo"),
        ]).unwrap().unwrap();
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_nested_if() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"),
            kw("if"), w_tok("b"), Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert!(matches!(first_if(&seq).then_body.first, Command::If(_)));
    }

    #[test]
    fn parse_if_unterminated_is_error() {
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"),
        ]);
        assert_eq!(r, Err(ParseError::UnterminatedIf));
    }

    #[test]
    fn parse_if_missing_then_is_error() {
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
    }

    #[test]
    fn parse_bare_then_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("then"), w_tok("x")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_bare_fi_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("fi")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_if_empty_condition_is_missing_command() {
        let r = parse(vec![
            kw("if"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert_eq!(r, Err(ParseError::MissingCommand));
    }

    #[test]
    fn parse_keyword_as_argument_is_literal() {
        let seq = parse(vec![w_tok("echo"), w_tok("if")]).unwrap().unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline {
            commands: vec![plain("echo", &["if"])],
        }));
    }

    #[test]
    fn parse_trailing_keyword_after_if_is_unexpected_keyword() {
        // `if a; then b; fi fi` — a stray `fi` after a complete `if`.
        // Must be a clean parse error, never a panic.
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), kw("fi"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))), "got {r:?}");
    }

    #[test]
    fn parse_if_condition_with_background_is_error() {
        // `&` is not allowed inside an `if` condition/body (v17 limitation).
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Background),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert_eq!(r, Err(ParseError::UnexpectedBackground));
    }

    #[test]
    fn parse_simple_while() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let c = first_while(&seq);
        assert!(!c.until);
        assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
        assert_eq!(c.body.first, Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] }));
    }

    #[test]
    fn parse_until_sets_flag() {
        let seq = parse(vec![
            kw("until"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_while(&seq).until);
    }

    #[test]
    fn parse_while_andor_condition() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
            Token::Op(Operator::Semi),
            kw("do"), w_tok("c"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let c = first_while(&seq);
        assert_eq!(c.condition.rest.len(), 1);
        assert_eq!(c.condition.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_while_multi_command_body() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
            Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_while(&seq).body.rest.len(), 1);
    }

    #[test]
    fn parse_while_followed_by_command() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"), Token::Op(Operator::Semi), w_tok("echo"),
        ]).unwrap().unwrap();
        assert!(matches!(seq.first, Command::While(_)));
        assert_eq!(seq.rest.len(), 1);
        assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
    }

    #[test]
    fn parse_nested_while() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"),
            kw("while"), w_tok("b"), Token::Op(Operator::Semi),
            kw("do"), w_tok("c"), Token::Op(Operator::Semi),
            kw("done"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(matches!(first_while(&seq).body.first, Command::While(_)));
    }

    #[test]
    fn parse_while_with_if_body() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"),
            kw("if"), w_tok("b"), Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(matches!(first_while(&seq).body.first, Command::If(_)));
    }

    #[test]
    fn parse_while_unterminated_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"),
        ]);
        assert_eq!(r, Err(ParseError::UnterminatedLoop));
    }

    #[test]
    fn parse_while_missing_do_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi), kw("done"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
    }

    #[test]
    fn parse_bare_do_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("do"), w_tok("x")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_bare_done_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("done")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_while_empty_condition_is_missing_command() {
        let r = parse(vec![
            kw("while"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi), kw("done"),
        ]);
        assert_eq!(r, Err(ParseError::MissingCommand));
    }

    #[test]
    fn parse_while_background_in_body_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Background),
            kw("done"),
        ]);
        assert_eq!(r, Err(ParseError::UnexpectedBackground));
    }

    #[test]
    fn parse_keyword_while_as_argument_is_literal() {
        let seq = parse(vec![w_tok("echo"), w_tok("while")]).unwrap().unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline {
            commands: vec![plain("echo", &["while"])],
        }));
    }

    #[test]
    fn multiline_if_parses_same_as_singleline() {
        let multiline = parse(vec![
            kw("if"), w_tok("a"), Token::Newline,
            kw("then"), w_tok("b"), Token::Newline,
            kw("fi"),
        ]).unwrap().unwrap();
        let singleline = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert_eq!(multiline, singleline);
    }

    #[test]
    fn newline_after_then_is_skipped() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Newline,
            kw("then"), Token::Newline,
            w_tok("b"), Token::Newline,
            kw("fi"),
        ]).unwrap().unwrap();
        let clause = first_if(&seq);
        assert_eq!(
            clause.then_body.first,
            Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] })
        );
    }

    #[test]
    fn multiline_while_parses() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("b"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_while(&seq);
        assert!(!clause.until);
        assert_eq!(
            clause.body.first,
            Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] })
        );
    }

    #[test]
    fn newline_separates_top_level_commands() {
        let seq = parse(vec![w_tok("a"), Token::Newline, w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
    }

    #[test]
    fn leading_newlines_are_skipped() {
        let seq = parse(vec![Token::Newline, Token::Newline, w_tok("a")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
    }

    #[test]
    fn all_newline_buffer_is_none() {
        assert_eq!(parse(vec![Token::Newline, Token::Newline]), Ok(None));
    }

    #[test]
    fn newline_after_pipe_continues_pipeline() {
        let seq = parse(vec![
            w_tok("a"), Token::Op(Operator::Pipe), Token::Newline, w_tok("b"),
        ]).unwrap().unwrap();
        let p = first_pipeline(&seq);
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn trailing_semicolon_then_newline_is_not_an_error() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), Token::Newline])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 0);
    }

    #[test]
    fn then_followed_by_semicolon_still_errors() {
        let result = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), Token::Op(Operator::Semi),
            w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]);
        assert_eq!(result, Err(ParseError::MissingCommand));
    }

    #[test]
    fn stray_word_after_compound_errors_without_panic() {
        let result = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), w_tok("extra"),
        ]);
        assert_eq!(result, Err(ParseError::UnexpectedToken));
    }

    #[test]
    fn stray_close_paren_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::RParen)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_open_paren_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::LParen)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_double_semi_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::DoubleSemi)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_semi_amp_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::SemiAmp)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_double_semi_amp_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::DoubleSemiAmp)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn if_with_no_body_at_end_of_input_is_unterminated() {
        let result = parse(vec![kw("if"), w_tok("a"), Token::Newline, kw("then")]);
        assert_eq!(result, Err(ParseError::UnterminatedIf));
    }

    #[test]
    fn while_with_no_body_at_end_of_input_is_unterminated() {
        let result = parse(vec![kw("while"), w_tok("a"), Token::Newline, kw("do")]);
        assert_eq!(result, Err(ParseError::UnterminatedLoop));
    }
}
