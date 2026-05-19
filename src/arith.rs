//! Arithmetic expansion: AST, parser, and evaluator for `$((expr))`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArithToken {
    Number(i64),
    Ident(String),
    LParen, RParen,
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    AndAnd, OrOr, Bang,
    Question, Colon,
}

pub(crate) fn tokenize(input: &str) -> Result<Vec<ArithToken>, ArithError> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => { chars.next(); }
            '0'..='9' => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() { s.push(d); chars.next(); } else { break; }
                }
                let n: i64 = s.parse().map_err(|_|
                    ArithError::Parse(format!("integer literal out of range: {s}")))?;
                out.push(ArithToken::Number(n));
            }
            '$' => {
                chars.next();
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d == '_' || d.is_ascii_alphanumeric() {
                        if s.is_empty() && d.is_ascii_digit() { break; }
                        s.push(d); chars.next();
                    } else { break; }
                }
                if s.is_empty() {
                    return Err(ArithError::Parse(
                        "expected identifier after '$'".to_string()));
                }
                out.push(ArithToken::Ident(s));
            }
            c if c == '_' || c.is_ascii_alphabetic() => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d == '_' || d.is_ascii_alphanumeric() {
                        s.push(d); chars.next();
                    } else { break; }
                }
                out.push(ArithToken::Ident(s));
            }
            '(' => { chars.next(); out.push(ArithToken::LParen); }
            ')' => { chars.next(); out.push(ArithToken::RParen); }
            '+' => { chars.next(); out.push(ArithToken::Plus); }
            '-' => { chars.next(); out.push(ArithToken::Minus); }
            '*' => { chars.next(); out.push(ArithToken::Star); }
            '/' => { chars.next(); out.push(ArithToken::Slash); }
            '%' => { chars.next(); out.push(ArithToken::Percent); }
            '?' => { chars.next(); out.push(ArithToken::Question); }
            ':' => { chars.next(); out.push(ArithToken::Colon); }
            '!' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::Ne);
                } else {
                    out.push(ArithToken::Bang);
                }
            }
            '=' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::Eq);
                } else {
                    return Err(ArithError::Parse(
                        "unexpected '=' (assignment is out of scope; did you mean '=='?)"
                            .to_string()));
                }
            }
            '<' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::Le);
                } else {
                    out.push(ArithToken::Lt);
                }
            }
            '>' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::Ge);
                } else {
                    out.push(ArithToken::Gt);
                }
            }
            '&' => {
                chars.next();
                if chars.peek() == Some(&'&') {
                    chars.next();
                    out.push(ArithToken::AndAnd);
                } else {
                    return Err(ArithError::Parse(
                        "unexpected '&' (bitwise operators are out of scope)".to_string()));
                }
            }
            '|' => {
                chars.next();
                if chars.peek() == Some(&'|') {
                    chars.next();
                    out.push(ArithToken::OrOr);
                } else {
                    return Err(ArithError::Parse(
                        "unexpected '|' (bitwise operators are out of scope)".to_string()));
                }
            }
            other => {
                return Err(ArithError::Parse(format!("unexpected character: {other:?}")));
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithExpr {
    Num(i64),
    Var(String),
    Neg(Box<ArithExpr>),
    Not(Box<ArithExpr>),
    Add(Box<ArithExpr>, Box<ArithExpr>),
    Sub(Box<ArithExpr>, Box<ArithExpr>),
    Mul(Box<ArithExpr>, Box<ArithExpr>),
    Div(Box<ArithExpr>, Box<ArithExpr>),
    Mod(Box<ArithExpr>, Box<ArithExpr>),
    Eq(Box<ArithExpr>, Box<ArithExpr>),
    Ne(Box<ArithExpr>, Box<ArithExpr>),
    Lt(Box<ArithExpr>, Box<ArithExpr>),
    Le(Box<ArithExpr>, Box<ArithExpr>),
    Gt(Box<ArithExpr>, Box<ArithExpr>),
    Ge(Box<ArithExpr>, Box<ArithExpr>),
    And(Box<ArithExpr>, Box<ArithExpr>),
    Or(Box<ArithExpr>, Box<ArithExpr>),
    Ternary(Box<ArithExpr>, Box<ArithExpr>, Box<ArithExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithError {
    Parse(String),
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
}

impl std::fmt::Display for ArithError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "parse error: {m}"),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::ModuloByZero => write!(f, "modulo by zero"),
            Self::NotAnInteger { var, value } =>
                write!(f, "variable '{var}' is not an integer: '{value}'"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_division_by_zero() {
        assert_eq!(ArithError::DivisionByZero.to_string(), "division by zero");
    }

    #[test]
    fn display_modulo_by_zero() {
        assert_eq!(ArithError::ModuloByZero.to_string(), "modulo by zero");
    }

    #[test]
    fn display_parse_error_includes_message() {
        let e = ArithError::Parse("unexpected end of input".to_string());
        assert_eq!(e.to_string(), "parse error: unexpected end of input");
    }

    #[test]
    fn display_not_an_integer_quotes_var_and_value() {
        let e = ArithError::NotAnInteger {
            var: "x".to_string(),
            value: "abc".to_string(),
        };
        assert_eq!(e.to_string(), "variable 'x' is not an integer: 'abc'");
    }

    #[test]
    fn tokenize_single_number() {
        assert_eq!(tokenize("42").unwrap(), vec![ArithToken::Number(42)]);
    }

    #[test]
    fn tokenize_zero() {
        assert_eq!(tokenize("0").unwrap(), vec![ArithToken::Number(0)]);
    }

    #[test]
    fn tokenize_large_number() {
        assert_eq!(
            tokenize("9223372036854775807").unwrap(),
            vec![ArithToken::Number(i64::MAX)]
        );
    }

    #[test]
    fn tokenize_number_overflow_is_parse_error() {
        let err = tokenize("99999999999999999999").unwrap_err();
        assert!(matches!(err, ArithError::Parse(_)), "got {:?}", err);
    }

    #[test]
    fn tokenize_identifier() {
        assert_eq!(
            tokenize("foo").unwrap(),
            vec![ArithToken::Ident("foo".to_string())]
        );
    }

    #[test]
    fn tokenize_identifier_with_dollar_prefix_strips_dollar() {
        assert_eq!(
            tokenize("$foo").unwrap(),
            vec![ArithToken::Ident("foo".to_string())]
        );
    }

    #[test]
    fn tokenize_single_char_operators() {
        let input = "+ - * / % ( ) ! ? :";
        let expected = vec![
            ArithToken::Plus, ArithToken::Minus, ArithToken::Star,
            ArithToken::Slash, ArithToken::Percent,
            ArithToken::LParen, ArithToken::RParen,
            ArithToken::Bang, ArithToken::Question, ArithToken::Colon,
        ];
        assert_eq!(tokenize(input).unwrap(), expected);
    }

    #[test]
    fn tokenize_multi_char_operators() {
        let input = "== != <= >= && || < >";
        let expected = vec![
            ArithToken::Eq, ArithToken::Ne, ArithToken::Le, ArithToken::Ge,
            ArithToken::AndAnd, ArithToken::OrOr,
            ArithToken::Lt, ArithToken::Gt,
        ];
        assert_eq!(tokenize(input).unwrap(), expected);
    }

    #[test]
    fn tokenize_skips_whitespace() {
        assert_eq!(
            tokenize("  1   +   2  ").unwrap(),
            vec![ArithToken::Number(1), ArithToken::Plus, ArithToken::Number(2)]
        );
    }

    #[test]
    fn tokenize_unknown_char_is_parse_error() {
        let err = tokenize("1 @ 2").unwrap_err();
        assert!(matches!(err, ArithError::Parse(_)));
    }

    #[test]
    fn tokenize_single_amp_is_parse_error() {
        let err = tokenize("1 & 2").unwrap_err();
        assert!(matches!(err, ArithError::Parse(_)));
    }

    #[test]
    fn tokenize_single_pipe_is_parse_error() {
        let err = tokenize("1 | 2").unwrap_err();
        assert!(matches!(err, ArithError::Parse(_)));
    }
}
