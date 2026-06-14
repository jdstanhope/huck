//! Arithmetic expansion: AST, parser, and evaluator for `$((expr))`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArithToken {
    Number(i64),
    Ident(String),
    LParen, RParen,
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    AndAnd, OrOr, Bang,
    Question, Colon, Comma,
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
    // array subscripts: `name[subscript]`. LBracket carries the RAW inner
    // source text between the brackets (used as a literal key for
    // associative arrays); the bracketed tokens are still emitted between
    // LBracket/RBracket so the subscript can be parsed as an arith
    // expression for indexed arrays.
    LBracket(String), RBracket,
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
            // Unreachable post-v93 for the three arith *contexts* (`(( ))`,
            // `$(( ))`, arith-`for`): those expand `$`-forms before calling
            // `arith::parse`. Kept defensive for any other `arith::parse`
            // caller (e.g. integer-coerce on a value still bearing a `$`).
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
            '[' => {
                // Capture the RAW inner text up to the matching ']' (for the
                // associative-key case), tracking bracket nesting so that
                // e.g. `a[b[0]]` captures `b[0]`. The inner tokens are still
                // produced by the main loop after this LBracket.
                chars.next();
                let mut raw = String::new();
                let mut depth = 1i32;
                let mut lookahead = chars.clone();
                while let Some(&d) = lookahead.peek() {
                    match d {
                        '[' => depth += 1,
                        ']' => {
                            depth -= 1;
                            if depth == 0 { break; }
                        }
                        _ => {}
                    }
                    raw.push(d);
                    lookahead.next();
                }
                out.push(ArithToken::LBracket(raw));
            }
            ']' => { chars.next(); out.push(ArithToken::RBracket); }
            ',' => { chars.next(); out.push(ArithToken::Comma); }
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

