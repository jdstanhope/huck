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
fn parse_hex_digits(bytes: &[u8], i: &mut usize) -> Result<i64, ArithError> {
    let mut s = String::new();
    while *i < bytes.len() && (bytes[*i] as char).is_ascii_hexdigit() {
        s.push(bytes[*i] as char);
        *i += 1;
    }
    if s.is_empty() {
        return Err(ArithError::parse("hex literal requires at least one digit"));
    }
    i64::from_str_radix(&s, 16).map_err(|_|
        ArithError::parse(format!("hex literal out of range: 0x{s}")))
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
///
/// Returns bash-mapped error kinds for positioned errors (caller adds offset):
///   digit >= base     → `ValueTooGreatForBase`
///   no digits         → `InvalidIntegerConstant`
///   overflow          → legacy `ArithError::parse(...)` (out-of-scope)
fn parse_base_n_digits(bytes: &[u8], i: &mut usize, base: u32) -> Result<i64, ArithError> {
    let mut value: i64 = 0;
    let mut any_digit = false;
    while *i < bytes.len() {
        let c = bytes[*i] as char;
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
            return Err(ArithError { kind: ArithErrorKind::ValueTooGreatForBase, offset: None });
        }
        value = value
            .checked_mul(base as i64)
            .and_then(|v| v.checked_add(digit as i64))
            .ok_or_else(|| ArithError::parse(format!(
                "base-{base} literal out of range"
            )))?;
        any_digit = true;
        *i += 1;
    }
    if !any_digit {
        return Err(ArithError { kind: ArithErrorKind::InvalidIntegerConstant, offset: None });
    }
    Ok(value)
}

