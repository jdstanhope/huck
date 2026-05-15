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

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Pipeline>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut commands: Vec<Command> = Vec::new();

    // Builder state for the command currently being assembled.
    let mut program: Option<String> = None;
    let mut args: Vec<String> = Vec::new();
    let mut stdin: Option<String> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    let mut iter = tokens.into_iter();

    while let Some(token) = iter.next() {
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
            Token::Op(Operator::And | Operator::Or | Operator::Semi) => {
                // INTERIM (made real in Task 2): sequencing operators are not
                // yet parsed; report as a syntax error.
                return Err(ParseError::MissingCommand);
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

    Ok(Some(Pipeline { commands }))
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

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Ok(Some(Pipeline {
                commands: vec![plain("ls", &[])],
            }))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Ok(Some(Pipeline {
                commands: vec![plain("ls", &["-la", "/tmp"])],
            }))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let pipeline = parse(vec![w("ls"), Token::Op(Operator::RedirOut), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_append() {
        let pipeline = parse(vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
            Some(Redirect::Append("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_in() {
        let pipeline = parse(vec![w("cat"), Token::Op(Operator::RedirIn), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(pipeline.commands[0].stdin, Some("f".to_string()));
    }

    #[test]
    fn parse_redirect_stderr() {
        let pipeline = parse(vec![w("cmd"), Token::Op(Operator::RedirErr), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stderr,
            Some(Redirect::Truncate("e".to_string()))
        );
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let pipeline = parse(vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stderr,
            Some(Redirect::Append("e".to_string()))
        );
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let pipeline = parse(vec![w("a"), Token::Op(Operator::Pipe), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(pipeline.commands, vec![plain("a", &[]), plain("b", &[])]);
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let pipeline = parse(vec![
            w("a"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::Pipe),
            w("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(pipeline.commands.len(), 3);
    }

    #[test]
    fn parse_pipeline_with_redirects_on_stages() {
        // a < in | b > out
        let pipeline = parse(vec![
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
        assert_eq!(pipeline.commands[0].stdin, Some("in".to_string()));
        assert_eq!(
            pipeline.commands[1].stdout,
            Some(Redirect::Truncate("out".to_string()))
        );
    }

    #[test]
    fn parse_last_redirect_of_a_kind_wins() {
        // ls > a > b
        let pipeline = parse(vec![
            w("ls"),
            Token::Op(Operator::RedirOut),
            w("a"),
            Token::Op(Operator::RedirOut),
            w("b"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
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

    #[test]
    fn parse_sequencing_op_is_interim_missing_command() {
        // INTERIM (deleted in Task 2): the parser does not yet handle sequencing.
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Semi), w("b")]),
            Err(ParseError::MissingCommand)
        );
    }
}
