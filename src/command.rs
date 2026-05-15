use crate::lexer::{Operator, Token};

#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    Truncate(String), // > file   (and the target form of 2>)
    Append(String),   // >> file  (and the target form of 2>>)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>, // invariant: never empty
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi, // ;
    And,  // &&
    Or,   // ||
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut iter = tokens.into_iter().peekable();
    let first = parse_pipeline(&mut iter)?;
    let mut rest = Vec::new();

    while let Some(token) = iter.next() {
        let connector = match token {
            Token::Op(Operator::Semi) => Connector::Semi,
            Token::Op(Operator::And) => Connector::And,
            Token::Op(Operator::Or) => Connector::Or,
            _ => unreachable!("parse_pipeline returns only at a sequencing op or end"),
        };
        // Trailing `;` is allowed: stop here if there's nothing after it.
        if matches!(connector, Connector::Semi) && iter.peek().is_none() {
            break;
        }
        let pipeline = parse_pipeline(&mut iter)?;
        rest.push((connector, pipeline));
    }

    Ok(Some(Sequence { first, rest }))
}

/// Parses one pipeline from the iterator. Stops at — without consuming — the
/// next sequencing operator (`;`, `&&`, `||`) or end of input. Returns
/// `Err(ParseError::MissingCommand)` if the pipeline ended with no program.
fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<Command> = Vec::new();

    // Builder state for the command currently being assembled.
    let mut program: Option<String> = None;
    let mut args: Vec<String> = Vec::new();
    let mut stdin: Option<String> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    while let Some(token) = iter.peek() {
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or)
        ) {
            // Don't consume — the outer loop handles it.
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
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(Command {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                });
            }
            Token::Op(op) => {
                // A redirect operator: the next token must be a filename word.
                let target = match iter.next() {
                    Some(Token::Word(word)) => word,
                    Some(Token::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    None => return Err(ParseError::MissingRedirectTarget),
                };
                match op {
                    Operator::RedirIn => stdin = Some(target),
                    Operator::RedirOut => stdout = Some(Redirect::Truncate(target)),
                    Operator::RedirAppend => stdout = Some(Redirect::Append(target)),
                    Operator::RedirErr => stderr = Some(Redirect::Truncate(target)),
                    Operator::RedirErrAppend => stderr = Some(Redirect::Append(target)),
                    Operator::Pipe | Operator::And | Operator::Or | Operator::Semi => {
                        unreachable!("handled in the outer arms");
                    }
                }
            }
        }
    }

    let prog = program.ok_or(ParseError::MissingCommand)?;
    commands.push(Command {
        program: prog,
        args,
        stdin,
        stdout,
        stderr,
    });

    Ok(Pipeline { commands })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Token {
        Token::Word(s.to_string())
    }

    /// Builds a command with no redirections.
    fn plain(program: &str, args: &[&str]) -> Command {
        Command {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    /// Builds a sequence with a single pipeline (no sequencing operators).
    fn one_pipeline(commands: Vec<Command>) -> Sequence {
        Sequence {
            first: Pipeline { commands },
            rest: vec![],
        }
    }

    // ----- single-pipeline cases (regressions from v2) -----

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Ok(Some(one_pipeline(vec![plain("ls", &[])])))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Ok(Some(one_pipeline(vec![plain("ls", &["-la", "/tmp"])])))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let seq = parse(vec![w("ls"), Token::Op(Operator::RedirOut), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_redirect_append() {
        let seq = parse(vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Append("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_in() {
        let seq = parse(vec![w("cat"), Token::Op(Operator::RedirIn), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands[0].stdin, Some("f".to_string()));
    }

    #[test]
    fn parse_redirect_stderr() {
        let seq = parse(vec![w("cmd"), Token::Op(Operator::RedirErr), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stderr,
            Some(Redirect::Truncate("e".to_string()))
        );
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let seq = parse(vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stderr,
            Some(Redirect::Append("e".to_string()))
        );
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Pipe), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[]), plain("b", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::Pipe),
            w("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands.len(), 3);
    }

    #[test]
    fn parse_pipeline_with_redirects_on_stages() {
        // a < in | b > out
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::RedirIn),
            w("in"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::RedirOut),
            w("out"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands[0].stdin, Some("in".to_string()));
        assert_eq!(
            seq.first.commands[1].stdout,
            Some(Redirect::Truncate("out".to_string()))
        );
    }

    #[test]
    fn parse_last_redirect_of_a_kind_wins() {
        let seq = parse(vec![
            w("ls"),
            Token::Op(Operator::RedirOut),
            w("a"),
            Token::Op(Operator::RedirOut),
            w("b"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("b".to_string()))
        );
    }

    #[test]
    fn parse_leading_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Pipe), w("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Pipe)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_pipe_is_missing_command() {
        // Two consecutive Op(Pipe) — not Op(Or) — at the parser level.
        assert_eq!(
            parse(vec![
                w("a"),
                Token::Op(Operator::Pipe),
                Token::Op(Operator::Pipe),
                w("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_program_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::RedirOut), w("f")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_target_is_error() {
        assert_eq!(
            parse(vec![w("ls"), Token::Op(Operator::RedirOut)]),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_is_operator_is_error() {
        assert_eq!(
            parse(vec![
                w("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Pipe),
                w("b"),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }

    // ----- new: command sequencing -----

    #[test]
    fn parse_semicolon_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Semi), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
        assert_eq!(seq.rest[0].1.commands, vec![plain("b", &[])]);
    }

    #[test]
    fn parse_and_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::And), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_or_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Or), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Or);
    }

    #[test]
    fn parse_mixed_sequencing_operators() {
        // a && b || c ; d
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::And),
            w("b"),
            Token::Op(Operator::Or),
            w("c"),
            Token::Op(Operator::Semi),
            w("d"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert_eq!(
            seq.rest.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![Connector::And, Connector::Or, Connector::Semi]
        );
        assert_eq!(seq.rest[0].1.commands, vec![plain("b", &[])]);
        assert_eq!(seq.rest[1].1.commands, vec![plain("c", &[])]);
        assert_eq!(seq.rest[2].1.commands, vec![plain("d", &[])]);
    }

    #[test]
    fn parse_sequence_of_multi_stage_pipelines() {
        // ls | grep foo && find . -name bar | wc -l
        let seq = parse(vec![
            w("ls"),
            Token::Op(Operator::Pipe),
            w("grep"),
            w("foo"),
            Token::Op(Operator::And),
            w("find"),
            w("."),
            w("-name"),
            w("bar"),
            Token::Op(Operator::Pipe),
            w("wc"),
            w("-l"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands,
            vec![plain("ls", &[]), plain("grep", &["foo"])]
        );
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::And);
        assert_eq!(
            seq.rest[0].1.commands,
            vec![plain("find", &[".", "-name", "bar"]), plain("wc", &["-l"])]
        );
    }

    #[test]
    fn parse_pipeline_with_redirect_inside_sequence() {
        // echo hi > f ; cat f
        let seq = parse(vec![
            w("echo"),
            w("hi"),
            Token::Op(Operator::RedirOut),
            w("f"),
            Token::Op(Operator::Semi),
            w("cat"),
            w("f"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
        assert_eq!(seq.rest[0].1.commands, vec![plain("cat", &["f"])]);
    }

    #[test]
    fn parse_trailing_semicolon_is_allowed() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Semi)])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_trailing_and_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::And)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_or_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Or)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_leading_semicolon_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Semi), w("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_sequencing_op_is_missing_command() {
        assert_eq!(
            parse(vec![
                w("a"),
                Token::Op(Operator::And),
                Token::Op(Operator::And),
                w("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_target_is_sequencing_op_is_error() {
        // ls > ;
        assert_eq!(
            parse(vec![
                w("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Semi),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }
}
