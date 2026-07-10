//! Arithmetic expansion: AST, parser, and evaluator for `$((expr))`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArithToken {
    Number(i64),
    Ident(String),
    LParen,
    RParen,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    Question,
    Colon,
    Comma,
    // v38 — bitwise & shift:
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    // v38 — power:
    Power,
    // v38 — assignment:
    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    ShlEq,
    ShrEq,
    AmpEq,
    CaretEq,
    PipeEq,
    // v38 — inc/dec:
    PlusPlus,
    MinusMinus,
    // array subscripts: `name[subscript]`. LBracket carries the RAW inner
    // source text between the brackets (used as a literal key for
    // associative arrays); the bracketed tokens are still emitted between
    // LBracket/RBracket so the subscript can be parsed as an arith
    // expression for indexed arrays.
    LBracket(String),
    RBracket,
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
    i64::from_str_radix(&s, 16)
        .map_err(|_| ArithError::parse(format!("hex literal out of range: 0x{s}")))
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
            return Err(ArithError::plain(ArithErrorKind::ValueTooGreatForBase));
        }
        value = value
            .checked_mul(base as i64)
            .and_then(|v| v.checked_add(digit as i64))
            .ok_or_else(|| ArithError::parse(format!("base-{base} literal out of range")))?;
        any_digit = true;
        *i += 1;
    }
    if !any_digit {
        return Err(ArithError::plain(ArithErrorKind::InvalidIntegerConstant));
    }
    Ok(value)
}

/// Returns the exclusive end of the maximal number-token run starting at
/// `start`: the run of bytes in `[A-Za-z0-9#@_]`. bash consumes the whole run
/// before validating, so the error token covers the full run even when the
/// fault (e.g. base > 64) is detected earlier.
fn number_run_end(bytes: &[u8], start: usize) -> usize {
    let mut e = start;
    while e < bytes.len() {
        let b = bytes[e];
        let is_run = b.is_ascii_alphanumeric() || b == b'#' || b == b'@' || b == b'_';
        if !is_run {
            break;
        }
        e += 1;
    }
    e
}

/// Consumes the longest run of identifier characters (`[_A-Za-z0-9]`) from
/// `bytes` starting at `*i`, appending each to `s` and advancing `*i`.
fn read_ident_chars(bytes: &[u8], i: &mut usize, s: &mut String) {
    while *i < bytes.len() && (bytes[*i] == b'_' || (bytes[*i] as char).is_ascii_alphanumeric()) {
        s.push(bytes[*i] as char);
        *i += 1;
    }
}

