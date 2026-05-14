#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
}

pub fn tokenize(input: &str) -> Result<Vec<String>, LexError> {
    let tokens = input.split_whitespace().map(|s| s.to_string()).collect();
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), vec!["ls", "-la"]);
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<String>::new());
    }
}
