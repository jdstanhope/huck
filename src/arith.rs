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
    // v38 — bitwise & shift:
    Amp, Pipe, Caret, Tilde,
    Shl, Shr,
    // v38 — power:
    Power,
    // v38 — assignment:
    Assign,
    PlusEq, MinusEq, StarEq,
    SlashEq, PercentEq,
    ShlEq, ShrEq,
    AmpEq, CaretEq, PipeEq,
    // v38 — inc/dec:
    PlusPlus, MinusMinus,
}

/// Parses hex digits 0-9, a-f, A-F after the `0x` / `0X` prefix has
/// been consumed. Returns the i64 value. Errors on no digits, invalid
/// digits, or out-of-range value.
fn parse_hex_digits(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<i64, ArithError> {
    let mut s = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_hexdigit() {
            s.push(c);
            chars.next();
        } else {
            break;
        }
    }
    if s.is_empty() {
        return Err(ArithError::Parse("hex literal requires at least one digit".to_string()));
    }
    i64::from_str_radix(&s, 16).map_err(|_|
        ArithError::Parse(format!("hex literal out of range: 0x{s}")))
}

/// Parses base-N digits after the `N#` prefix has been consumed. The
/// digit alphabet (matches bash):
///   0-9 → 0-9
///   a-z → 10-35
///   A-Z → 36-61
///   @   → 62
///   _   → 63
/// For bases ≤ 36, a-z and A-Z are both valid as 10-35 (case-insensitive).
/// For bases > 36, a-z (10-35) and A-Z (36-61) are distinct.
fn parse_base_n_digits(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    base: u32,
) -> Result<i64, ArithError> {
    let mut value: i64 = 0;
    let mut any_digit = false;
    while let Some(&c) = chars.peek() {
        let digit = match c {
            '0'..='9' => (c as u32) - ('0' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 10,
            'A'..='Z' => {
                if base <= 36 {
                    // Case-insensitive: A-Z → 10-35.
                    (c as u32) - ('A' as u32) + 10
                } else {
                    // Case-sensitive: A-Z → 36-61.
                    (c as u32) - ('A' as u32) + 36
                }
            }
            '@' => 62,
            '_' => 63,
            _ => break,
        };
        if digit >= base {
            return Err(ArithError::Parse(format!(
                "invalid digit for base {base}: '{c}'"
            )));
        }
        value = value
            .checked_mul(base as i64)
            .and_then(|v| v.checked_add(digit as i64))
            .ok_or_else(|| ArithError::Parse(format!(
                "base-{base} literal out of range"
            )))?;
        any_digit = true;
        chars.next();
    }
    if !any_digit {
        return Err(ArithError::Parse(format!(
            "base-{base} literal requires at least one digit"
        )));
    }
    Ok(value)
}

pub(crate) fn tokenize(input: &str) -> Result<Vec<ArithToken>, ArithError> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' | '\r' => { chars.next(); }
            '0'..='9' => {
                // Read greedy leading decimal digits.
                let mut digits = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() { digits.push(d); chars.next(); } else { break; }
                }
                let n: i64 = if chars.peek() == Some(&'#') {
                    // Base-N literal: leading digits parsed as decimal base.
                    chars.next();
                    let base: u32 = digits.parse()
                        .map_err(|_| ArithError::Parse(format!("invalid base: {digits}")))?;
                    if !(2..=64).contains(&base) {
                        return Err(ArithError::Parse(format!(
                            "base must be 2-64, got {base}"
                        )));
                    }
                    parse_base_n_digits(&mut chars, base)?
                } else if digits == "0" && matches!(chars.peek(), Some('x') | Some('X')) {
                    // Hex literal: 0x... / 0X...
                    chars.next();
                    parse_hex_digits(&mut chars)?
                } else if digits.len() > 1 && digits.starts_with('0') {
                    // Octal literal: 010 → 8. All digits must be 0-7.
                    i64::from_str_radix(&digits, 8)
                        .map_err(|_| ArithError::Parse(format!(
                            "invalid octal literal: {digits}"
                        )))?
                } else {
                    digits.parse()
                        .map_err(|_| ArithError::Parse(format!(
                            "integer literal out of range: {digits}"
                        )))?
                };
                out.push(ArithToken::Number(n));
            }
            '$' => {
                chars.next();
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d == '_' || d.is_ascii_alphanumeric() {
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
            '+' => {
                chars.next();
                match chars.peek() {
                    Some('+') => { chars.next(); out.push(ArithToken::PlusPlus); }
                    Some('=') => { chars.next(); out.push(ArithToken::PlusEq); }
                    _ => out.push(ArithToken::Plus),
                }
            }
            '-' => {
                chars.next();
                match chars.peek() {
                    Some('-') => { chars.next(); out.push(ArithToken::MinusMinus); }
                    Some('=') => { chars.next(); out.push(ArithToken::MinusEq); }
                    _ => out.push(ArithToken::Minus),
                }
            }
            '*' => {
                chars.next();
                match chars.peek() {
                    Some('*') => { chars.next(); out.push(ArithToken::Power); }
                    Some('=') => { chars.next(); out.push(ArithToken::StarEq); }
                    _ => out.push(ArithToken::Star),
                }
            }
            '/' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::SlashEq);
                } else {
                    out.push(ArithToken::Slash);
                }
            }
            '%' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::PercentEq);
                } else {
                    out.push(ArithToken::Percent);
                }
            }
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
                    out.push(ArithToken::Assign);
                }
            }
            '<' => {
                chars.next();
                match chars.peek() {
                    Some('<') => {
                        chars.next();
                        if chars.peek() == Some(&'=') {
                            chars.next();
                            out.push(ArithToken::ShlEq);
                        } else {
                            out.push(ArithToken::Shl);
                        }
                    }
                    Some('=') => { chars.next(); out.push(ArithToken::Le); }
                    _ => out.push(ArithToken::Lt),
                }
            }
            '>' => {
                chars.next();
                match chars.peek() {
                    Some('>') => {
                        chars.next();
                        if chars.peek() == Some(&'=') {
                            chars.next();
                            out.push(ArithToken::ShrEq);
                        } else {
                            out.push(ArithToken::Shr);
                        }
                    }
                    Some('=') => { chars.next(); out.push(ArithToken::Ge); }
                    _ => out.push(ArithToken::Gt),
                }
            }
            '&' => {
                chars.next();
                match chars.peek() {
                    Some('&') => { chars.next(); out.push(ArithToken::AndAnd); }
                    Some('=') => { chars.next(); out.push(ArithToken::AmpEq); }
                    _ => out.push(ArithToken::Amp),
                }
            }
            '|' => {
                chars.next();
                match chars.peek() {
                    Some('|') => { chars.next(); out.push(ArithToken::OrOr); }
                    Some('=') => { chars.next(); out.push(ArithToken::PipeEq); }
                    _ => out.push(ArithToken::Pipe),
                }
            }
            '^' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::CaretEq);
                } else {
                    out.push(ArithToken::Caret);
                }
            }
            '~' => {
                chars.next();
                out.push(ArithToken::Tilde);
            }
            other => {
                return Err(ArithError::Parse(format!("unexpected character: {other:?}")));
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]  // variants constructed by parser in Task 3
pub enum AssignOp {
    Set,    // =
    Add,    // +=
    Sub,    // -=
    Mul,    // *=
    Div,    // /=
    Mod,    // %=
    Shl,    // <<=
    Shr,    // >>=
    BitAnd, // &=
    BitXor, // ^=
    BitOr,  // |=
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
    // v38 — bitwise binops:
    #[allow(dead_code)]  // constructed by parser in Task 3
    BitAnd(Box<ArithExpr>, Box<ArithExpr>),
    #[allow(dead_code)]  // constructed by parser in Task 3
    BitOr(Box<ArithExpr>, Box<ArithExpr>),
    #[allow(dead_code)]  // constructed by parser in Task 3
    BitXor(Box<ArithExpr>, Box<ArithExpr>),
    #[allow(dead_code)]  // constructed by parser in Task 3
    BitNot(Box<ArithExpr>),
    #[allow(dead_code)]  // constructed by parser in Task 3
    Shl(Box<ArithExpr>, Box<ArithExpr>),
    #[allow(dead_code)]  // constructed by parser in Task 3
    Shr(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — power (right-associative):
    #[allow(dead_code)]  // constructed by parser in Task 3
    Pow(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — assignment (LHS must be a Var; enforced at parse time):
    #[allow(dead_code)]  // constructed by parser in Task 3
    Assign { name: String, op: AssignOp, rhs: Box<ArithExpr> },
    // v38 — pre/post inc/dec (LHS must be a Var):
    #[allow(dead_code)]  // constructed by parser in Task 3
    PreInc(String),
    #[allow(dead_code)]  // constructed by parser in Task 3
    PreDec(String),
    #[allow(dead_code)]  // constructed by parser in Task 3
    PostInc(String),
    #[allow(dead_code)]  // constructed by parser in Task 3
    PostDec(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithError {
    Parse(String),
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    #[allow(dead_code)]  // constructed by evaluator in Task 4
    NegativeExponent,
    #[allow(dead_code)]  // constructed by evaluator in Task 4
    ShiftCountOutOfRange { count: i64 },
}

impl std::fmt::Display for ArithError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "{m}"),
            Self::DivisionByZero => write!(f, "division by zero"),
            Self::ModuloByZero => write!(f, "modulo by zero"),
            Self::NotAnInteger { var, value } =>
                write!(f, "variable '{var}' is not an integer: '{value}'"),
            Self::NegativeExponent => write!(f, "exponentiation with negative exponent"),
            Self::ShiftCountOutOfRange { count } =>
                write!(f, "shift count out of range: {count}"),
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

/// A row in the Pratt-parser operator table: left binding power, right
/// binding power, and the AST constructor for the binary node.
type BinOpEntry = (u8, u8, fn(Box<ArithExpr>, Box<ArithExpr>) -> ArithExpr);

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
        while let Some(op) = self.peek().cloned() {
            // Ternary: `cond ? then : else`. Right-associative, lowest binding power.
            if op == ArithToken::Question && min_bp <= 1 {
                self.bump();
                let then_branch = self.parse_expr(0)?;
                match self.bump() {
                    Some(ArithToken::Colon) => {}
                    other => return Err(ArithError::Parse(format!(
                        "expected ':' in ternary, got {other:?}"
                    ))),
                }
                let else_branch = self.parse_expr(1)?;
                lhs = ArithExpr::Ternary(Box::new(lhs), Box::new(then_branch), Box::new(else_branch));
                continue;
            }
            let (lbp, rbp, make): BinOpEntry = match op {
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
            let raw = shell.lookup_var(name).unwrap_or_default();
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
        // v38 variants — implementation in Task 4
        ArithExpr::BitAnd(_, _) => unreachable!("BitAnd: Task 4"),
        ArithExpr::BitOr(_, _) => unreachable!("BitOr: Task 4"),
        ArithExpr::BitXor(_, _) => unreachable!("BitXor: Task 4"),
        ArithExpr::BitNot(_) => unreachable!("BitNot: Task 4"),
        ArithExpr::Shl(_, _) => unreachable!("Shl: Task 4"),
        ArithExpr::Shr(_, _) => unreachable!("Shr: Task 4"),
        ArithExpr::Pow(_, _) => unreachable!("Pow: Task 4"),
        ArithExpr::Assign { .. } => unreachable!("Assign: Task 4"),
        ArithExpr::PreInc(_) => unreachable!("PreInc: Task 4"),
        ArithExpr::PreDec(_) => unreachable!("PreDec: Task 4"),
        ArithExpr::PostInc(_) => unreachable!("PostInc: Task 4"),
        ArithExpr::PostDec(_) => unreachable!("PostDec: Task 4"),
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
    fn display_parse_error_is_bare_message() {
        let e = ArithError::Parse("unexpected end of input".to_string());
        assert_eq!(e.to_string(), "unexpected end of input");
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
    fn tokenize_single_amp_is_bitwise_and() {
        // v38: bare & is now bitwise AND (was: parse error).
        assert_eq!(tokenize("1 & 2").unwrap(), vec![
            ArithToken::Number(1), ArithToken::Amp, ArithToken::Number(2),
        ]);
    }

    #[test]
    fn tokenize_single_pipe_is_bitwise_or() {
        // v38: bare | is now bitwise OR (was: parse error).
        assert_eq!(tokenize("1 | 2").unwrap(), vec![
            ArithToken::Number(1), ArithToken::Pipe, ArithToken::Number(2),
        ]);
    }

    #[test]
    fn tokenize_hex_literal() {
        assert_eq!(tokenize("0x10").unwrap(), vec![ArithToken::Number(16)]);
    }

    #[test]
    fn tokenize_hex_literal_uppercase() {
        assert_eq!(tokenize("0X1F").unwrap(), vec![ArithToken::Number(31)]);
    }

    #[test]
    fn tokenize_octal_literal() {
        assert_eq!(tokenize("010").unwrap(), vec![ArithToken::Number(8)]);
    }

    #[test]
    fn tokenize_octal_invalid_digit_errors() {
        // 08 has a digit (8) that's invalid for octal.
        assert!(tokenize("08").is_err());
    }

    #[test]
    fn tokenize_base_n_binary() {
        assert_eq!(tokenize("2#1010").unwrap(), vec![ArithToken::Number(10)]);
    }

    #[test]
    fn tokenize_base_n_hex_via_pound() {
        assert_eq!(tokenize("16#FF").unwrap(), vec![ArithToken::Number(255)]);
    }

    #[test]
    fn tokenize_base_n_invalid_base_low_errors() {
        assert!(tokenize("1#0").is_err());
    }

    #[test]
    fn tokenize_base_n_invalid_base_high_errors() {
        assert!(tokenize("65#0").is_err());
    }

    #[test]
    fn tokenize_base_n_invalid_digit_errors() {
        // Base 8 cannot have digit 9.
        assert!(tokenize("8#9").is_err());
    }

    #[test]
    fn tokenize_bitwise_operators() {
        assert_eq!(
            tokenize("&|^~<<>>").unwrap(),
            vec![
                ArithToken::Amp, ArithToken::Pipe,
                ArithToken::Caret, ArithToken::Tilde,
                ArithToken::Shl, ArithToken::Shr,
            ]
        );
    }

    #[test]
    fn tokenize_power_operator() {
        assert_eq!(tokenize("2**3").unwrap(), vec![
            ArithToken::Number(2), ArithToken::Power, ArithToken::Number(3),
        ]);
    }

    #[test]
    fn tokenize_compound_assignments() {
        // = += -= *= /= %= <<= >>= &= ^= |=
        let input = "= += -= *= /= %= <<= >>= &= ^= |=";
        let tokens = tokenize(input).unwrap();
        assert_eq!(tokens, vec![
            ArithToken::Assign,
            ArithToken::PlusEq, ArithToken::MinusEq, ArithToken::StarEq,
            ArithToken::SlashEq, ArithToken::PercentEq,
            ArithToken::ShlEq, ArithToken::ShrEq,
            ArithToken::AmpEq, ArithToken::CaretEq, ArithToken::PipeEq,
        ]);
    }

    #[test]
    fn tokenize_inc_dec_operators() {
        assert_eq!(tokenize("++ --").unwrap(), vec![
            ArithToken::PlusPlus, ArithToken::MinusMinus,
        ]);
    }

    #[test]
    fn tokenize_distinguishes_eq_from_assign() {
        assert_eq!(tokenize("==").unwrap(), vec![ArithToken::Eq]);
        assert_eq!(tokenize("=").unwrap(), vec![ArithToken::Assign]);
    }

    #[test]
    fn tokenize_distinguishes_lt_from_shl() {
        assert_eq!(tokenize("<").unwrap(), vec![ArithToken::Lt]);
        assert_eq!(tokenize("<<").unwrap(), vec![ArithToken::Shl]);
        assert_eq!(tokenize("<<=").unwrap(), vec![ArithToken::ShlEq]);
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