pub(crate) fn tokenize(input: &str) -> Result<(Vec<ArithToken>, Vec<usize>), ArithError> {
    let mut out = Vec::new();
    let mut offsets = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
            }
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
                    // bash reports the FULL number run as the error token
                    // (`[A-Za-z0-9#@_]*` from num_start), regardless of where in
                    // the run the error was detected.
                    i += 1;
                    let base: u32 = digits.parse().map_err(|_| {
                        ArithError::at_span(
                            ArithErrorKind::InvalidNumber,
                            num_start,
                            number_run_end(bytes, num_start),
                        )
                    })?;
                    if base > 64 {
                        return Err(ArithError::at_span(
                            ArithErrorKind::InvalidBase,
                            num_start,
                            number_run_end(bytes, num_start),
                        ));
                    }
                    if base < 2 {
                        return Err(ArithError::at_span(
                            ArithErrorKind::InvalidNumber,
                            num_start,
                            number_run_end(bytes, num_start),
                        ));
                    }
                    parse_base_n_digits(bytes, &mut i, base).map_err(|e| match e.kind {
                        ArithErrorKind::InvalidIntegerConstant
                        | ArithErrorKind::ValueTooGreatForBase => {
                            ArithError::at_span(e.kind, num_start, number_run_end(bytes, num_start))
                        }
                        _ => e,
                    })?
                } else if digits == "0" && i < bytes.len() && (bytes[i] == b'x' || bytes[i] == b'X')
                {
                    // Hex literal: 0x... / 0X...
                    i += 1;
                    parse_hex_digits(bytes, &mut i)?
                } else if digits.len() > 1 && digits.starts_with('0') {
                    // Octal literal: 010 → 8. All digits must be 0-7.
                    i64::from_str_radix(&digits, 8).map_err(|_| {
                        ArithError::parse(format!("invalid octal literal: {digits}"))
                    })?
                } else {
                    digits.parse().map_err(|_| {
                        ArithError::parse(format!("integer literal out of range: {digits}"))
                    })?
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
                read_ident_chars(bytes, &mut i, &mut s);
                if s.is_empty() {
                    return Err(ArithError::parse("expected identifier after '$'"));
                }
                offsets.push(start);
                out.push(ArithToken::Ident(s));
            }
            b if b == b'_' || (b as char).is_ascii_alphabetic() => {
                let start = i;
                let mut s = String::new();
                read_ident_chars(bytes, &mut i, &mut s);
                offsets.push(start);
                out.push(ArithToken::Ident(s));
            }
            b'(' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::LParen);
            }
            b')' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::RParen);
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
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    raw.push(bytes[j] as char);
                    j += 1;
                }
                offsets.push(start);
                out.push(ArithToken::LBracket(raw));
            }
            b']' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::RBracket);
            }
            b',' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::Comma);
            }
            b'+' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'+' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::PlusPlus);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::PlusEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Plus);
                }
            }
            b'-' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'-' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::MinusMinus);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::MinusEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Minus);
                }
            }
            b'*' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'*' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::Power);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::StarEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Star);
                }
            }
            b'/' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::SlashEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Slash);
                }
            }
            b'%' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::PercentEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Percent);
                }
            }
            b'?' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::Question);
            }
            b':' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::Colon);
            }
            b'!' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::Ne);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Bang);
                }
            }
            b'=' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::Eq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Assign);
                }
            }
            b'<' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'<' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        i += 1;
                        offsets.push(start);
                        out.push(ArithToken::ShlEq);
                    } else {
                        offsets.push(start);
                        out.push(ArithToken::Shl);
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::Le);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Lt);
                }
            }
            b'>' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'>' {
                    i += 1;
                    if i < bytes.len() && bytes[i] == b'=' {
                        i += 1;
                        offsets.push(start);
                        out.push(ArithToken::ShrEq);
                    } else {
                        offsets.push(start);
                        out.push(ArithToken::Shr);
                    }
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::Ge);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Gt);
                }
            }
            b'&' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'&' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::AndAnd);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::AmpEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Amp);
                }
            }
            b'|' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'|' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::OrOr);
                } else if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::PipeEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Pipe);
                }
            }
            b'^' => {
                let start = i;
                i += 1;
                if i < bytes.len() && bytes[i] == b'=' {
                    i += 1;
                    offsets.push(start);
                    out.push(ArithToken::CaretEq);
                } else {
                    offsets.push(start);
                    out.push(ArithToken::Caret);
                }
            }
            b'~' => {
                let start = i;
                i += 1;
                offsets.push(start);
                out.push(ArithToken::Tilde);
            }
            other => {
                return Err(ArithError::parse(format!(
                    "unexpected character: {:?}",
                    other as char
                )));
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
    Element {
        name: String,
        subscript: Box<ArithExpr>,
        subscript_raw: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithExpr {
    Num(i64),
    Var(String),
    /// `name[subscript]` array-element READ. See `LValue::Element` for the
    /// indexed-vs-associative interpretation of `subscript`/`subscript_raw`.
    Index {
        name: String,
        subscript: Box<ArithExpr>,
        subscript_raw: String,
    },
    Neg(Box<ArithExpr>),
    Not(Box<ArithExpr>),
    Add(Box<ArithExpr>, Box<ArithExpr>),
    Sub(Box<ArithExpr>, Box<ArithExpr>),
    Mul(Box<ArithExpr>, Box<ArithExpr>),
    Div(Box<ArithExpr>, Box<ArithExpr>, usize), // usize = divisor token offset
    Mod(Box<ArithExpr>, Box<ArithExpr>, usize),
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
    Assign {
        target: LValue,
        op: AssignOp,
        rhs: Box<ArithExpr>,
    },
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
    // Reserved bash-compat error kind: the "bad array subscript" string is
    // wired into the Display impl but no code path constructs this variant yet.
    // (Surfaced once `arith` became `pub(crate)` — `pub mod` had exempted it
    // from dead-code analysis. Kept for the pending arith-subscript work.)
    #[allow(dead_code)]
    BadArraySubscript, // "bad array subscript"
    // Eval-time:
    DivisionByZero,
    ModuloByZero,
    NotAnInteger {
        var: String,
        value: String,
    },
    NegativeExponent,
    ShiftCountOutOfRange {
        count: i64,
    },
    ReadonlyVar(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArithError {
    pub kind: ArithErrorKind,
    /// Byte offset into the parsed source of the error token (bash `lasttp`).
    pub offset: Option<usize>,
    /// Exclusive byte end of the error token. When `Some(e)`, the error token
    /// is `src[offset..e]` (and bash's echo is truncated to `src[..e]`) — used
    /// for tokenize-time NUMBER errors, where bash reports just the full number
    /// run with no trailing content. When `None`, the token runs to end of
    /// source (parse errors, eval errors).
    pub token_end: Option<usize>,
}

impl ArithError {
    /// Legacy free-form parse error, no position.
    pub fn parse(msg: impl Into<String>) -> Self {
        ArithError {
            kind: ArithErrorKind::Parse(msg.into()),
            offset: None,
            token_end: None,
        }
    }
    /// A bash-mapped error with a token offset (token runs to end of source).
    pub fn at(kind: ArithErrorKind, offset: usize) -> Self {
        ArithError {
            kind,
            offset: Some(offset),
            token_end: None,
        }
    }
    /// A bash-mapped error with an explicit token span `[start, end)`. Used by
    /// tokenize-time NUMBER errors, where bash's error token is the full number
    /// run and the echo is truncated to `src[..end]`.
    pub fn at_span(kind: ArithErrorKind, start: usize, end: usize) -> Self {
        ArithError {
            kind,
            offset: Some(start),
            token_end: Some(end),
        }
    }
    /// A plain error with no position info (offset and token_end are None).
    pub fn plain(kind: ArithErrorKind) -> Self {
        ArithError {
            kind,
            offset: None,
            token_end: None,
        }
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
            NotAnInteger { var, value } => {
                write!(f, "variable '{var}' is not an integer: '{value}'")
            }
            NegativeExponent => write!(f, "exponentiation with negative exponent"),
            ShiftCountOutOfRange { count } => write!(f, "shift count out of range: {count}"),
            ReadonlyVar(name) => write!(f, "{name}: readonly variable"),
            // Bash-mapped kinds render their bash text under Display too.
            other => write!(f, "{}", ArithError::plain(other.clone()).bash_message()),
        }
    }
}

pub fn parse(input: &str) -> Result<ArithExpr, ArithError> {
    let (tokens, offsets) = tokenize(input)?;
    let mut p = Parser {
        tokens,
        offsets,
        pos: 0,
        err_off: 0,
    };
    let expr = p.parse_comma_expr()?;
    if p.pos < p.tokens.len() {
        let off = p.offsets[p.pos];
        return Err(ArithError::at(ArithErrorKind::SyntaxErrorInExpression, off));
    }
    Ok(expr)
}

/// Builds bash's post-prologue error body:
/// `"<expr>: <msg> (error token is \"<tok>\")"`.
///
/// Normally `<expr>` is `src` leading-trimmed and `<tok>` is `src[offset..]`
/// (token runs to end of source — parse and eval errors). When the error
/// carries an explicit `token_end` (tokenize-time NUMBER errors), bash reports
/// only the full number run: the echo is truncated to `src[..end]` and the
/// token to `src[offset..end]` — no trailing content.
pub fn render_error_body(src: &str, err: &ArithError) -> String {
    let (expr, tok): (&str, &str) = match (err.offset, err.token_end) {
        (Some(off), Some(end)) if end <= src.len() && off <= end => {
            (src[..end].trim_start(), &src[off..end])
        }
        (Some(off), _) if off <= src.len() => (src.trim_start(), &src[off..]),
        _ => (src.trim_start(), ""),
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
        ArithExpr::Index {
            name,
            subscript,
            subscript_raw,
        } => Some(LValue::Element {
            name,
            subscript,
            subscript_raw,
        }),
        _ => None,
    }
}

fn assign_op_from_token(t: &ArithToken) -> Option<AssignOp> {
    match t {
        ArithToken::Assign => Some(AssignOp::Set),
        ArithToken::PlusEq => Some(AssignOp::Add),
        ArithToken::MinusEq => Some(AssignOp::Sub),
        ArithToken::StarEq => Some(AssignOp::Mul),
        ArithToken::SlashEq => Some(AssignOp::Div),
        ArithToken::PercentEq => Some(AssignOp::Mod),
        ArithToken::ShlEq => Some(AssignOp::Shl),
        ArithToken::ShrEq => Some(AssignOp::Shr),
        ArithToken::AmpEq => Some(AssignOp::BitAnd),
        ArithToken::CaretEq => Some(AssignOp::BitXor),
        ArithToken::PipeEq => Some(AssignOp::BitOr),
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
                    None => {
                        return Err(ArithError::parse("postfix ++/-- requires variable on LHS"));
                    }
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
                if 2 < min_bp {
                    break;
                }
                self.bump();
                let target = match expr_to_lvalue(lhs) {
                    Some(t) => t,
                    None => return Err(self.fail(ArithErrorKind::AssignToNonVar)),
                };
                let rhs = self.parse_expr(1)?; // rbp = 1 allows cascading assigns
                lhs = ArithExpr::Assign {
                    target,
                    op: assign_op,
                    rhs: Box::new(rhs),
                };
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
                let else_branch = self.parse_expr(3)?; // rbp = 3 for right-assoc
                lhs =
                    ArithExpr::Ternary(Box::new(lhs), Box::new(then_branch), Box::new(else_branch));
                continue;
            }

            // 4. Power ** (lbp = 25, rbp = 24 — right-associative).
            if op == ArithToken::Power && 25 >= min_bp {
                self.bump();
                let rhs = self.parse_expr(24)?;
                lhs = ArithExpr::Pow(Box::new(lhs), Box::new(rhs));
                continue;
            }

            // 4b. Div/Mod (lbp = 22, rbp = 23 — left-associative, special-cased
            //     to carry the divisor token's byte offset for by-zero error reporting).
            if matches!(op, ArithToken::Slash | ArithToken::Percent) && 22 >= min_bp {
                self.bump();
                let rhs = self.parse_expr(23)?;
                let off = self.err_off;
                lhs = if op == ArithToken::Slash {
                    ArithExpr::Div(Box::new(lhs), Box::new(rhs), off)
                } else {
                    ArithExpr::Mod(Box::new(lhs), Box::new(rhs), off)
                };
                continue;
            }

            // 5. Standard left-associative binops via the precedence table.
            let (lbp, rbp, make): BinOpEntry = match op {
                ArithToken::OrOr => (4, 5, ArithExpr::Or),
                ArithToken::AndAnd => (6, 7, ArithExpr::And),
                ArithToken::Pipe => (8, 9, ArithExpr::BitOr),
                ArithToken::Caret => (10, 11, ArithExpr::BitXor),
                ArithToken::Amp => (12, 13, ArithExpr::BitAnd),
                ArithToken::Eq => (14, 15, ArithExpr::Eq),
                ArithToken::Ne => (14, 15, ArithExpr::Ne),
                ArithToken::Lt => (16, 17, ArithExpr::Lt),
                ArithToken::Le => (16, 17, ArithExpr::Le),
                ArithToken::Gt => (16, 17, ArithExpr::Gt),
                ArithToken::Ge => (16, 17, ArithExpr::Ge),
                ArithToken::Shl => (18, 19, ArithExpr::Shl),
                ArithToken::Shr => (18, 19, ArithExpr::Shr),
                ArithToken::Plus => (20, 21, ArithExpr::Add),
                ArithToken::Minus => (20, 21, ArithExpr::Sub),
                ArithToken::Star => (22, 23, ArithExpr::Mul),
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

    /// Parses `[subscript]` given the current token is `LBracket`. Returns
    /// the parsed subscript expression (for indexed arrays) and the raw
    /// inner source text (for associative-array keys).
    fn parse_subscript(&mut self) -> Result<(ArithExpr, String), ArithError> {
        let raw = match self.bump() {
            Some(ArithToken::LBracket(raw)) => raw,
            other => return Err(ArithError::parse(format!("expected '[', got {other:?}"))),
        };
        let subscript = self.parse_comma_expr()?;
        match self.bump() {
            Some(ArithToken::RBracket) => {}
            other => {
                return Err(ArithError::parse(format!(
                    "expected ']' after array subscript, got {other:?}"
                )));
            }
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
                    Ok(ArithExpr::Index {
                        name: s,
                        subscript: Box::new(subscript),
                        subscript_raw,
                    })
                } else {
                    Ok(ArithExpr::Var(s))
                }
            }
            Some(ArithToken::Minus) => {
                let inner = self.parse_expr(26)?;
                Ok(ArithExpr::Neg(Box::new(inner)))
            }
            Some(ArithToken::Plus) => self.parse_expr(26),
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
                    None => Err(ArithError::parse("prefix ++ requires variable")),
                }
            }
            Some(ArithToken::MinusMinus) => {
                // --name / --name[sub]: prefix decrement requires an lvalue.
                let inner = self.parse_prefix()?;
                match expr_to_lvalue(inner) {
                    Some(target) => Ok(ArithExpr::PreDec(target)),
                    None => Err(ArithError::parse("prefix -- requires variable")),
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
    raw.parse::<i64>().map_err(|_| {
        ArithError::plain(ArithErrorKind::NotAnInteger {
            var: name.to_string(),
            value: raw,
        })
    })
}

/// Writes an i64 back to a shell variable as a decimal string.
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) -> Result<(), ArithError> {
    shell
        .try_set(name, value.to_string())
        .map_err(|_| ArithError::plain(ArithErrorKind::ReadonlyVar(name.to_string())))
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
    let parsed = parse(&raw).map_err(|_| {
        ArithError::plain(ArithErrorKind::NotAnInteger {
            var: name.to_string(),
            value: raw.clone(),
        })
    })?;
    eval(&parsed, shell)
}

/// Reads the current i64 value of an lvalue (scalar var or array element).
fn read_lvalue_i64(shell: &mut Shell, target: &LValue) -> Result<i64, ArithError> {
    match target {
        LValue::Scalar(name) => read_var_i64(shell, name),
        LValue::Element {
            name,
            subscript,
            subscript_raw,
        } => {
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
        LValue::Element {
            name,
            subscript,
            subscript_raw,
        } => {
            if shell.get_associative(name).is_some() {
                shell
                    .set_associative_element(name, subscript_raw.clone(), value.to_string())
                    .map_err(|_| ArithError::plain(ArithErrorKind::ReadonlyVar(name.to_string())))
            } else {
                let idx = eval(subscript, shell)?;
                shell
                    .set_indexed_element(name, idx as usize, value.to_string())
                    .map_err(|_| ArithError::plain(ArithErrorKind::ReadonlyVar(name.to_string())))
            }
        }
    }
}

/// Validates that `count` is in `[0, 64)` (the legal shift range) and returns
/// it as `u32`. Returns `ShiftCountOutOfRange` on failure.
fn check_shift_count(count: i64) -> Result<u32, ArithError> {
    if !(0..64).contains(&count) {
        return Err(ArithError::plain(ArithErrorKind::ShiftCountOutOfRange {
            count,
        }));
    }
    Ok(count as u32)
}

pub fn eval(expr: &ArithExpr, shell: &mut Shell) -> Result<i64, ArithError> {
    match expr {
        ArithExpr::Num(n) => Ok(*n),
        ArithExpr::Var(name) => {
            let raw = shell.lookup_var(name).unwrap_or_default();
            if raw.is_empty() {
                Ok(0)
            } else {
                raw.parse::<i64>().map_err(|_| {
                    ArithError::plain(ArithErrorKind::NotAnInteger {
                        var: name.clone(),
                        value: raw.to_string(),
                    })
                })
            }
        }
        ArithExpr::Index {
            name,
            subscript,
            subscript_raw,
        } => read_lvalue_i64(
            shell,
            &LValue::Element {
                name: name.clone(),
                subscript: subscript.clone(),
                subscript_raw: subscript_raw.clone(),
            },
        ),
        ArithExpr::Neg(e) => Ok(eval(e, shell)?.wrapping_neg()),
        ArithExpr::Not(e) => Ok(if eval(e, shell)? == 0 { 1 } else { 0 }),
        ArithExpr::Add(a, b) => Ok(eval(a, shell)?.wrapping_add(eval(b, shell)?)),
        ArithExpr::Sub(a, b) => Ok(eval(a, shell)?.wrapping_sub(eval(b, shell)?)),
        ArithExpr::Mul(a, b) => Ok(eval(a, shell)?.wrapping_mul(eval(b, shell)?)),
        ArithExpr::Div(a, b, off) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if rhs == 0 {
                return Err(ArithError::at(ArithErrorKind::DivisionByZero, *off));
            }
            Ok(lhs.wrapping_div(rhs))
        }
        ArithExpr::Mod(a, b, off) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            if rhs == 0 {
                return Err(ArithError::at(ArithErrorKind::ModuloByZero, *off));
            }
            Ok(lhs.wrapping_rem(rhs))
        }
        ArithExpr::Eq(a, b) => Ok(bool_to_i64(eval(a, shell)? == eval(b, shell)?)),
        ArithExpr::Ne(a, b) => Ok(bool_to_i64(eval(a, shell)? != eval(b, shell)?)),
        ArithExpr::Lt(a, b) => Ok(bool_to_i64(eval(a, shell)? < eval(b, shell)?)),
        ArithExpr::Le(a, b) => Ok(bool_to_i64(eval(a, shell)? <= eval(b, shell)?)),
        ArithExpr::Gt(a, b) => Ok(bool_to_i64(eval(a, shell)? > eval(b, shell)?)),
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
            eval(r, shell) // value of a comma sequence is the last operand
        }
        ArithExpr::BitAnd(a, b) => Ok(eval(a, shell)? & eval(b, shell)?),
        ArithExpr::BitOr(a, b) => Ok(eval(a, shell)? | eval(b, shell)?),
        ArithExpr::BitXor(a, b) => Ok(eval(a, shell)? ^ eval(b, shell)?),
        ArithExpr::BitNot(e) => Ok(!eval(e, shell)?),
        ArithExpr::Shl(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            Ok(lhs.wrapping_shl(check_shift_count(rhs)?))
        }
        ArithExpr::Shr(a, b) => {
            let lhs = eval(a, shell)?;
            let rhs = eval(b, shell)?;
            Ok(lhs.wrapping_shr(check_shift_count(rhs)?))
        }
        ArithExpr::Pow(a, b) => {
            let base = eval(a, shell)?;
            let exp = eval(b, shell)?;
            if exp < 0 {
                return Err(ArithError::plain(ArithErrorKind::NegativeExponent));
            }
            Ok(base.wrapping_pow(exp as u32))
        }
        ArithExpr::Assign { target, op, rhs } => {
            let rhs_val = eval(rhs, shell)?;
            let new_val = match op {
                AssignOp::Set => rhs_val,
                AssignOp::Add => read_lvalue_i64(shell, target)?.wrapping_add(rhs_val),
                AssignOp::Sub => read_lvalue_i64(shell, target)?.wrapping_sub(rhs_val),
                AssignOp::Mul => read_lvalue_i64(shell, target)?.wrapping_mul(rhs_val),
                AssignOp::Div => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if rhs_val == 0 {
                        return Err(ArithError::plain(ArithErrorKind::DivisionByZero));
                    }
                    lhs.wrapping_div(rhs_val)
                }
                AssignOp::Mod => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    if rhs_val == 0 {
                        return Err(ArithError::plain(ArithErrorKind::ModuloByZero));
                    }
                    lhs.wrapping_rem(rhs_val)
                }
                AssignOp::Shl => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    lhs.wrapping_shl(check_shift_count(rhs_val)?)
                }
                AssignOp::Shr => {
                    let lhs = read_lvalue_i64(shell, target)?;
                    lhs.wrapping_shr(check_shift_count(rhs_val)?)
                }
                AssignOp::BitAnd => read_lvalue_i64(shell, target)? & rhs_val,
                AssignOp::BitXor => read_lvalue_i64(shell, target)? ^ rhs_val,
                AssignOp::BitOr => read_lvalue_i64(shell, target)? | rhs_val,
            };
            write_lvalue_i64(shell, target, new_val)?;
            Ok(new_val)
        }
        ArithExpr::PreInc(target) => do_pre_incdec(shell, target, 1),
        ArithExpr::PreDec(target) => do_pre_incdec(shell, target, -1),
        ArithExpr::PostInc(target) => do_post_incdec(shell, target, 1),
        ArithExpr::PostDec(target) => do_post_incdec(shell, target, -1),
    }
}

/// Pre-increment/decrement: read→apply delta→write→return new value.
fn do_pre_incdec(shell: &mut Shell, target: &LValue, delta: i64) -> Result<i64, ArithError> {
    let new_val = read_lvalue_i64(shell, target)?.wrapping_add(delta);
    write_lvalue_i64(shell, target, new_val)?;
    Ok(new_val)
}

/// Post-increment/decrement: read old value→write (old+delta)→return old value.
fn do_post_incdec(shell: &mut Shell, target: &LValue, delta: i64) -> Result<i64, ArithError> {
    let old_val = read_lvalue_i64(shell, target)?;
    write_lvalue_i64(shell, target, old_val.wrapping_add(delta))?;
    Ok(old_val)
}

fn bool_to_i64(b: bool) -> i64 {
    if b { 1 } else { 0 }
}

#[cfg(test)]
mod tests;
