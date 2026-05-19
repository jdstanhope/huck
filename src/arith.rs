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

pub fn parse(input: &str) -> Result<ArithExpr, ArithError> {
    let tokens = tokenize(input)?;
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_expr(0)?;
    if p.pos < p.tokens.len() {
        return Err(ArithError::Parse(format!(
            "unexpected token after expression: {:?}", p.tokens[p.pos]
        )));
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<ArithToken>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&ArithToken> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<ArithToken> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<ArithExpr, ArithError> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let op = match self.peek() {
                Some(t) => t.clone(),
                None => break,
            };
            // Ternary: `cond ? then : else`. Right-associative, lowest binding power.
            if op == ArithToken::Question && min_bp <= 1 {
                self.bump();
                let then_branch = self.parse_expr(0)?;
                match self.bump() {
                    Some(ArithToken::Colon) => {}
                    other => return Err(ArithError::Parse(format!(
                        "expected ':' in ternary, got {:?}", other
                    ))),
                }
                let else_branch = self.parse_expr(1)?;
                lhs = ArithExpr::Ternary(Box::new(lhs), Box::new(then_branch), Box::new(else_branch));
                continue;
            }
            let (lbp, rbp, make): (u8, u8, fn(Box<ArithExpr>, Box<ArithExpr>) -> ArithExpr) =
                match op {
                    ArithToken::OrOr   => (2, 3, ArithExpr::Or),
                    ArithToken::AndAnd => (4, 5, ArithExpr::And),
                    ArithToken::Eq     => (6, 7, ArithExpr::Eq),
                    ArithToken::Ne     => (6, 7, ArithExpr::Ne),
                    ArithToken::Lt     => (8, 9, ArithExpr::Lt),
                    ArithToken::Le     => (8, 9, ArithExpr::Le),
                    ArithToken::Gt     => (8, 9, ArithExpr::Gt),
                    ArithToken::Ge     => (8, 9, ArithExpr::Ge),
                    ArithToken::Plus   => (10, 11, ArithExpr::Add),
                    ArithToken::Minus  => (10, 11, ArithExpr::Sub),
                    ArithToken::Star   => (12, 13, ArithExpr::Mul),
                    ArithToken::Slash  => (12, 13, ArithExpr::Div),
                    ArithToken::Percent => (12, 13, ArithExpr::Mod),
                    _ => break,
                };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let rhs = self.parse_expr(rbp)?;
            lhs = make(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<ArithExpr, ArithError> {
        match self.bump() {
            Some(ArithToken::Number(n)) => Ok(ArithExpr::Num(n)),
            Some(ArithToken::Ident(s)) => Ok(ArithExpr::Var(s)),
            Some(ArithToken::Minus) => {
                let inner = self.parse_expr(14)?;
                Ok(ArithExpr::Neg(Box::new(inner)))
            }
            Some(ArithToken::Plus) => {
                self.parse_expr(14)
            }
            Some(ArithToken::Bang) => {
                let inner = self.parse_expr(14)?;
                Ok(ArithExpr::Not(Box::new(inner)))
            }
            Some(ArithToken::LParen) => {
                let inner = self.parse_expr(0)?;
                match self.bump() {
                    Some(ArithToken::RParen) => Ok(inner),
                    other => Err(ArithError::Parse(format!(
                        "expected ')', got {:?}", other
                    ))),
                }
            }
            Some(t) => Err(ArithError::Parse(format!(
                "expected expression, got {:?}", t
            ))),
            None => Err(ArithError::Parse("unexpected end of input".to_string())),
        }
    }
}

use crate::shell_state::Shell;

pub fn eval(expr: &ArithExpr, shell: &Shell) -> Result<i64, ArithError> {
    match expr {
        ArithExpr::Num(n) => Ok(*n),
        ArithExpr::Var(name) => {
            let raw = shell.get(name).unwrap_or("");
            if raw.is_empty() {
                Ok(0)
            } else {
                raw.parse::<i64>().map_err(|_| ArithError::NotAnInteger {
                    var: name.clone(),
                    value: raw.to_string(),
                })
            }
        }
        ArithExpr::Neg(e) => Ok(eval(e, shell)?.wrapping_neg()),
        ArithExpr::Not(e) => Ok(if eval(e, shell)? == 0 { 1 } else { 0 }),
        ArithExpr::Add(a, b) => Ok(eval(a, shell)?.wrapping_add(eval(b, shell)?)),
        ArithExpr::Sub(a, b) => Ok(eval(a, shell)?.wrapping_sub(eval(b, shell)?)),
        ArithExpr::Mul(a, b) => Ok(eval(a, shell)?.wrapping_mul(eval(b, shell)?)),
        ArithExpr::Div(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if rhs == 0 { return Err(ArithError::DivisionByZero); }
            Ok(lhs.wrapping_div(rhs))
        }
        ArithExpr::Mod(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if rhs == 0 { return Err(ArithError::ModuloByZero); }
            Ok(lhs.wrapping_rem(rhs))
        }
        ArithExpr::Eq(a, b) => Ok(bool_to_i64(eval(a, shell)? == eval(b, shell)?)),
        ArithExpr::Ne(a, b) => Ok(bool_to_i64(eval(a, shell)? != eval(b, shell)?)),
        ArithExpr::Lt(a, b) => Ok(bool_to_i64(eval(a, shell)? <  eval(b, shell)?)),
        ArithExpr::Le(a, b) => Ok(bool_to_i64(eval(a, shell)? <= eval(b, shell)?)),
        ArithExpr::Gt(a, b) => Ok(bool_to_i64(eval(a, shell)? >  eval(b, shell)?)),
        ArithExpr::Ge(a, b) => Ok(bool_to_i64(eval(a, shell)? >= eval(b, shell)?)),
        ArithExpr::And(a, b) => {
            if eval(a, shell)? == 0 {
                Ok(0)
            } else {
                Ok(bool_to_i64(eval(b, shell)? != 0))
            }
        }
        ArithExpr::Or(a, b) => {
            if eval(a, shell)? != 0 {
                Ok(1)
            } else {
                Ok(bool_to_i64(eval(b, shell)? != 0))
            }
        }
        ArithExpr::Ternary(c, t, e) => {
            if eval(c, shell)? != 0 {
                eval(t, shell)
            } else {
                eval(e, shell)
            }
        }
    }
}

fn bool_to_i64(b: bool) -> i64 {
    if b { 1 } else { 0 }
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

    fn n(x: i64) -> Box<ArithExpr> { Box::new(ArithExpr::Num(x)) }
    fn v(name: &str) -> Box<ArithExpr> { Box::new(ArithExpr::Var(name.to_string())) }

    #[test]
    fn parse_number_literal() {
        assert_eq!(parse("42").unwrap(), ArithExpr::Num(42));
    }

    #[test]
    fn parse_identifier() {
        assert_eq!(parse("foo").unwrap(), ArithExpr::Var("foo".to_string()));
    }

    #[test]
    fn parse_addition() {
        assert_eq!(parse("1+2").unwrap(), ArithExpr::Add(n(1), n(2)));
    }

    #[test]
    fn parse_subtraction_left_associative() {
        assert_eq!(
            parse("1-2-3").unwrap(),
            ArithExpr::Sub(Box::new(ArithExpr::Sub(n(1), n(2))), n(3))
        );
    }

    #[test]
    fn parse_multiplication_binds_tighter_than_addition() {
        assert_eq!(
            parse("1+2*3").unwrap(),
            ArithExpr::Add(n(1), Box::new(ArithExpr::Mul(n(2), n(3))))
        );
    }

    #[test]
    fn parse_parenthesized_overrides_precedence() {
        assert_eq!(
            parse("(1+2)*3").unwrap(),
            ArithExpr::Mul(Box::new(ArithExpr::Add(n(1), n(2))), n(3))
        );
    }

    #[test]
    fn parse_unary_minus() {
        assert_eq!(parse("-5").unwrap(), ArithExpr::Neg(n(5)));
    }

    #[test]
    fn parse_double_unary_minus() {
        assert_eq!(parse("--5").unwrap(), ArithExpr::Neg(Box::new(ArithExpr::Neg(n(5)))));
    }

    #[test]
    fn parse_unary_not() {
        assert_eq!(parse("!0").unwrap(), ArithExpr::Not(n(0)));
    }

    #[test]
    fn parse_comparison() {
        assert_eq!(parse("1<2").unwrap(), ArithExpr::Lt(n(1), n(2)));
    }

    #[test]
    fn parse_equality_lower_than_comparison() {
        assert_eq!(
            parse("1<2 == 1").unwrap(),
            ArithExpr::Eq(Box::new(ArithExpr::Lt(n(1), n(2))), n(1))
        );
    }

    #[test]
    fn parse_logical_and_binds_tighter_than_or() {
        assert_eq!(
            parse("a||b&&c").unwrap(),
            ArithExpr::Or(v("a"), Box::new(ArithExpr::And(v("b"), v("c"))))
        );
    }

    #[test]
    fn parse_ternary_right_associative() {
        assert_eq!(parse("a?b:c?d:e").unwrap(),
            ArithExpr::Ternary(v("a"), v("b"),
                Box::new(ArithExpr::Ternary(v("c"), v("d"), v("e"))))
        );
    }

    #[test]
    fn parse_empty_is_error() {
        assert!(matches!(parse("").unwrap_err(), ArithError::Parse(_)));
    }

    #[test]
    fn parse_trailing_junk_is_error() {
        assert!(matches!(parse("1+2 3").unwrap_err(), ArithError::Parse(_)));
    }

    #[test]
    fn parse_unbalanced_paren_is_error() {
        assert!(matches!(parse("(1+2").unwrap_err(), ArithError::Parse(_)));
    }

    #[test]
    fn parse_missing_rhs_is_error() {
        assert!(matches!(parse("1+").unwrap_err(), ArithError::Parse(_)));
    }

    #[test]
    fn parse_strips_dollar_on_var() {
        assert_eq!(parse("$x + 1").unwrap(),
            ArithExpr::Add(v("x"), n(1)));
    }

    use crate::shell_state::Shell;

    fn eval_str(s: &str, shell: &Shell) -> Result<i64, ArithError> {
        eval(&parse(s).unwrap(), shell)
    }

    #[test]
    fn eval_number_literal() {
        let s = Shell::new();
        assert_eq!(eval_str("42", &s).unwrap(), 42);
    }

    #[test]
    fn eval_addition() {
        let s = Shell::new();
        assert_eq!(eval_str("1+2", &s).unwrap(), 3);
    }

    #[test]
    fn eval_precedence() {
        let s = Shell::new();
        assert_eq!(eval_str("2+3*4", &s).unwrap(), 14);
        assert_eq!(eval_str("(2+3)*4", &s).unwrap(), 20);
    }

    #[test]
    fn eval_subtraction_left_assoc() {
        let s = Shell::new();
        assert_eq!(eval_str("1-2-3", &s).unwrap(), -4);
    }

    #[test]
    fn eval_unary_minus() {
        let s = Shell::new();
        assert_eq!(eval_str("-5", &s).unwrap(), -5);
        assert_eq!(eval_str("--5", &s).unwrap(), 5);
    }

    #[test]
    fn eval_division_truncates_toward_zero() {
        let s = Shell::new();
        assert_eq!(eval_str("7/2", &s).unwrap(), 3);
        assert_eq!(eval_str("-7/2", &s).unwrap(), -3);
    }

    #[test]
    fn eval_modulo() {
        let s = Shell::new();
        assert_eq!(eval_str("7%3", &s).unwrap(), 1);
        assert_eq!(eval_str("-7%3", &s).unwrap(), -1);
    }

    #[test]
    fn eval_division_by_zero() {
        let s = Shell::new();
        assert_eq!(eval_str("1/0", &s).unwrap_err(), ArithError::DivisionByZero);
    }

    #[test]
    fn eval_modulo_by_zero() {
        let s = Shell::new();
        assert_eq!(eval_str("1%0", &s).unwrap_err(), ArithError::ModuloByZero);
    }

    #[test]
    fn eval_comparison_returns_one_or_zero() {
        let s = Shell::new();
        assert_eq!(eval_str("1<2", &s).unwrap(), 1);
        assert_eq!(eval_str("2<1", &s).unwrap(), 0);
        assert_eq!(eval_str("1==1", &s).unwrap(), 1);
        assert_eq!(eval_str("1!=1", &s).unwrap(), 0);
    }

    #[test]
    fn eval_logical_not() {
        let s = Shell::new();
        assert_eq!(eval_str("!0", &s).unwrap(), 1);
        assert_eq!(eval_str("!5", &s).unwrap(), 0);
        assert_eq!(eval_str("!!5", &s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_and_short_circuits() {
        let s = Shell::new();
        assert_eq!(eval_str("0 && 1/0", &s).unwrap(), 0);
    }

    #[test]
    fn eval_logical_or_short_circuits() {
        let s = Shell::new();
        assert_eq!(eval_str("1 || 1/0", &s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_and_returns_one_when_both_truthy() {
        let s = Shell::new();
        assert_eq!(eval_str("5 && 3", &s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_or_returns_one_when_either_truthy() {
        let s = Shell::new();
        assert_eq!(eval_str("0 || 3", &s).unwrap(), 1);
        assert_eq!(eval_str("0 || 0", &s).unwrap(), 0);
    }

    #[test]
    fn eval_ternary() {
        let s = Shell::new();
        assert_eq!(eval_str("1 ? 42 : 99", &s).unwrap(), 42);
        assert_eq!(eval_str("0 ? 42 : 99", &s).unwrap(), 99);
    }

    #[test]
    fn eval_overflow_wraps() {
        let s = Shell::new();
        let max = i64::MAX.to_string();
        let expr = format!("{max} + 1");
        assert_eq!(eval_str(&expr, &s).unwrap(), i64::MIN);
    }

    #[test]
    fn eval_unset_var_is_zero() {
        let s = Shell::new();
        assert_eq!(eval_str("HUCK_TEST_UNSET_ARITH + 5", &s).unwrap(), 5);
    }

    #[test]
    fn eval_set_var_lookup() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_X", "10".to_string());
        assert_eq!(eval_str("HUCK_TEST_ARITH_X * 2", &s).unwrap(), 20);
    }

    #[test]
    fn eval_var_with_dollar_prefix_same_as_bare() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_Y", "7".to_string());
        assert_eq!(eval_str("$HUCK_TEST_ARITH_Y + 1", &s).unwrap(), 8);
    }

    #[test]
    fn eval_empty_var_is_zero() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_EMPTY", "".to_string());
        assert_eq!(eval_str("HUCK_TEST_ARITH_EMPTY + 3", &s).unwrap(), 3);
    }

    #[test]
    fn eval_non_integer_var_is_error() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_BAD", "abc".to_string());
        let err = eval_str("HUCK_TEST_ARITH_BAD + 1", &s).unwrap_err();
        assert_eq!(
            err,
            ArithError::NotAnInteger {
                var: "HUCK_TEST_ARITH_BAD".to_string(),
                value: "abc".to_string()
            }
        );
    }
}
