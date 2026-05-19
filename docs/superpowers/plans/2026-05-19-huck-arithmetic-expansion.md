# huck v11: Arithmetic Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `$((expr))` arithmetic expansion with C-style core operators, i64 wrapping arithmetic, non-recursive variable lookup, and bash-compatible error semantics.

**Architecture:** A new `src/arith.rs` module owns the AST, tokenizer, Pratt parser, evaluator, and error type. The shell lexer detects `$((` at token time, scans the inner text with paren-depth tracking, parses it eagerly, and stores a new `WordPart::Arith` variant. Expand-time evaluation looks up variables from the live `Shell` and renders the integer result into the surrounding `Field`.

**Tech Stack:** Rust 2024 edition, no new dependencies, hand-written Pratt parser.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-18-huck-arithmetic-expansion-design.md`.

---

## File Map

- **Create:** `src/arith.rs` — AST, tokenizer, parser, evaluator, error type
- **Create:** `tests/arith_integration.rs` — end-to-end via shell binary
- **Modify:** `src/lib.rs` or `src/main.rs` — register the new module
- **Modify:** `src/lexer.rs` — add `$((` detection, `WordPart::Arith` variant, `LexError::UnterminatedArith` and `LexError::ArithParse` variants
- **Modify:** `src/expand.rs` — new `Arith` arm in `expand` and `expand_assignment`
- **Modify:** `src/command.rs` — verify no exhaustive matches on `WordPart` break (none expected; if any exist, add `_` arm)
- **Modify:** `src/executor.rs` — same as command.rs
- **Modify:** `README.md` — v11 status row, features section, test count

---

## Task 1: AST and error type

Create `src/arith.rs` with the `ArithExpr` enum and `ArithError` enum. No parsing or evaluation yet — just the types and a `Display` impl for the error.

**Files:**
- Create: `src/arith.rs`
- Modify: `src/main.rs` (register module)

- [ ] **Step 1: Write failing tests for the error Display impl**

Create `src/arith.rs` with this header and the test module at the bottom:

```rust
//! Arithmetic expansion: AST, parser, and evaluator for `$((expr))`.

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
}
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs`. Find the existing `mod` declarations near the top and add:

```rust
mod arith;
```

(Place it alphabetically with the existing `mod expand;`, `mod executor;`, etc.)

- [ ] **Step 3: Run tests**

Run: `cargo test --test '*' --quiet 2>&1 | tail -5 && cargo test arith:: --quiet 2>&1 | tail -10`

Or simply: `cargo test arith`
Expected: 4 new tests pass; build succeeds with a dead-code warning on `ArithExpr` variants (expected — they get used in Task 3).

- [ ] **Step 4: Commit**

```bash
git add src/arith.rs src/main.rs
git commit -m "v11 task 1: add ArithExpr AST and ArithError type"
```

---

## Task 2: Arith tokenizer

Add an internal token type and a tokenizer function to `src/arith.rs`. Whitespace skipped; multi-char operators (`==`, `!=`, `<=`, `>=`, `&&`, `||`) recognized; leading `$` before an identifier silently stripped.

**Files:**
- Modify: `src/arith.rs`

- [ ] **Step 1: Write failing tests for the tokenizer**

Add to the `tests` module in `src/arith.rs`:

```rust
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
    // Bitwise `&` is out of scope for v11; a lone `&` is unexpected.
    let err = tokenize("1 & 2").unwrap_err();
    assert!(matches!(err, ArithError::Parse(_)));
}