/// An assignable / incrementable target: either a scalar variable or an
/// array element. For array elements, `subscript` is the parsed arithmetic
/// expression (used for INDEXED arrays — arith-evaluated to an index) and
/// `subscript_raw` is the raw inner source text (used for ASSOCIATIVE
/// arrays — a literal key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LValue {
    Scalar(String),
    Element { name: String, subscript: Box<ArithExpr>, subscript_raw: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithExpr {
    Num(i64),
    Var(String),
    /// `name[subscript]` array-element READ. See `LValue::Element` for the
    /// indexed-vs-associative interpretation of `subscript`/`subscript_raw`.
    Index { name: String, subscript: Box<ArithExpr>, subscript_raw: String },
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
    /// `L , R` — evaluate L (for side effects), then R; value is R. Lowest
    /// precedence. (M-108)
    Comma(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — bitwise binops:
    BitAnd(Box<ArithExpr>, Box<ArithExpr>),
    BitOr(Box<ArithExpr>, Box<ArithExpr>),
    BitXor(Box<ArithExpr>, Box<ArithExpr>),
    BitNot(Box<ArithExpr>),
    Shl(Box<ArithExpr>, Box<ArithExpr>),
    Shr(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — power (right-associative):
    Pow(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — assignment (LHS must be an lvalue; enforced at parse time):
    Assign { target: LValue, op: AssignOp, rhs: Box<ArithExpr> },
    // v38 — pre/post inc/dec (LHS must be an lvalue):
    PreInc(LValue),
    PreDec(LValue),
    PostInc(LValue),
    PostDec(LValue),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithError {
    Parse(String),
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
    ReadonlyVar(String),
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
            Self::ReadonlyVar(name) => write!(f, "{name}: readonly variable"),
        }
    }
}

pub fn parse(input: &str) -> Result<ArithExpr, ArithError> {
    let tokens = tokenize(input)?;
    let mut p = Parser { tokens, pos: 0 };
    let expr = p.parse_comma_expr()?;
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

/// Maps an assignment token to its corresponding AssignOp variant.
/// Returns None for non-assignment tokens.
/// Converts a parsed primary expression into an assignable lvalue, or
/// `None` if it isn't a scalar var or array element.
fn expr_to_lvalue(e: ArithExpr) -> Option<LValue> {
    match e {
        ArithExpr::Var(name) => Some(LValue::Scalar(name)),
        ArithExpr::Index { name, subscript, subscript_raw } =>
            Some(LValue::Element { name, subscript, subscript_raw }),
        _ => None,
    }
}

fn assign_op_from_token(t: &ArithToken) -> Option<AssignOp> {
    match t {
        ArithToken::Assign     => Some(AssignOp::Set),
        ArithToken::PlusEq     => Some(AssignOp::Add),
        ArithToken::MinusEq    => Some(AssignOp::Sub),
        ArithToken::StarEq     => Some(AssignOp::Mul),
        ArithToken::SlashEq    => Some(AssignOp::Div),
        ArithToken::PercentEq  => Some(AssignOp::Mod),
        ArithToken::ShlEq      => Some(AssignOp::Shl),
        ArithToken::ShrEq      => Some(AssignOp::Shr),
        ArithToken::AmpEq      => Some(AssignOp::BitAnd),
        ArithToken::CaretEq    => Some(AssignOp::BitXor),
        ArithToken::PipeEq     => Some(AssignOp::BitOr),
        _ => None,
    }
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

    /// Parse a comma-separated sequence of full expressions (each
    /// `parse_expr(0)`); left-associative, value is the last. Comma is the
    /// lowest-precedence arithmetic operator, so it lives ABOVE the Pratt loop
    /// — this keeps every existing binding power untouched and makes
    /// `a = 1, 2` parse as `(a=1), 2`. (M-108)
    fn parse_comma_expr(&mut self) -> Result<ArithExpr, ArithError> {
        let mut lhs = self.parse_expr(0)?;
        while self.peek() == Some(&ArithToken::Comma) {
            self.bump();
            let rhs = self.parse_expr(0)?;
            lhs = ArithExpr::Comma(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<ArithExpr, ArithError> {
        let mut lhs = self.parse_prefix()?;
        while let Some(op) = self.peek().cloned() {

            // 1. Postfix ++/-- (BP 27 — highest). Must come before infix
            //    handling so a++ + 1 parses as (a++) + 1.
            if matches!(op, ArithToken::PlusPlus | ArithToken::MinusMinus) && 27 >= min_bp {
                self.bump();
                let target = match expr_to_lvalue(lhs) {
                    Some(t) => t,
                    None => return Err(ArithError::Parse(
                        "postfix ++/-- requires variable on LHS".to_string()
                    )),
                };
                lhs = match op {
                    ArithToken::PlusPlus => ArithExpr::PostInc(target),
                    _ => ArithExpr::PostDec(target),
                };
                continue;
            }

            // 2. Assignment (lbp = 2, rbp = 1 — right-associative).
            //    LHS must be a Var.
            if let Some(assign_op) = assign_op_from_token(&op) {
                if 2 < min_bp { break; }
                self.bump();
                let target = match expr_to_lvalue(lhs) {
                    Some(t) => t,
                    None => return Err(ArithError::Parse(
                        "assignment requires variable on LHS".to_string()
                    )),
                };
                let rhs = self.parse_expr(1)?;  // rbp = 1 allows cascading assigns
                lhs = ArithExpr::Assign { target, op: assign_op, rhs: Box::new(rhs) };
                continue;
            }

            // 3. Ternary (BP 3 — right-associative, special-cased like assignment).
            if op == ArithToken::Question && 3 >= min_bp {
                self.bump();
                let then_branch = self.parse_expr(0)?;
                match self.bump() {
                    Some(ArithToken::Colon) => {}
                    other => return Err(ArithError::Parse(format!(
                        "expected ':' in ternary, got {other:?}"
                    ))),
                }
                let else_branch = self.parse_expr(3)?;  // rbp = 3 for right-assoc
                lhs = ArithExpr::Ternary(
                    Box::new(lhs), Box::new(then_branch), Box::new(else_branch)
                );
                continue;
            }

            // 4. Power ** (lbp = 25, rbp = 24 — right-associative).
            if op == ArithToken::Power && 25 >= min_bp {
                self.bump();
                let rhs = self.parse_expr(24)?;
                lhs = ArithExpr::Pow(Box::new(lhs), Box::new(rhs));
                continue;
            }

            // 5. Standard left-associative binops via the precedence table.
            let (lbp, rbp, make): BinOpEntry = match op {
                ArithToken::OrOr    => (4, 5, ArithExpr::Or),
                ArithToken::AndAnd  => (6, 7, ArithExpr::And),
                ArithToken::Pipe    => (8, 9, ArithExpr::BitOr),
                ArithToken::Caret   => (10, 11, ArithExpr::BitXor),
                ArithToken::Amp     => (12, 13, ArithExpr::BitAnd),
                ArithToken::Eq      => (14, 15, ArithExpr::Eq),
                ArithToken::Ne      => (14, 15, ArithExpr::Ne),
                ArithToken::Lt      => (16, 17, ArithExpr::Lt),
                ArithToken::Le      => (16, 17, ArithExpr::Le),
                ArithToken::Gt      => (16, 17, ArithExpr::Gt),
                ArithToken::Ge      => (16, 17, ArithExpr::Ge),
                ArithToken::Shl     => (18, 19, ArithExpr::Shl),
                ArithToken::Shr     => (18, 19, ArithExpr::Shr),
                ArithToken::Plus    => (20, 21, ArithExpr::Add),
                ArithToken::Minus   => (20, 21, ArithExpr::Sub),
                ArithToken::Star    => (22, 23, ArithExpr::Mul),
                ArithToken::Slash   => (22, 23, ArithExpr::Div),
                ArithToken::Percent => (22, 23, ArithExpr::Mod),
                _ => break,
            };
            if lbp < min_bp { break; }
            self.bump();
            let rhs = self.parse_expr(rbp)?;
            lhs = make(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    /// Parses `[subscript]` given the current token is `LBracket`. Returns
    /// the parsed subscript expression (for indexed arrays) and the raw
    /// inner source text (for associative-array keys).
    fn parse_subscript(&mut self) -> Result<(ArithExpr, String), ArithError> {
        let raw = match self.bump() {
            Some(ArithToken::LBracket(raw)) => raw,
            other => return Err(ArithError::Parse(format!(
                "expected '[', got {other:?}"
            ))),
        };
        let subscript = self.parse_comma_expr()?;
        match self.bump() {
            Some(ArithToken::RBracket) => {}
            other => return Err(ArithError::Parse(format!(
                "expected ']' after array subscript, got {other:?}"
            ))),
        }
        Ok((subscript, raw))
    }

    fn parse_prefix(&mut self) -> Result<ArithExpr, ArithError> {
        match self.bump() {
            Some(ArithToken::Number(n)) => Ok(ArithExpr::Num(n)),
            Some(ArithToken::Ident(s)) => {
                // `name[subscript]` → array-element reference.
                if matches!(self.peek(), Some(ArithToken::LBracket(_))) {
                    let (subscript, subscript_raw) = self.parse_subscript()?;
                    Ok(ArithExpr::Index { name: s, subscript: Box::new(subscript), subscript_raw })
                } else {
                    Ok(ArithExpr::Var(s))
                }
            }
            Some(ArithToken::Minus) => {
                let inner = self.parse_expr(26)?;
                Ok(ArithExpr::Neg(Box::new(inner)))
            }
            Some(ArithToken::Plus) => {
                self.parse_expr(26)
            }
            Some(ArithToken::Bang) => {
                let inner = self.parse_expr(26)?;
                Ok(ArithExpr::Not(Box::new(inner)))
            }
            Some(ArithToken::Tilde) => {
                let inner = self.parse_expr(26)?;
                Ok(ArithExpr::BitNot(Box::new(inner)))
            }
            Some(ArithToken::PlusPlus) => {
                // ++name / ++name[sub]: prefix increment requires an lvalue.
                let inner = self.parse_prefix()?;
                match expr_to_lvalue(inner) {
                    Some(target) => Ok(ArithExpr::PreInc(target)),
                    None => Err(ArithError::Parse(
                        "prefix ++ requires variable".to_string()
                    )),
                }
            }
            Some(ArithToken::MinusMinus) => {
                // --name / --name[sub]: prefix decrement requires an lvalue.
                let inner = self.parse_prefix()?;
                match expr_to_lvalue(inner) {
                    Some(target) => Ok(ArithExpr::PreDec(target)),
                    None => Err(ArithError::Parse(
                        "prefix -- requires variable".to_string()
                    )),
                }
            }
            Some(ArithToken::LParen) => {
                let inner = self.parse_comma_expr()?;
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

/// Reads a shell variable's current i64 value. Returns 0 if unset or
/// empty (matches existing eval Var behavior).
fn read_var_i64(shell: &Shell, name: &str) -> Result<i64, ArithError> {
    let raw = shell.lookup_var(name).unwrap_or_default();
    if raw.is_empty() {
        return Ok(0);
    }
    raw.parse::<i64>().map_err(|_| ArithError::NotAnInteger {
        var: name.to_string(),
        value: raw,
    })
}

/// Writes an i64 back to a shell variable as a decimal string.
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) -> Result<(), ArithError> {
    shell.try_set(name, value.to_string())
        .map_err(|_| ArithError::ReadonlyVar(name.to_string()))
}

/// Arith-evaluates an element's raw string value to an i64, exactly like a
/// scalar variable: empty/unset → 0, an integer literal → its value, and an
/// arbitrary arith expression (e.g. "1+1") → recursively evaluated.
fn element_string_to_i64(
    shell: &mut Shell,
    name: &str,
    raw: Option<String>,
) -> Result<i64, ArithError> {
    let raw = raw.unwrap_or_default();
    if raw.is_empty() {
        return Ok(0);
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Ok(n);
    }
    // Non-numeric: arith-evaluate recursively (an element may hold "1+1").
    let parsed = parse(&raw).map_err(|_| ArithError::NotAnInteger {
        var: name.to_string(),
        value: raw.clone(),
    })?;
    eval(&parsed, shell)
}

/// Reads the current i64 value of an lvalue (scalar var or array element).
fn read_lvalue_i64(shell: &mut Shell, target: &LValue) -> Result<i64, ArithError> {
    match target {
        LValue::Scalar(name) => read_var_i64(shell, name),
        LValue::Element { name, subscript, subscript_raw } => {
            if shell.get_associative(name).is_some() {
                let key = subscript_raw.clone();
                let raw = shell.lookup_associative_element(name, &key);
                element_string_to_i64(shell, name, raw)
            } else {
                let idx = eval(subscript, shell)?;
                let raw = shell.lookup_array_element(name, idx as usize);
                element_string_to_i64(shell, name, raw)
            }
        }
    }
}

/// Writes an i64 to an lvalue (scalar var or array element).
fn write_lvalue_i64(shell: &mut Shell, target: &LValue, value: i64) -> Result<(), ArithError> {
    match target {
        LValue::Scalar(name) => write_var_i64(shell, name, value),
        LValue::Element { name, subscript, subscript_raw } => {
            if shell.get_associative(name).is_some() {
                shell
                    .set_associative_element(name, subscript_raw.clone(), value.to_string())
                    .map_err(|_| ArithError::ReadonlyVar(name.to_string()))
            } else {
                let idx = eval(subscript, shell)?;
                shell
                    .set_array_element(name, idx as usize, value.to_string())
                    .map_err(|_| ArithError::ReadonlyVar(name.to_string()))
            }
        }
    }
}

pub fn eval(expr: &ArithExpr, shell: &mut Shell) -> Result<i64, ArithError> {
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
        ArithExpr::Index { name, subscript, subscript_raw } => {
            read_lvalue_i64(shell, &LValue::Element {
                name: name.clone(),
                subscript: subscript.clone(),
                subscript_raw: subscript_raw.clone(),
            })
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
        ArithExpr::Comma(l, r) => {
            eval(l, shell)?; // evaluate L for its side effects; discard value
            eval(r, shell)   // value of a comma sequence is the last operand
        }
        ArithExpr::BitAnd(a, b) => Ok(eval(a, shell)? & eval(b, shell)?),
        ArithExpr::BitOr(a, b)  => Ok(eval(a, shell)? | eval(b, shell)?),
        ArithExpr::BitXor(a, b) => Ok(eval(a, shell)? ^ eval(b, shell)?),
        ArithExpr::BitNot(e)    => Ok(!eval(e, shell)?),
        ArithExpr::Shl(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if !(0..64).contains(&rhs) {
                return Err(ArithError::ShiftCountOutOfRange { count: rhs });
            }
            Ok(lhs.wrapping_shl(rhs as u32))
        }
        ArithExpr::Shr(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if !(0..64).contains(&rhs) {
                return Err(ArithError::ShiftCountOutOfRange { count: rhs });
            }
            Ok(lhs.wrapping_shr(rhs as u32))
        }
        ArithExpr::Pow(a, b) => {
            let base = eval(a, shell)?;
            let exp = eval(b, shell)?;
            if exp < 0 {
                return Err(ArithError::NegativeExponent);
            }
            Ok(base.wrapping_pow(exp as u32))
        }
        ArithExpr::Assign { target, op, rhs } => {
            let rhs_val = eval(rhs, shell)?;
            let new_val = match op {
                AssignOp::Set    => rhs_val,
                AssignOp::Add    => read_lvalue_i64(shell, target)?.wrapping_add(rhs_val),
                AssignOp::Sub    => read_lvalue_i64(shell, target)?.wrapping_sub(rhs_val),
                AssignOp::Mul    => read_lvalue_i64(shell, target)?.wrapping_mul(rhs_val),
                AssignOp::Div => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if rhs_val == 0 { return Err(ArithError::DivisionByZero); }
                    lhs.wrapping_div(rhs_val)
                }
                AssignOp::Mod => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if rhs_val == 0 { return Err(ArithError::ModuloByZero); }
                    lhs.wrapping_rem(rhs_val)
                }
                AssignOp::Shl => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError::ShiftCountOutOfRange { count: rhs_val });
                    }
                    lhs.wrapping_shl(rhs_val as u32)
                }
                AssignOp::Shr => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError::ShiftCountOutOfRange { count: rhs_val });
                    }
                    lhs.wrapping_shr(rhs_val as u32)
                }
                AssignOp::BitAnd => read_lvalue_i64(shell, target)? & rhs_val,
                AssignOp::BitXor => read_lvalue_i64(shell, target)? ^ rhs_val,
                AssignOp::BitOr  => read_lvalue_i64(shell, target)? | rhs_val,
            };
            write_lvalue_i64(shell, target, new_val)?;
            Ok(new_val)
        }
        ArithExpr::PreInc(target) => {
            let new_val = read_lvalue_i64(shell, target)?.wrapping_add(1);
            write_lvalue_i64(shell, target, new_val)?;
            Ok(new_val)
        }
        ArithExpr::PreDec(target) => {
            let new_val = read_lvalue_i64(shell, target)?.wrapping_sub(1);
            write_lvalue_i64(shell, target, new_val)?;
            Ok(new_val)
        }
        ArithExpr::PostInc(target) => {
            let old_val = read_lvalue_i64(shell, target)?;
            write_lvalue_i64(shell, target, old_val.wrapping_add(1))?;
            Ok(old_val)
        }
        ArithExpr::PostDec(target) => {
            let old_val = read_lvalue_i64(shell, target)?;
            write_lvalue_i64(shell, target, old_val.wrapping_sub(1))?;
            Ok(old_val)
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
    fn parse_double_minus_with_number_is_prefix_dec_error() {
        // v38: -- is now MinusMinus (prefix decrement), which requires a
        // variable name after it. --5 → parse error.
        assert!(matches!(parse("--5"), Err(ArithError::Parse(_))));
    }

    #[test]
    fn parse_unary_minus_double_negation_uses_space() {
        // To express double negation of a literal, use a space: - -5.
        assert_eq!(parse("- -5").unwrap(), ArithExpr::Neg(Box::new(ArithExpr::Neg(n(5)))));
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

    #[test]
    fn parse_bitwise_precedence_or_below_and() {
        // 1 | 2 & 3 parses as 1 | (2 & 3) — & binds tighter than |.
        let expr = parse("1 | 2 & 3").unwrap();
        assert_eq!(expr, ArithExpr::BitOr(
            n(1),
            Box::new(ArithExpr::BitAnd(n(2), n(3))),
        ));
    }

    #[test]
    fn parse_shift_below_addition() {
        // 1 + 2 << 3 parses as (1 + 2) << 3 — << has lower precedence than +.
        let expr = parse("1 + 2 << 3").unwrap();
        assert_eq!(expr, ArithExpr::Shl(
            Box::new(ArithExpr::Add(n(1), n(2))),
            n(3),
        ));
    }

    #[test]
    fn parse_power_right_associative() {
        // 2 ** 3 ** 2 parses as Pow(2, Pow(3, 2)).
        let expr = parse("2 ** 3 ** 2").unwrap();
        assert_eq!(expr, ArithExpr::Pow(
            n(2),
            Box::new(ArithExpr::Pow(n(3), n(2))),
        ));
    }

    #[test]
    fn parse_assignment_right_associative() {
        // a = b = 5 parses as Assign(a, Set, Assign(b, Set, 5)).
        let expr = parse("a = b = 5").unwrap();
        assert_eq!(expr, ArithExpr::Assign {
            target: LValue::Scalar("a".to_string()),
            op: AssignOp::Set,
            rhs: Box::new(ArithExpr::Assign {
                target: LValue::Scalar("b".to_string()),
                op: AssignOp::Set,
                rhs: n(5),
            }),
        });
    }

    #[test]
    fn parse_assignment_lhs_must_be_var() {
        // (a + b) = 5 → parse error (LHS not a Var).
        assert!(matches!(parse("(a + b) = 5"), Err(ArithError::Parse(_))));
    }

    #[test]
    fn parse_postfix_lhs_must_be_var() {
        // (a + b)++ → parse error.
        assert!(matches!(parse("(a + b)++"), Err(ArithError::Parse(_))));
    }

    #[test]
    fn parse_compound_assignment_all_forms() {
        // All 11 compound assignment forms.
        let cases = [
            ("a = 1", AssignOp::Set),
            ("a += 1", AssignOp::Add),
            ("a -= 1", AssignOp::Sub),
            ("a *= 1", AssignOp::Mul),
            ("a /= 1", AssignOp::Div),
            ("a %= 1", AssignOp::Mod),
            ("a <<= 1", AssignOp::Shl),
            ("a >>= 1", AssignOp::Shr),
            ("a &= 1", AssignOp::BitAnd),
            ("a ^= 1", AssignOp::BitXor),
            ("a |= 1", AssignOp::BitOr),
        ];
        for (input, expected_op) in cases {
            let expr = parse(input).unwrap();
            match expr {
                ArithExpr::Assign { target, op, rhs } => {
                    assert_eq!(target, LValue::Scalar("a".to_string()), "for input {input}");
                    assert_eq!(op, expected_op, "for input {input}");
                    assert_eq!(*rhs, ArithExpr::Num(1), "for input {input}");
                }
                other => panic!("expected Assign for {input}, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_pre_post_inc_dec() {
        assert_eq!(parse("++a").unwrap(), ArithExpr::PreInc(LValue::Scalar("a".to_string())));
        assert_eq!(parse("--a").unwrap(), ArithExpr::PreDec(LValue::Scalar("a".to_string())));
        assert_eq!(parse("a++").unwrap(), ArithExpr::PostInc(LValue::Scalar("a".to_string())));
        assert_eq!(parse("a--").unwrap(), ArithExpr::PostDec(LValue::Scalar("a".to_string())));
    }

    use crate::shell_state::Shell;

    fn eval_str(s: &str, shell: &mut Shell) -> Result<i64, ArithError> {
        eval(&parse(s).unwrap(), shell)
    }

    #[test]
    fn comma_value_is_last_operand() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1, 2, 3", &mut s).unwrap(), 3);
    }

    #[test]
    fn comma_keeps_side_effects_of_all_operands() {
        let mut s = Shell::new();
        // a=1 then b=2; value is the last (2); both vars set.
        assert_eq!(eval_str("a=1, b=2", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
        assert_eq!(s.lookup_var("b").as_deref(), Some("2"));
    }

    #[test]
    fn comma_is_lower_precedence_than_assignment() {
        // `a = 1, 2` is `(a=1), 2`: value 2, a==1 (NOT a=(1,2)=2).
        let mut s = Shell::new();
        assert_eq!(eval_str("a = 1, 2", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
    }

    #[test]
    fn comma_inside_parens() {
        let mut s = Shell::new();
        assert_eq!(eval_str("(1, 2) + 3", &mut s).unwrap(), 5);
    }

    #[test]
    fn comma_side_effect_ordering() {
        // i=0 then i++ : value of i++ is 0, i becomes 1.
        let mut s = Shell::new();
        assert_eq!(eval_str("i=0, i++", &mut s).unwrap(), 0);
        assert_eq!(s.lookup_var("i").as_deref(), Some("1"));
    }

    #[test]
    fn comma_nested_left_fold() {
        let mut s = Shell::new();
        assert_eq!(eval_str("(1,2),3", &mut s).unwrap(), 3);
    }

    #[test]
    fn trailing_comma_is_error() {
        // `eval_str` unwraps `parse`, so a parse error would panic rather than
        // surface as Err — assert on `parse` directly to capture the error.
        assert!(parse("1,").is_err());
    }

    #[test]
    fn leading_comma_is_error() {
        assert!(parse(",1").is_err());
    }

    #[test]
    fn eval_number_literal() {
        let mut s = Shell::new();
        assert_eq!(eval_str("42", &mut s).unwrap(), 42);
    }

    #[test]
    fn eval_addition() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1+2", &mut s).unwrap(), 3);
    }

    #[test]
    fn eval_precedence() {
        let mut s = Shell::new();
        assert_eq!(eval_str("2+3*4", &mut s).unwrap(), 14);
        assert_eq!(eval_str("(2+3)*4", &mut s).unwrap(), 20);
    }

    #[test]
    fn eval_subtraction_left_assoc() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1-2-3", &mut s).unwrap(), -4);
    }

    #[test]
    fn eval_unary_minus() {
        let mut s = Shell::new();
        assert_eq!(eval_str("-5", &mut s).unwrap(), -5);
        // v38: "--5" now parses as prefix-decrement on a literal → error.
        // Use "- -5" (with space) for explicit double-negation.
        assert_eq!(eval_str("- -5", &mut s).unwrap(), 5);
    }

    #[test]
    fn eval_division_truncates_toward_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("7/2", &mut s).unwrap(), 3);
        assert_eq!(eval_str("-7/2", &mut s).unwrap(), -3);
    }

    #[test]
    fn eval_modulo() {
        let mut s = Shell::new();
        assert_eq!(eval_str("7%3", &mut s).unwrap(), 1);
        assert_eq!(eval_str("-7%3", &mut s).unwrap(), -1);
    }

    #[test]
    fn eval_division_by_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1/0", &mut s).unwrap_err(), ArithError::DivisionByZero);
    }

    #[test]
    fn eval_modulo_by_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1%0", &mut s).unwrap_err(), ArithError::ModuloByZero);
    }

    #[test]
    fn eval_comparison_returns_one_or_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1<2", &mut s).unwrap(), 1);
        assert_eq!(eval_str("2<1", &mut s).unwrap(), 0);
        assert_eq!(eval_str("1==1", &mut s).unwrap(), 1);
        assert_eq!(eval_str("1!=1", &mut s).unwrap(), 0);
    }

    #[test]
    fn eval_logical_not() {
        let mut s = Shell::new();
        assert_eq!(eval_str("!0", &mut s).unwrap(), 1);
        assert_eq!(eval_str("!5", &mut s).unwrap(), 0);
        assert_eq!(eval_str("!!5", &mut s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_and_short_circuits() {
        let mut s = Shell::new();
        assert_eq!(eval_str("0 && 1/0", &mut s).unwrap(), 0);
    }

    #[test]
    fn eval_logical_or_short_circuits() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1 || 1/0", &mut s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_and_returns_one_when_both_truthy() {
        let mut s = Shell::new();
        assert_eq!(eval_str("5 && 3", &mut s).unwrap(), 1);
    }

    #[test]
    fn eval_logical_or_returns_one_when_either_truthy() {
        let mut s = Shell::new();
        assert_eq!(eval_str("0 || 3", &mut s).unwrap(), 1);
        assert_eq!(eval_str("0 || 0", &mut s).unwrap(), 0);
    }

    #[test]
    fn eval_ternary() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1 ? 42 : 99", &mut s).unwrap(), 42);
        assert_eq!(eval_str("0 ? 42 : 99", &mut s).unwrap(), 99);
    }

    #[test]
    fn eval_overflow_wraps() {
        let mut s = Shell::new();
        let max = i64::MAX.to_string();
        let expr = format!("{max} + 1");
        assert_eq!(eval_str(&expr, &mut s).unwrap(), i64::MIN);
    }

    #[test]
    fn eval_unset_var_is_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("HUCK_TEST_UNSET_ARITH + 5", &mut s).unwrap(), 5);
    }

    #[test]
    fn eval_set_var_lookup() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_X", "10".to_string());
        assert_eq!(eval_str("HUCK_TEST_ARITH_X * 2", &mut s).unwrap(), 20);
    }

    #[test]
    fn eval_var_with_dollar_prefix_same_as_bare() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_Y", "7".to_string());
        assert_eq!(eval_str("$HUCK_TEST_ARITH_Y + 1", &mut s).unwrap(), 8);
    }

    #[test]
    fn eval_empty_var_is_zero() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_EMPTY", "".to_string());
        assert_eq!(eval_str("HUCK_TEST_ARITH_EMPTY + 3", &mut s).unwrap(), 3);
    }

    #[test]
    fn eval_non_integer_var_is_error() {
        let mut s = Shell::new();
        s.export_set("HUCK_TEST_ARITH_BAD", "abc".to_string());
        let err = eval_str("HUCK_TEST_ARITH_BAD + 1", &mut s).unwrap_err();
        assert_eq!(
            err,
            ArithError::NotAnInteger {
                var: "HUCK_TEST_ARITH_BAD".to_string(),
                value: "abc".to_string()
            }
        );
    }

    #[test]
    fn eval_bitwise_and() {
        let mut s = Shell::new();
        assert_eq!(eval_str("0xF0 & 0x0F", &mut s).unwrap(), 0);
        assert_eq!(eval_str("0xFF & 0x33", &mut s).unwrap(), 0x33);
    }

    #[test]
    fn eval_bitwise_or() {
        let mut s = Shell::new();
        assert_eq!(eval_str("0xF0 | 0x0F", &mut s).unwrap(), 0xFF);
    }

    #[test]
    fn eval_bitwise_xor() {
        let mut s = Shell::new();
        assert_eq!(eval_str("0xFF ^ 0x0F", &mut s).unwrap(), 0xF0);
    }

    #[test]
    fn eval_bitwise_not() {
        let mut s = Shell::new();
        assert_eq!(eval_str("~0", &mut s).unwrap(), -1);
        assert_eq!(eval_str("~(-1)", &mut s).unwrap(), 0);
    }

    #[test]
    fn eval_left_shift() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1 << 4", &mut s).unwrap(), 16);
        assert_eq!(eval_str("1 << 0", &mut s).unwrap(), 1);
    }

    #[test]
    fn eval_arithmetic_right_shift_preserves_sign() {
        let mut s = Shell::new();
        // Rust's i64 >> is arithmetic right shift; sign bit replicates.
        assert_eq!(eval_str("(-8) >> 1", &mut s).unwrap(), -4);
        assert_eq!(eval_str("16 >> 2", &mut s).unwrap(), 4);
    }

    #[test]
    fn eval_shift_negative_count_errors() {
        let mut s = Shell::new();
        assert!(matches!(
            eval_str("1 << -1", &mut s),
            Err(ArithError::ShiftCountOutOfRange { count: -1 })
        ));
    }

    #[test]
    fn eval_shift_count_64_or_more_errors() {
        let mut s = Shell::new();
        assert!(matches!(
            eval_str("1 << 64", &mut s),
            Err(ArithError::ShiftCountOutOfRange { count: 64 })
        ));
    }

    #[test]
    fn eval_pow_basic() {
        let mut s = Shell::new();
        assert_eq!(eval_str("2 ** 10", &mut s).unwrap(), 1024);
    }

    #[test]
    fn eval_pow_zero_exponent() {
        let mut s = Shell::new();
        assert_eq!(eval_str("5 ** 0", &mut s).unwrap(), 1);
        assert_eq!(eval_str("0 ** 0", &mut s).unwrap(), 1);
    }

    #[test]
    fn eval_pow_negative_exponent_errors() {
        let mut s = Shell::new();
        assert!(matches!(
            eval_str("2 ** -1", &mut s),
            Err(ArithError::NegativeExponent)
        ));
    }

    #[test]
    fn eval_assign_basic_mutates_shell() {
        let mut s = Shell::new();
        assert_eq!(eval_str("a = 5", &mut s).unwrap(), 5);
        assert_eq!(s.lookup_var("a"), Some("5".to_string()));
    }

    #[test]
    fn eval_assign_compound_add() {
        let mut s = Shell::new();
        s.set("a", "3".to_string());
        assert_eq!(eval_str("a += 4", &mut s).unwrap(), 7);
        assert_eq!(s.lookup_var("a"), Some("7".to_string()));
    }

    #[test]
    fn eval_assign_div_by_zero_errors() {
        let mut s = Shell::new();
        s.set("a", "10".to_string());
        assert!(matches!(eval_str("a /= 0", &mut s), Err(ArithError::DivisionByZero)));
    }

    #[test]
    fn eval_pre_inc_returns_new_value() {
        let mut s = Shell::new();
        s.set("a", "5".to_string());
        assert_eq!(eval_str("++a", &mut s).unwrap(), 6);
        assert_eq!(s.lookup_var("a"), Some("6".to_string()));
    }

    #[test]
    fn parse_index_read() {
        assert_eq!(parse("arr[0]").unwrap(), ArithExpr::Index {
            name: "arr".to_string(),
            subscript: n(0),
            subscript_raw: "0".to_string(),
        });
    }

    #[test]
    fn parse_index_arith_subscript_keeps_raw() {
        // The parsed subscript is an arith expr; the raw text is preserved
        // verbatim (used as an associative key).
        let expr = parse("m[1+1]").unwrap();
        match expr {
            ArithExpr::Index { name, subscript, subscript_raw } => {
                assert_eq!(name, "m");
                assert_eq!(*subscript, ArithExpr::Add(n(1), n(1)));
                assert_eq!(subscript_raw, "1+1");
            }
            other => panic!("expected Index, got {other:?}"),
        }
    }

    #[test]
    fn parse_index_assign_lvalue() {
        let expr = parse("a[2] = 9").unwrap();
        match expr {
            ArithExpr::Assign { target, op, rhs } => {
                assert_eq!(target, LValue::Element {
                    name: "a".to_string(),
                    subscript: n(2),
                    subscript_raw: "2".to_string(),
                });
                assert_eq!(op, AssignOp::Set);
                assert_eq!(*rhs, ArithExpr::Num(9));
            }
            other => panic!("expected Assign, got {other:?}"),
        }
    }

    #[test]
    fn eval_index_read_indexed_array() {
        let mut s = Shell::new();
        s.set_array_element("arr", 0, "10".to_string()).unwrap();
        s.set_array_element("arr", 1, "20".to_string()).unwrap();
        assert_eq!(eval_str("arr[0] + arr[1]", &mut s).unwrap(), 30);
    }

    #[test]
    fn eval_index_unset_element_is_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("nope[3] + 5", &mut s).unwrap(), 5);
    }

    #[test]
    fn eval_index_compound_assign_indexed() {
        let mut s = Shell::new();
        s.set_array_element("a", 0, "10".to_string()).unwrap();
        s.set_array_element("a", 1, "20".to_string()).unwrap();
        assert_eq!(eval_str("a[0] += a[1]", &mut s).unwrap(), 30);
        assert_eq!(s.lookup_array_element("a", 0), Some("30".to_string()));
    }

    #[test]
    fn eval_index_post_inc_element() {
        let mut s = Shell::new();
        s.set_array_element("a", 1, "2".to_string()).unwrap();
        assert_eq!(eval_str("a[1]++", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_array_element("a", 1), Some("3".to_string()));
    }

    #[test]
    fn eval_post_inc_returns_old_value() {
        let mut s = Shell::new();
        s.set("a", "5".to_string());
        assert_eq!(eval_str("a++", &mut s).unwrap(), 5);
        assert_eq!(s.lookup_var("a"), Some("6".to_string()));
    }
}
