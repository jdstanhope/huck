use crate::lexer::Token;

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

/// INTERIM (replaced by the full pipeline parser in Task 3): builds a single
/// command from word tokens. A line containing any operator token is not yet
/// supported and parses to `None`, so the REPL simply re-prompts.
pub fn parse(tokens: Vec<Token>) -> Option<Command> {
    let mut words = Vec::new();
    for token in tokens {
        match token {
            Token::Word(w) => words.push(w),
            Token::Op(_) => return None,
        }
    }
    let mut iter = words.into_iter();
    let program = iter.next()?;
    let args = iter.collect();
    Some(Command { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Operator;

    fn w(s: &str) -> Token {
        Token::Word(s.to_string())
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), None);
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Some(Command {
                program: "ls".to_string(),
                args: vec![],
            })
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Some(Command {
                program: "ls".to_string(),
                args: vec!["-la".to_string(), "/tmp".to_string()],
            })
        );
    }

    #[test]
    fn parse_operator_token_returns_none_for_now() {
        assert_eq!(
            parse(vec![w("ls"), Token::Op(Operator::Pipe), w("cat")]),
            None
        );
    }
}