#[test]
fn tokenize_single_pipe_is_parse_error() {
    let err = tokenize("1 | 2").unwrap_err();
    assert!(matches!(err, ArithError::Parse(_)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test arith::tests::tokenize_`
Expected: FAIL — `tokenize`, `ArithToken` not defined.

- [ ] **Step 3: Implement the tokenizer**

Add to `src/arith.rs` (above the test module):

```rust
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
                // Strip the `$` prefix and continue as if reading an identifier.
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test arith::tests::tokenize_`
Expected: PASS (all 12 tokenizer tests).

- [ ] **Step 5: Commit**

```bash
git add src/arith.rs
git commit -m "v11 task 2: arith tokenizer with whitespace skipping and \$-prefix stripping"
```

---

## Task 3: Pratt parser

Add `pub fn parse(input: &str) -> Result<ArithExpr, ArithError>`. Tokenize then run a Pratt parser following the precedence table in the spec.

**Files:**
- Modify: `src/arith.rs`

- [ ] **Step 1: Write failing tests for the parser**

Add to the `tests` module in `src/arith.rs`:

```rust
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
    // 1-2-3 → (1-2)-3
    assert_eq!(
        parse("1-2-3").unwrap(),
        ArithExpr::Sub(Box::new(ArithExpr::Sub(n(1), n(2))), n(3))
    );
}

#[test]
fn parse_multiplication_binds_tighter_than_addition() {
    // 1+2*3 → 1+(2*3)
    assert_eq!(
        parse("1+2*3").unwrap(),
        ArithExpr::Add(n(1), Box::new(ArithExpr::Mul(n(2), n(3))))
    );
}

#[test]
fn parse_parenthesized_overrides_precedence() {
    // (1+2)*3 → (1+2)*3
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
    // 1<2 == 1 → (1<2) == 1
    assert_eq!(
        parse("1<2 == 1").unwrap(),
        ArithExpr::Eq(Box::new(ArithExpr::Lt(n(1), n(2))), n(1))
    );
}

#[test]
fn parse_logical_and_binds_tighter_than_or() {
    // a || b && c → a || (b && c)
    assert_eq!(
        parse("a||b&&c").unwrap(),
        ArithExpr::Or(v("a"), Box::new(ArithExpr::And(v("b"), v("c"))))
    );
}

#[test]
fn parse_ternary_right_associative() {
    // a?b:c?d:e → a?b:(c?d:e)
    assert_eq!(
        parse("a?b:c?d:e").unwrap(),
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test arith::tests::parse_`
Expected: FAIL — `parse` not defined.

- [ ] **Step 3: Implement the Pratt parser**

Add to `src/arith.rs` (above the test module, after `tokenize`):

```rust
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

    /// Pratt parser. `min_bp` is the minimum binding power that callers
    /// allow on the right side; an operator with lower binding power
    /// returns control to the caller.
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
                let else_branch = self.parse_expr(1)?; // right-assoc
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
                let inner = self.parse_expr(14)?; // unary > binary mul/div
                Ok(ArithExpr::Neg(Box::new(inner)))
            }
            Some(ArithToken::Plus) => {
                // Unary `+` is a no-op; just parse the operand.
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test arith::tests::parse_`
Expected: PASS (all 17 parser tests).

- [ ] **Step 5: Commit**

```bash
git add src/arith.rs
git commit -m "v11 task 3: Pratt parser for arithmetic expressions"
```

---

## Task 4: Evaluator

Add `pub fn eval(expr: &ArithExpr, shell: &Shell) -> Result<i64, ArithError>`. Wrapping arithmetic, short-circuit logical ops, comparisons return 1/0, variable lookup parses value as i64 (empty/unset → 0).

**Files:**
- Modify: `src/arith.rs`

- [ ] **Step 1: Write failing tests for the evaluator**

Add to the `tests` module in `src/arith.rs`:

```rust
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
    assert_eq!(eval_str("-7/2", &s).unwrap(), -3); // Rust i64 division
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
    // Right side would divide by zero; if short-circuit works, no error.
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
    // i64::MAX + 1 wraps to i64::MIN.
    let max = i64::MAX.to_string();
    let expr = format!("{max} + 1");
    assert_eq!(eval_str(&expr, &s).unwrap(), i64::MIN);
}

#[test]
fn eval_unset_var_is_zero() {
    let s = Shell::new();
    // Use a name that won't be in the environment.
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test arith::tests::eval_`
Expected: FAIL — `eval` not defined.

- [ ] **Step 3: Implement the evaluator**

Add to `src/arith.rs` (above the test module, after `parse`):

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test arith::tests::eval_`
Expected: PASS (all 21 evaluator tests).

- [ ] **Step 5: Run the full suite to catch regressions**

Run: `cargo test`
Expected: 318 baseline + ~54 new arith tests = ~372 passing, 0 failed.

- [ ] **Step 6: Commit**

```bash
git add src/arith.rs
git commit -m "v11 task 4: arithmetic evaluator with wrapping and short-circuit"
```

---

## Task 5: WordPart::Arith variant, LexError variants, expand arms

Wire the new variant into the WordPart enum, add the two new LexError variants, and add `Arith` arms in `expand` and `expand_assignment`. The expand arms are functional — they call `arith::eval`. No lexer production yet (Task 6), so the new variant has no producers and the arms are dead at runtime until Task 6 lands. Rust will warn on dead code; that's expected.

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/expand.rs`

- [ ] **Step 1: Write a failing test that constructs WordPart::Arith manually and pipes through expand**

Add to `src/expand.rs` test module:

```rust
#[test]
fn expand_arith_part_renders_decimal_result() {
    use crate::arith::ArithExpr;
    let mut shell = Shell::new();
    // Build a Word containing only an Arith part: $((2 + 3)).
    let word = Word(vec![WordPart::Arith {
        expr: ArithExpr::Add(
            Box::new(ArithExpr::Num(2)),
            Box::new(ArithExpr::Num(3)),
        ),
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "5");
    assert_eq!(fields[0].quoted, vec![true]);
}

#[test]
fn expand_arith_part_division_by_zero_yields_empty_field_and_sets_status() {
    use crate::arith::ArithExpr;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Arith {
        expr: ArithExpr::Div(
            Box::new(ArithExpr::Num(1)),
            Box::new(ArithExpr::Num(0)),
        ),
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    // No other parts, fallback emits one empty field.
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "");
    assert_eq!(shell.last_status(), 1);
}

#[test]
fn expand_assignment_arith_part_renders_decimal() {
    use crate::arith::ArithExpr;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Arith {
        expr: ArithExpr::Mul(
            Box::new(ArithExpr::Num(6)),
            Box::new(ArithExpr::Num(7)),
        ),
        quoted: false,
    }]);
    let value = expand_assignment(&word, &mut shell);
    assert_eq!(value, "42");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_arith_ expand_assignment_arith_`
Expected: FAIL — `WordPart::Arith` variant does not exist.

- [ ] **Step 3: Add the WordPart::Arith variant**

Edit `src/lexer.rs`. Find the `pub enum WordPart` definition (around line 34) and add the new variant:

```rust
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
    Arith { expr: crate::arith::ArithExpr, quoted: bool },
}
```

- [ ] **Step 4: Add the LexError variants**

Edit `src/lexer.rs`. Find `pub enum LexError` (lines 1-9) and add two variants:

```rust
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    UnterminatedArith,
    ArithParse(String),
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
}
```

- [ ] **Step 5: Add `Arith` arm in expand**

Edit `src/expand.rs`. Find the per-WordPart `match` block in `expand` (where the other `WordPart::Literal`/`Tilde`/etc. arms live). Add this arm:

```rust
WordPart::Arith { expr, quoted: _ } => {
    has_emitted = true; // ensure end-of-word emits at least an empty Field
    match crate::arith::eval(expr, shell) {
        Ok(n) => current.push_str(&n.to_string(), true),
        Err(e) => {
            eprintln!("huck: arithmetic: {}", e);
            shell.set_last_status(1);
            // Append nothing; the field stays empty if no other parts.
        }
    }
}
```

(Place it after the existing `CommandSub` arm.)

- [ ] **Step 6: Add `Arith` arm in expand_assignment**

Edit `src/expand.rs`. Find `pub fn expand_assignment` (around line 178). The per-WordPart match block uses a `String` accumulator named `result`. Add this arm alongside the existing `WordPart::Literal` / `Tilde` / `Var` arms:

```rust
WordPart::Arith { expr, quoted: _ } => {
    match crate::arith::eval(expr, shell) {
        Ok(n) => result.push_str(&n.to_string()),
        Err(e) => {
            eprintln!("huck: arithmetic: {}", e);
            shell.set_last_status(1);
            // Append nothing.
        }
    }
}
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test expand_arith_ expand_assignment_arith_`
Expected: PASS (3 new tests).

- [ ] **Step 8: Run the full suite**

Run: `cargo test`
Expected: all tests pass. Build may warn about unused variants `UnterminatedArith` / `ArithParse` (no producer until Task 6) — that's expected.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs src/expand.rs
git commit -m "v11 task 5: WordPart::Arith variant and expand arms"
```

---

## Task 6: Lexer `$((..))` scanning

Detect `$((` in `read_dollar_expansion`, scan the inner text with paren-depth tracking, call `arith::parse`, push `WordPart::Arith`.

**Files:**
- Modify: `src/lexer.rs`

- [ ] **Step 1: Write failing lexer tests**

Add to `src/lexer.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn tokenize_arith_simple() {
    use crate::arith::ArithExpr;
    let tokens = tokenize("$((1+2))").unwrap();
    assert_eq!(tokens.len(), 1);
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert_eq!(parts.len(), 1);
    let WordPart::Arith { expr, quoted } = &parts[0] else {
        panic!("expected Arith part, got {:?}", parts[0])
    };
    assert_eq!(*quoted, false);
    assert_eq!(*expr, ArithExpr::Add(
        Box::new(ArithExpr::Num(1)),
        Box::new(ArithExpr::Num(2)),
    ));
}

#[test]
fn tokenize_arith_with_nested_parens() {
    use crate::arith::ArithExpr;
    let tokens = tokenize("$(( (1+2) * 3 ))").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::Arith { expr, .. } = &parts[0] else { panic!() };
    assert_eq!(*expr, ArithExpr::Mul(
        Box::new(ArithExpr::Add(
            Box::new(ArithExpr::Num(1)),
            Box::new(ArithExpr::Num(2)),
        )),
        Box::new(ArithExpr::Num(3)),
    ));
}

#[test]
fn tokenize_arith_inside_double_quotes_is_quoted() {
    let tokens = tokenize("\"$((1+2))\"").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::Arith { quoted, .. } = &parts[0] else { panic!() };
    assert_eq!(*quoted, true);
}

#[test]
fn tokenize_arith_unterminated_returns_error() {
    let err = tokenize("$((1+2").unwrap_err();
    assert_eq!(err, LexError::UnterminatedArith);
}

#[test]
fn tokenize_arith_parse_error_returns_arith_parse_err() {
    let err = tokenize("$((1+))").unwrap_err();
    assert!(matches!(err, LexError::ArithParse(_)), "got {:?}", err);
}

#[test]
fn tokenize_arith_and_command_sub_both_recognized() {
    // Make sure $(( wins over $( for the same input prefix.
    let tokens = tokenize("$((1)) $(echo x)").unwrap();
    // Word 1: Arith; Word 2: CommandSub.
    let Token::Word(Word(parts1)) = &tokens[0] else { panic!() };
    assert!(matches!(parts1[0], WordPart::Arith { .. }));
    let Token::Word(Word(parts2)) = &tokens[1] else { panic!() };
    assert!(matches!(parts2[0], WordPart::CommandSub { .. }));
}

#[test]
fn tokenize_arith_var_with_dollar_prefix_inside() {
    use crate::arith::ArithExpr;
    let tokens = tokenize("$(($x))").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::Arith { expr, .. } = &parts[0] else { panic!() };
    assert_eq!(*expr, ArithExpr::Var("x".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test tokenize_arith_`
Expected: FAIL — `$((` not detected, falls through to existing `$(` CommandSub path which yields wrong shape.

- [ ] **Step 3: Add a helper for arith inner scanning**

Add this helper function in `src/lexer.rs`, alongside the existing `scan_paren_substitution` (around line 311):

```rust
/// Reads the inner text of a `$((...))` arithmetic expansion. The opening
/// `$((` has already been consumed; this function scans forward until the
/// matching `))` at depth 0. Returns the inner text (without the closing
/// `))`). Tracks paren depth so that nested `(` / `)` inside the
/// expression do not prematurely close the expansion.
fn scan_arith_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: u32 = 1; // we are inside the outer `((`
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedArith),
            Some('(') => {
                depth += 1;
                body.push('(');
            }
            Some(')') => {
                if depth == 1 {
                    // The next char must be `)` to close `))`.
                    match chars.next() {
                        Some(')') => return Ok(body),
                        Some(_) | None => return Err(LexError::UnterminatedArith),
                    }
                } else {
                    depth -= 1;
                    body.push(')');
                }
            }
            Some(c) => body.push(c),
        }
    }
}
```