pub(crate) fn tokenize(input: &str) -> Result<(Vec<ArithToken>, Vec<usize>), ArithError> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => { i += 1; }
            b'0'..=b'9' => {
                let num_start = i;
                // Read greedy leading decimal digits.
                let mut digits = String::new();
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    digits.push(bytes[i] as char);
                    i += 1;
                }
                let n: i64 = if i < bytes.len() && bytes[i] == b'#' {
                    // Base-N literal: leading digits parsed as decimal base.
                    i += 1;
                    let base: u32 = digits.parse()
                        .map_err(|_| ArithError::at(ArithErrorKind::InvalidNumber, num_start))?;
                    if base > 64 {
                        return Err(ArithError::at(ArithErrorKind::InvalidBase, num_start));
                    }
                    if base < 2 {
                        return Err(ArithError::at(ArithErrorKind::InvalidNumber, num_start));
                    }
                    parse_base_n_digits(bytes, &mut i, base).map_err(|e| match e.kind {
                        ArithErrorKind::InvalidIntegerConstant
                        | ArithErrorKind::ValueTooGreatForBase => ArithError::at(e.kind, num_start),
                        _ => e,
                    })?
                } else if digits == "0"
                    && i < bytes.len()
                    && (bytes[i] == b'x' || bytes[i] == b'X')
                {
                    // Hex literal: 0x... / 0X...
                    i += 1;
                    parse_hex_digits(bytes, &mut i)?
                } else if digits.len() > 1 && digits.starts_with('0') {
                    // Octal literal: 010 → 8. All digits must be 0-7.
                    i64::from_str_radix(&digits, 8)
                        .map_err(|_| ArithError::parse(format!(
                            "invalid octal literal: {digits}"
                        )))?
                } else {
                    digits.parse()
                        .map_err(|_| ArithError::parse(format!(
                            "integer literal out of range: {digits}"
                        )))?
                };
                offsets.push(num_start);
                out.push(ArithToken::Number(n));
            }
            // Unreachable post-v93 for the three arith *contexts* (`(( ))`,
            // `$(( ))`, arith-`for`): those expand `$`-forms before calling
            // `arith::parse`. Kept defensive for any other `arith::parse`
            // caller (e.g. integer-coerce on a value still bearing a `$`).
            b'$' => {
                let start = i;
                i += 1;
                let mut s = String::new();
                while i < bytes.len()
                    && (bytes[i] == b'_' || (bytes[i] as char).is_ascii_alphanumeric())
                {
                    s.push(bytes[i] as char);
                    i += 1;
                }
                if s.is_empty() {
                    return Err(ArithError::parse("expected identifier after '$'"));
                }
                offsets.push(start);
                out.push(ArithToken::Ident(s));
            }
            b if b == b'_' || (b as char).is_ascii_alphabetic() => {
                let start = i;
                let mut s = String::new();
                while i < bytes.len()
                    && (bytes[i] == b'_' || (bytes[i] as char).is_ascii_alphanumeric())
                {
                    s.push(bytes[i] as char);
                    i += 1;
                }
                offsets.push(start);
                out.push(ArithToken::Ident(s));
            }
            b'(' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::LParen);
            }
            b')' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::RParen);
            }
            b'[' => {
                // Capture the RAW inner text up to the matching ']' (for the
                // associative-key case), tracking bracket nesting so that
                // e.g. `a[b[0]]` captures `b[0]`. The inner tokens are still
                // produced by the main loop after this LBracket.
                let start = i;
                i += 1;
                let mut raw = String::new();
                let mut depth = 1i32;
                let mut j = i;
                while j < bytes.len() {
                    match bytes[j] {
                        b'[' => depth += 1,
                        b']' => { depth -= 1; if depth == 0 { break; } }
                        _ => {}
                    }
                    raw.push(bytes[j] as char);
                    j += 1;
                }
                offsets.push(start);
                out.push(ArithToken::LBracket(raw));
            }
            b']' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::RBracket);
            }
            b',' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::Comma);
            }
            b'+' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'+' {
                    i += 1; offsets.push(start); out.push(ArithToken::PlusPlus);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::PlusEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Plus);
                }
            }
            b'-' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'-' {
                    i += 1; offsets.push(start); out.push(ArithToken::MinusMinus);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::MinusEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Minus);
                }
            }
            b'*' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'*' {
                    i += 1; offsets.push(start); out.push(ArithToken::Power);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::StarEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Star);
                }
            }
            b'/' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::SlashEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Slash);
                }
            }
            b'%' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::PercentEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Percent);
                }
            }
            b'?' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::Question);
            }
            b':' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::Colon);
            }
            b'!' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::Ne);
                } else {
                    offsets.push(start); out.push(ArithToken::Bang);
                }
            }
            b'=' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::Eq);
                } else {
                    offsets.push(start); out.push(ArithToken::Assign);
                }
            }
            b'<' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'<' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        i += 1; offsets.push(start); out.push(ArithToken::ShlEq);
                    } else {
                        offsets.push(start); out.push(ArithToken::Shl);
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::Le);
                } else {
                    offsets.push(start); out.push(ArithToken::Lt);
                }
            }
            b'>' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'>' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        i += 1; offsets.push(start); out.push(ArithToken::ShrEq);
                    } else {
                        offsets.push(start); out.push(ArithToken::Shr);
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::Ge);
                } else {
                    offsets.push(start); out.push(ArithToken::Gt);
                }
            }
            b'&' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'&' {
                    i += 1; offsets.push(start); out.push(ArithToken::AndAnd);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::AmpEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Amp);
                }
            }
            b'|' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'|' {
                    i += 1; offsets.push(start); out.push(ArithToken::OrOr);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::PipeEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Pipe);
                }
            }
            b'^' => {
                let start = i; i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1; offsets.push(start); out.push(ArithToken::CaretEq);
                } else {
                    offsets.push(start); out.push(ArithToken::Caret);
                }
            }
            b'~' => {
                let start = i; i += 1;
                offsets.push(start); out.push(ArithToken::Tilde);
            }
            other => {
                return Err(ArithError::parse(format!("unexpected character: {:?}", other as char)));
            }
        }
    }
    Ok((out, offsets))
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
pub enum ArithErrorKind {
    // Legacy free-form parse message (kept for sites not yet bash-mapped).
    Parse(String),
    // Bash-mapped parse/lex errors (each renders a fixed bash string):
    AssignToNonVar,          // "attempted assignment to non-variable"
    InvalidBase,             // "invalid arithmetic base"
    InvalidIntegerConstant,  // "invalid integer constant"
    ValueTooGreatForBase,    // "value too great for base"
    InvalidNumber,           // "invalid number"
    MissingCloseParen,       // "missing `)'"
    OperandExpected,         // "syntax error: operand expected"
    ExpressionExpected,      // "expression expected"
    ColonExpected,           // "`:' expected for conditional expression"
    SyntaxErrorInExpression, // "syntax error in expression"
    BadArraySubscript,       // "bad array subscript"
    // Eval-time:
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
    ReadonlyVar(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArithError {
    pub kind: ArithErrorKind,
    /// Byte offset into the parsed source of the error token (bash `lasttp`).
    pub offset: Option<usize>,
}

impl ArithError {
    /// Legacy free-form parse error, no position.
    pub fn parse(msg: impl Into<String>) -> Self {
        ArithError { kind: ArithErrorKind::Parse(msg.into()), offset: None }
    }
    /// A bash-mapped error with a token offset.
    pub fn at(kind: ArithErrorKind, offset: usize) -> Self {
        ArithError { kind, offset: Some(offset) }
    }
    /// The text bash prints after the `<expr>: ` echo.
    pub fn bash_message(&self) -> String {
        use ArithErrorKind::*;
        match &self.kind {
            Parse(m) => m.clone(),
            AssignToNonVar => "attempted assignment to non-variable".into(),
            InvalidBase => "invalid arithmetic base".into(),
            InvalidIntegerConstant => "invalid integer constant".into(),
            ValueTooGreatForBase => "value too great for base".into(),
            InvalidNumber => "invalid number".into(),
            MissingCloseParen => "missing `)'".into(),
            OperandExpected => "syntax error: operand expected".into(),
            ExpressionExpected => "expression expected".into(),
            ColonExpected => "`:' expected for conditional expression".into(),
            SyntaxErrorInExpression => "syntax error in expression".into(),
            BadArraySubscript => "bad array subscript".into(),
            DivisionByZero | ModuloByZero => "division by 0".into(),
            NotAnInteger { value, .. } => format!("{value}: syntax error: operand expected"),
            NegativeExponent => "exponent less than 0".into(),
            ShiftCountOutOfRange { .. } => "shift count out of range".into(),
            ReadonlyVar(name) => format!("{name}: readonly variable"),
        }
    }
}

impl std::fmt::Display for ArithError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Legacy rendering kept so existing `{e}` callers compile unchanged
        // until Task 5 swaps the emission sites to bash formatting.
        use ArithErrorKind::*;
        match &self.kind {
            Parse(m) => write!(f, "{m}"),
            DivisionByZero => write!(f, "division by zero"),
            ModuloByZero => write!(f, "modulo by zero"),
            NotAnInteger { var, value } =>
                write!(f, "variable '{var}' is not an integer: '{value}'"),
            NegativeExponent => write!(f, "exponentiation with negative exponent"),
            ShiftCountOutOfRange { count } => write!(f, "shift count out of range: {count}"),
            ReadonlyVar(name) => write!(f, "{name}: readonly variable"),
            // Bash-mapped kinds render their bash text under Display too.
            other => write!(f, "{}", ArithError { kind: other.clone(), offset: None }.bash_message()),
        }
    }
}

