#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

pub fn parse(tokens: Vec<String>) -> Option<Command> {
    let mut iter = tokens.into_iter();
    let program = iter.next()?;
    let args = iter.collect();
    Some(Command { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), None);
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec!["ls".to_string()]),
            Some(Command {
                program: "ls".to_string(),
                args: vec![],
            })
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![
                "ls".to_string(),
                "-la".to_string(),
                "/tmp".to_string()
            ]),
            Some(Command {
                program: "ls".to_string(),
                args: vec!["-la".to_string(), "/tmp".to_string()],
            })
        );
    }
}