- [ ] **Step 4: Wire `$((` detection into `read_dollar_expansion`**

Edit `read_dollar_expansion` in `src/lexer.rs` (around line 275). Replace the existing `Some('(')` arm:

```rust
Some('(') => {
    chars.next(); // consume first '('
    if chars.peek() == Some(&'(') {
        chars.next(); // consume second '(' — this is `$((`
        let inner = scan_arith_body(chars)?;
        let expr = crate::arith::parse(&inner)
            .map_err(|e| LexError::ArithParse(e.to_string()))?;
        parts.push(WordPart::Arith { expr, quoted });
    } else {
        let sequence = scan_paren_substitution(chars)?;
        parts.push(WordPart::CommandSub { sequence, quoted });
    }
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test tokenize_arith_`
Expected: PASS (7 new tests).

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: all tests pass. Dead-code warnings on `UnterminatedArith` / `ArithParse` should be gone now.

- [ ] **Step 7: Manual smoke test**

```bash
cargo build --release
~/projects/shuck/target/release/huck <<'EOF'
echo $((2+3*4))
echo $((-5))
x=10
echo $((x+1))
FOO=$((1+1))
echo $FOO
echo "$((6*7))"
echo $((1/0))
exit
EOF
```

Expected output (order may include `huck>` prompt lines):
```
14
-5
11
2
42
huck: arithmetic: division by zero
```

(The empty stdout line for the `1/0` case is expected — echo got an empty argument.)

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "v11 task 6: lexer scans \$((...)) and produces WordPart::Arith"
```

---

## Task 7: Integration tests via shell binary

End-to-end tests spawn the built binary, feed stdin, assert stdout/stderr/exit status. Mirrors the smoke test from Task 6.

**Files:**
- Create: `tests/arith_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/arith_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn arith_basic_precedence() {
    let (out, _) = run("echo $((2+3*4))\nexit\n");
    assert!(out.lines().any(|l| l == "14"), "stdout: {out}");
}

#[test]
fn arith_with_variable() {
    let (out, _) = run("x=5\necho $((x*2))\nexit\n");
    assert!(out.lines().any(|l| l == "10"), "stdout: {out}");
}

#[test]
fn arith_assignment_rhs() {
    let (out, _) = run("FOO=$((1+1))\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn arith_negative_result() {
    let (out, _) = run("echo $((-5*3))\nexit\n");
    assert!(out.lines().any(|l| l == "-15"), "stdout: {out}");
}

#[test]
fn arith_inside_double_quotes() {
    let (out, _) = run("echo \"answer: $((6*7))\"\nexit\n");
    assert!(out.lines().any(|l| l == "answer: 42"), "stdout: {out}");
}

#[test]
fn arith_division_by_zero_writes_to_stderr() {
    let (_, err) = run("echo $((1/0))\nexit\n");
    assert!(err.contains("division by zero"), "stderr: {err}");
}

#[test]
fn arith_ternary() {
    let (out, _) = run("echo $((1<2 ? 100 : 200))\nexit\n");
    assert!(out.lines().any(|l| l == "100"), "stdout: {out}");
}

#[test]
fn arith_logical_short_circuit() {
    // The RHS `1/0` would error if evaluated; short-circuit must prevent that.
    let (out, err) = run("echo $((0 && 1/0))\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
    assert!(!err.contains("division by zero"), "stderr: {err}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test arith_integration`
Expected: all 8 tests pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (~380 total).

- [ ] **Step 4: Commit**

```bash
git add tests/arith_integration.rs
git commit -m "v11 task 7: end-to-end arithmetic expansion integration tests"
```

---

## Task 8: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add v11 row to the status table**

Find the status table in `README.md` and append after the v10 row:

```
| v11       | Arithmetic expansion (`$((expr))`)                      |
```

- [ ] **Step 2: Add the Arithmetic expansion subsection**

After the **Pathname expansion (v10):** block, add:

```markdown
**Arithmetic expansion (v11):**
`$((expr))` evaluates a C-style integer expression and substitutes
the decimal result into the surrounding word. Operators: `+`, `-`,
`*`, `/`, `%`, comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`),
logical (`&&`, `||`, `!`) with short-circuit, ternary (`?:`),
parentheses, unary `+`/`-`/`!`. Integers are 64-bit signed and
wrap on overflow (matches bash). Variables are referenced by bare
name (`x`) or with `$` (`$x`); unset/empty values are treated as 0;
non-integer values produce a stderr error and an empty result.
Bitwise operators, assignment operators, increment/decrement, and
non-decimal bases are not implemented.
```

- [ ] **Step 3: Remove the v11 reference from Not-yet-implemented**

Find the bullet `arithmetic expansion (`$((expr))` — coming in v11)` and remove it (leave the surrounding commas / sentence well-formed).

- [ ] **Step 4: Update the Syntax line**

Find:
```
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`.
```
Replace with:
```
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`, `echo $((2+3))`.
```

- [ ] **Step 5: Update the test count**

Run: `cargo test 2>&1 | grep 'test result' | awk '{ total += $4 } END { print total }'`

(Or simply count: 318 baseline + ~62 new arith tests = ~380. Verify by reading the actual `cargo test` output.)

Update the line `cargo test               # full test suite (318 tests)` to the new total.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "v11 task 8: README — add v11 row and arithmetic expansion section"
```

---

## Final review checkpoint

After Task 8:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session covering: `$((2+3*4))`, `$((1<2 ? 10 : 20))`, `$((1/0))` (expect stderr message), `x=5; echo $((x+1))`, `FOO=$((6*7)); echo $FOO`, `echo "$((9-3))"`
- [ ] Final-review the whole branch as a single diff before merging to `main`