pub fn parse(input: &str) -> Result<ArithExpr, ArithError> {
    let (tokens, offsets) = tokenize(input)?;
    let mut p = Parser { tokens, offsets, pos: 0, err_off: 0 };
    let expr = p.parse_comma_expr()?;
    if p.pos < p.tokens.len() {
        let off = p.offsets[p.pos];
        return Err(ArithError::at(ArithErrorKind::SyntaxErrorInExpression, off));
    }
    Ok(expr)
}

/// Builds bash's post-prologue error body:
/// `"<expr>: <msg> (error token is \"<tok>\")"`, where `<expr>` is `src`
/// with leading whitespace trimmed and `<tok>` is `src[offset..]`.
pub fn render_error_body(src: &str, err: &ArithError) -> String {
    let expr = src.trim_start();
    let tok = match err.offset {
        Some(off) if off <= src.len() => &src[off..],
        _ => "",
    };
    format!("{expr}: {} (error token is \"{tok}\")", err.bash_message())
}

struct Parser {
    tokens: Vec<ArithToken>,
    offsets: Vec<usize>,
    pos: usize,
    err_off: usize,
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
        if t.is_some() {
            self.err_off = self.offsets[self.pos];
        }
        self.pos += 1;
        t
    }

    fn fail(&self, kind: ArithErrorKind) -> ArithError {
        ArithError::at(kind, self.err_off)
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
                    None => return Err(ArithError::parse(
                        "postfix ++/-- requires variable on LHS"
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
                    None => return Err(self.fail(ArithErrorKind::AssignToNonVar)),
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
                    _ => return Err(self.fail(ArithErrorKind::ColonExpected)),
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
            other => return Err(ArithError::parse(format!(
                "expected '[', got {other:?}"
            ))),
        };
        let subscript = self.parse_comma_expr()?;
        match self.bump() {
            Some(ArithToken::RBracket) => {}
            other => return Err(ArithError::parse(format!(
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
                    None => Err(ArithError::parse(
                        "prefix ++ requires variable"
                    )),
                }
            }
            Some(ArithToken::MinusMinus) => {
                // --name / --name[sub]: prefix decrement requires an lvalue.
                let inner = self.parse_prefix()?;
                match expr_to_lvalue(inner) {
                    Some(target) => Ok(ArithExpr::PreDec(target)),
                    None => Err(ArithError::parse(
                        "prefix -- requires variable"
                    )),
                }
            }
            Some(ArithToken::LParen) => {
                let inner = self.parse_comma_expr()?;
                match self.bump() {
                    Some(ArithToken::RParen) => Ok(inner),
                    _ => Err(self.fail(ArithErrorKind::MissingCloseParen)),
                }
            }
            Some(t) => {
                let kind = if matches!(t, ArithToken::Colon) {
                    ArithErrorKind::ExpressionExpected
                } else {
                    ArithErrorKind::OperandExpected
                };
                Err(self.fail(kind))
            }
            None => Err(self.fail(ArithErrorKind::OperandExpected)),
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
    raw.parse::<i64>().map_err(|_| ArithError { kind: ArithErrorKind::NotAnInteger {
        var: name.to_string(),
        value: raw,
    }, offset: None })
}

/// Writes an i64 back to a shell variable as a decimal string.
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) -> Result<(), ArithError> {
    shell.try_set(name, value.to_string())
        .map_err(|_| ArithError { kind: ArithErrorKind::ReadonlyVar(name.to_string()), offset: None })
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
    let parsed = parse(&raw).map_err(|_| ArithError { kind: ArithErrorKind::NotAnInteger {
        var: name.to_string(),
        value: raw.clone(),
    }, offset: None })?;
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
                let raw = shell.lookup_indexed_element(name, idx as usize);
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
                    .map_err(|_| ArithError { kind: ArithErrorKind::ReadonlyVar(name.to_string()), offset: None })
            } else {
                let idx = eval(subscript, shell)?;
                shell
                    .set_indexed_element(name, idx as usize, value.to_string())
                    .map_err(|_| ArithError { kind: ArithErrorKind::ReadonlyVar(name.to_string()), offset: None })
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
                raw.parse::<i64>().map_err(|_| ArithError { kind: ArithErrorKind::NotAnInteger {
                    var: name.clone(),
                    value: raw.to_string(),
                }, offset: None })
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
            if rhs == 0 { return Err(ArithError { kind: ArithErrorKind::DivisionByZero, offset: None }); }
            Ok(lhs.wrapping_div(rhs))
        }
        ArithExpr::Mod(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if rhs == 0 { return Err(ArithError { kind: ArithErrorKind::ModuloByZero, offset: None }); }
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
                return Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: rhs }, offset: None });
            }
            Ok(lhs.wrapping_shl(rhs as u32))
        }
        ArithExpr::Shr(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if !(0..64).contains(&rhs) {
                return Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: rhs }, offset: None });
            }
            Ok(lhs.wrapping_shr(rhs as u32))
        }
        ArithExpr::Pow(a, b) => {
            let base = eval(a, shell)?;
            let exp = eval(b, shell)?;
            if exp < 0 {
                return Err(ArithError { kind: ArithErrorKind::NegativeExponent, offset: None });
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
                    if rhs_val == 0 { return Err(ArithError { kind: ArithErrorKind::DivisionByZero, offset: None }); }
                    lhs.wrapping_div(rhs_val)
                }
                AssignOp::Mod => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if rhs_val == 0 { return Err(ArithError { kind: ArithErrorKind::ModuloByZero, offset: None }); }
                    lhs.wrapping_rem(rhs_val)
                }
                AssignOp::Shl => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: rhs_val }, offset: None });
                    }
                    lhs.wrapping_shl(rhs_val as u32)
                }
                AssignOp::Shr => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: rhs_val }, offset: None });
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
        assert_eq!(ArithError { kind: ArithErrorKind::DivisionByZero, offset: None }.to_string(), "division by zero");
    }

    #[test]
    fn display_modulo_by_zero() {
        assert_eq!(ArithError { kind: ArithErrorKind::ModuloByZero, offset: None }.to_string(), "modulo by zero");
    }

    #[test]
    fn display_parse_error_is_bare_message() {
        let e = ArithError::parse("unexpected end of input");
        assert_eq!(e.to_string(), "unexpected end of input");
    }

    #[test]
    fn display_not_an_integer_quotes_var_and_value() {
        let e = ArithError { kind: ArithErrorKind::NotAnInteger {
            var: "x".to_string(),
            value: "abc".to_string(),
        }, offset: None };
        assert_eq!(e.to_string(), "variable 'x' is not an integer: 'abc'");
    }

    #[test]
    fn tokenize_single_number() {
        let (toks, _) = tokenize("42").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(42)]);
    }

    #[test]
    fn tokenize_zero() {
        let (toks, _) = tokenize("0").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(0)]);
    }

    #[test]
    fn tokenize_large_number() {
        let (toks, _) = tokenize("9223372036854775807").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(i64::MAX)]);
    }

    #[test]
    fn tokenize_number_overflow_is_parse_error() {
        let err = tokenize("99999999999999999999").unwrap_err();
        assert!(matches!(err.kind, ArithErrorKind::Parse(_)), "got {:?}", err);
    }

    #[test]
    fn tokenize_identifier() {
        let (toks, _) = tokenize("foo").unwrap();
        assert_eq!(toks, vec![ArithToken::Ident("foo".to_string())]);
    }

    #[test]
    fn tokenize_identifier_with_dollar_prefix_strips_dollar() {
        let (toks, _) = tokenize("$foo").unwrap();
        assert_eq!(toks, vec![ArithToken::Ident("foo".to_string())]);
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
        let (toks, _) = tokenize(input).unwrap();
        assert_eq!(toks, expected);
    }

    #[test]
    fn tokenize_multi_char_operators() {
        let input = "== != <= >= && || < >";
        let expected = vec![
            ArithToken::Eq, ArithToken::Ne, ArithToken::Le, ArithToken::Ge,
            ArithToken::AndAnd, ArithToken::OrOr,
            ArithToken::Lt, ArithToken::Gt,
        ];
        let (toks, _) = tokenize(input).unwrap();
        assert_eq!(toks, expected);
    }

    #[test]
    fn tokenize_skips_whitespace() {
        let (toks, _) = tokenize("  1   +   2  ").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(1), ArithToken::Plus, ArithToken::Number(2)]);
    }

    #[test]
    fn tokenize_unknown_char_is_parse_error() {
        let err = tokenize("1 @ 2").unwrap_err();
        assert!(matches!(err.kind, ArithErrorKind::Parse(_)));
    }

    #[test]
    fn tokenize_single_amp_is_bitwise_and() {
        // v38: bare & is now bitwise AND (was: parse error).
        let (toks, _) = tokenize("1 & 2").unwrap();
        assert_eq!(toks, vec![
            ArithToken::Number(1), ArithToken::Amp, ArithToken::Number(2),
        ]);
    }

    #[test]
    fn tokenize_single_pipe_is_bitwise_or() {
        // v38: bare | is now bitwise OR (was: parse error).
        let (toks, _) = tokenize("1 | 2").unwrap();
        assert_eq!(toks, vec![
            ArithToken::Number(1), ArithToken::Pipe, ArithToken::Number(2),
        ]);
    }

    #[test]
    fn tokenize_hex_literal() {
        let (toks, _) = tokenize("0x10").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(16)]);
    }

    #[test]
    fn tokenize_hex_literal_uppercase() {
        let (toks, _) = tokenize("0X1F").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(31)]);
    }

    #[test]
    fn tokenize_octal_literal() {
        let (toks, _) = tokenize("010").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(8)]);
    }

    #[test]
    fn tokenize_octal_invalid_digit_errors() {
        // 08 has a digit (8) that's invalid for octal.
        assert!(tokenize("08").is_err());
    }

    #[test]
    fn tokenize_base_n_binary() {
        let (toks, _) = tokenize("2#1010").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(10)]);
    }

    #[test]
    fn tokenize_base_n_hex_via_pound() {
        let (toks, _) = tokenize("16#FF").unwrap();
        assert_eq!(toks, vec![ArithToken::Number(255)]);
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
        let (toks, _) = tokenize("&|^~<<>>").unwrap();
        assert_eq!(toks, vec![
            ArithToken::Amp, ArithToken::Pipe,
            ArithToken::Caret, ArithToken::Tilde,
            ArithToken::Shl, ArithToken::Shr,
        ]);
    }

    #[test]
    fn tokenize_power_operator() {
        let (toks, _) = tokenize("2**3").unwrap();
        assert_eq!(toks, vec![
            ArithToken::Number(2), ArithToken::Power, ArithToken::Number(3),
        ]);
    }

    #[test]
    fn tokenize_compound_assignments() {
        // = += -= *= /= %= <<= >>= &= ^= |=
        let input = "= += -= *= /= %= <<= >>= &= ^= |=";
        let (tokens, _) = tokenize(input).unwrap();
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
        let (toks, _) = tokenize("++ --").unwrap();
        assert_eq!(toks, vec![ArithToken::PlusPlus, ArithToken::MinusMinus]);
    }

    #[test]
    fn tokenize_distinguishes_eq_from_assign() {
        let (toks, _) = tokenize("==").unwrap();
        assert_eq!(toks, vec![ArithToken::Eq]);
        let (toks, _) = tokenize("=").unwrap();
        assert_eq!(toks, vec![ArithToken::Assign]);
    }

    #[test]
    fn tokenize_distinguishes_lt_from_shl() {
        let (toks, _) = tokenize("<").unwrap();
        assert_eq!(toks, vec![ArithToken::Lt]);
        let (toks, _) = tokenize("<<").unwrap();
        assert_eq!(toks, vec![ArithToken::Shl]);
        let (toks, _) = tokenize("<<=").unwrap();
        assert_eq!(toks, vec![ArithToken::ShlEq]);
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
        assert!(matches!(parse("--5"), Err(ref e) if matches!(e.kind, ArithErrorKind::Parse(_))));
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
        // Empty input → OperandExpected (bump returns None, err_off stays 0).
        assert!(matches!(parse("").unwrap_err().kind, ArithErrorKind::OperandExpected));
    }

    #[test]
    fn parse_trailing_junk_is_error() {
        // Trailing junk → SyntaxErrorInExpression at the junk token's offset.
        assert!(matches!(parse("1+2 3").unwrap_err().kind, ArithErrorKind::SyntaxErrorInExpression));
    }

    #[test]
    fn parse_unbalanced_paren_is_error() {
        // Missing ')' → MissingCloseParen.
        assert!(matches!(parse("(1+2").unwrap_err().kind, ArithErrorKind::MissingCloseParen));
    }

    #[test]
    fn parse_missing_rhs_is_error() {
        // Missing RHS operand → OperandExpected.
        assert!(matches!(parse("1+").unwrap_err().kind, ArithErrorKind::OperandExpected));
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
        // (a + b) = 5 → AssignToNonVar (LHS not a scalar var or array element).
        assert!(matches!(parse("(a + b) = 5"), Err(ref e) if matches!(e.kind, ArithErrorKind::AssignToNonVar)));
    }

    #[test]
    fn parse_postfix_lhs_must_be_var() {
        // (a + b)++ → parse error.
        assert!(matches!(parse("(a + b)++"), Err(ref e) if matches!(e.kind, ArithErrorKind::Parse(_))));
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
        assert_eq!(eval_str("1/0", &mut s).unwrap_err(), ArithError { kind: ArithErrorKind::DivisionByZero, offset: None });
    }

    #[test]
    fn eval_modulo_by_zero() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1%0", &mut s).unwrap_err(), ArithError { kind: ArithErrorKind::ModuloByZero, offset: None });
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
            ArithError { kind: ArithErrorKind::NotAnInteger {
                var: "HUCK_TEST_ARITH_BAD".to_string(),
                value: "abc".to_string()
            }, offset: None }
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
            Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: -1 }, .. })
        ));
    }

    #[test]
    fn eval_shift_count_64_or_more_errors() {
        let mut s = Shell::new();
        assert!(matches!(
            eval_str("1 << 64", &mut s),
            Err(ArithError { kind: ArithErrorKind::ShiftCountOutOfRange { count: 64 }, .. })
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
            Err(ArithError { kind: ArithErrorKind::NegativeExponent, .. })
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
        assert!(matches!(eval_str("a /= 0", &mut s), Err(ArithError { kind: ArithErrorKind::DivisionByZero, .. })));
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
        s.set_indexed_element("arr", 0, "10".to_string()).unwrap();
        s.set_indexed_element("arr", 1, "20".to_string()).unwrap();
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
        s.set_indexed_element("a", 0, "10".to_string()).unwrap();
        s.set_indexed_element("a", 1, "20".to_string()).unwrap();
        assert_eq!(eval_str("a[0] += a[1]", &mut s).unwrap(), 30);
        assert_eq!(s.lookup_indexed_element("a", 0), Some("30".to_string()));
    }

    #[test]
    fn eval_index_post_inc_element() {
        let mut s = Shell::new();
        s.set_indexed_element("a", 1, "2".to_string()).unwrap();
        assert_eq!(eval_str("a[1]++", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_indexed_element("a", 1), Some("3".to_string()));
    }

    #[test]
    fn eval_post_inc_returns_old_value() {
        let mut s = Shell::new();
        s.set("a", "5".to_string());
        assert_eq!(eval_str("a++", &mut s).unwrap(), 5);
        assert_eq!(s.lookup_var("a"), Some("6".to_string()));
    }

    #[test]
    fn arith_error_bash_message_mapping() {
        use ArithErrorKind::*;
        let mk = |k| ArithError { kind: k, offset: None };
        assert_eq!(mk(AssignToNonVar).bash_message(), "attempted assignment to non-variable");
        assert_eq!(mk(DivisionByZero).bash_message(), "division by 0");
        assert_eq!(mk(InvalidBase).bash_message(), "invalid arithmetic base");
        assert_eq!(mk(InvalidIntegerConstant).bash_message(), "invalid integer constant");
        assert_eq!(mk(ValueTooGreatForBase).bash_message(), "value too great for base");
        assert_eq!(mk(MissingCloseParen).bash_message(), "missing `)'");
        assert_eq!(mk(OperandExpected).bash_message(), "syntax error: operand expected");
        assert_eq!(mk(ExpressionExpected).bash_message(), "expression expected");
        assert_eq!(mk(ColonExpected).bash_message(), "`:' expected for conditional expression");
        assert_eq!(mk(SyntaxErrorInExpression).bash_message(), "syntax error in expression");
        assert_eq!(mk(InvalidNumber).bash_message(), "invalid number");
    }

    // ── Task 3: token offsets + render_error_body ──────────────────────────

    #[test]
    fn tokenize_reports_offsets() {
        let (toks, offs) = tokenize("7 = 43 ").unwrap();
        assert_eq!(toks.len(), offs.len());
        // tokens: 7@0, =@2, 43@4
        assert_eq!(offs, vec![0, 2, 4]);
    }

    #[test]
    fn render_assign_to_nonvar() {
        // `$(( 7 = 43 ))` inner text, untrimmed
        let err = parse(" 7 = 43 ").unwrap_err();
        assert_eq!(err.bash_message(), "attempted assignment to non-variable");
        assert_eq!(render_error_body(" 7 = 43 ", &err),
            "7 = 43 : attempted assignment to non-variable (error token is \"= 43 \")");
    }

    #[test]
    fn render_operand_expected_at_eof() {
        let err = parse(" 4 + ").unwrap_err();
        assert_eq!(render_error_body(" 4 + ", &err),
            "4 + : syntax error: operand expected (error token is \"+ \")");
    }

    #[test]
    fn render_missing_close_paren() {
        let err = parse("rv = 7 + (43 * 6").unwrap_err();
        assert_eq!(render_error_body("rv = 7 + (43 * 6", &err),
            "rv = 7 + (43 * 6: missing `)' (error token is \"6\")");
    }

    #[test]
    fn render_trailing_junk() {
        let err = parse("a b").unwrap_err();
        assert_eq!(render_error_body("a b", &err),
            "a b: syntax error in expression (error token is \"b\")");
    }

    #[test]
    fn render_invalid_base_and_constants() {
        assert_eq!(render_error_body("3425#56", &parse("3425#56").unwrap_err()),
            "3425#56: invalid arithmetic base (error token is \"3425#56\")");
        assert_eq!(render_error_body("2#", &parse("2#").unwrap_err()),
            "2#: invalid integer constant (error token is \"2#\")");
        assert_eq!(render_error_body("2#44", &parse("2#44").unwrap_err()),
            "2#44: value too great for base (error token is \"2#44\")");
    }

    #[test]
    fn render_ternary_branches() {
        assert_eq!(render_error_body("4 ? : 3 + 5", &parse("4 ? : 3 + 5").unwrap_err()),
            "4 ? : 3 + 5: expression expected (error token is \": 3 + 5\")");
    }
}
