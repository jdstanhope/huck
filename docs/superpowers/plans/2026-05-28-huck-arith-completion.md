# huck v38 — Arithmetic Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close out the arithmetic-feature cluster by adding bitwise
operators (M-55), assignment + inc/dec operators (M-56), non-decimal
literals (M-57), and `**` exponentiation to huck's `$((…))`. After v38,
huck's arith reaches bash-arith feature parity (modulo deliberate
scope-outs documented in the spec).

**Architecture:** All changes confined to `src/arith.rs` (tokenizer +
parser + evaluator + AST), plus three call-site updates for the
`arith::eval` signature change to `&mut Shell` (two in `src/expand.rs`,
one in `src/param_expansion.rs`). Tokenizer adds 19 new token variants
and rewrites every single-char operator arm to handle multi-char
operators. AST adds 12 new `ArithExpr` variants plus one `AssignOp`
sub-enum. The Pratt parser's precedence table is renumbered with
assignment at the lowest precedence (right-associative, LHS-must-be-Var
enforced) and the new bitwise/shift levels slotted between comparisons
and `+`/`-`. Power (`**`) is special-cased for right-associativity at
parse time. Evaluator gains 12 new arms plus read-modify-write helpers
for variable mutation.

**Tech Stack:** Rust. No new dependencies. Reuses existing
`shell.set` / `shell.lookup_var` for variable read/write.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-arith-completion-design.md`

**Branch:** `v38-arith-completion` (already created and checked out).

---

### Task 1: Tokenizer extension

**Files:**
- Modify: `src/arith.rs` (`ArithToken` enum at line 4-12; `tokenize` function at line 14-125; tests module at line ~440)

**Note for implementer:** This task adds 19 new `ArithToken` variants
and rewrites every single-char operator arm in `tokenize` to handle
multi-char operator sequences. Numeric literal parsing is extended
to handle hex (`0x…`), octal (`0…`), and base-N (`N#…`) forms.

Read `src/arith.rs:4-125` first to understand the existing tokenizer
shape. The current pattern for two-char operators (e.g. `<` / `<=`)
uses a peek-then-conditionally-consume idiom. The new code extends
this to three levels (`<` / `<=` / `<<` / `<<=`).

Two existing tokenizer tests will need repurposing in this task because
their inputs are now legal:
- Line 449 (`tokenize_single_amp_is_parse_error`): `1 & 2` is now valid
  bitwise AND, not an error.
- Line 455 (`tokenize_single_pipe_is_parse_error`): same for `|`.

- [ ] **Step 1: Add the new `ArithToken` variants**

In `src/arith.rs`, replace the existing `ArithToken` enum (lines 4-12) with:

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
```

- [ ] **Step 2: Write failing tokenizer tests for the new operators**

Append to the `tests` module in `src/arith.rs` (search for the existing
`tokenize_single_amp_is_parse_error` to find the area; insert these
tests immediately after the existing tokenize_* tests, before the
parser tests):

```rust
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
```

- [ ] **Step 3: Update the two existing tokenizer tests for now-legal inputs**

In the existing `tokenize_single_amp_is_parse_error` test (around line
449), replace the body to assert the new behavior (bitwise AND):

```rust
    #[test]
    fn tokenize_single_amp_is_bitwise_and() {
        // v38: bare & is now bitwise AND (was: parse error).
        assert_eq!(tokenize("1 & 2").unwrap(), vec![
            ArithToken::Number(1), ArithToken::Amp, ArithToken::Number(2),
        ]);
    }
```

Same for `tokenize_single_pipe_is_parse_error` (around line 455):

```rust
    #[test]
    fn tokenize_single_pipe_is_bitwise_or() {
        // v38: bare | is now bitwise OR (was: parse error).
        assert_eq!(tokenize("1 | 2").unwrap(), vec![
            ArithToken::Number(1), ArithToken::Pipe, ArithToken::Number(2),
        ]);
    }
```

- [ ] **Step 4: Run the new tests to verify they fail**

Run: `cargo test --lib arith::tests::tokenize_ 2>&1 | tail -30`

Expected: at least 14 tests fail (compile error because new token variants exist but the tokenizer doesn't emit them yet OR runtime errors because the tokenizer rejects the input).

If the entire `arith::tests` module fails to compile because of the
two repurposed tests referring to assertions that don't match current
behavior, that's expected too — the failing assertions will become
passing once the tokenizer is updated in Step 5.

- [ ] **Step 5: Add the numeric-literal helpers**

In `src/arith.rs`, just above the `tokenize` function (around line 14),
add these two helper functions:

```rust
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
            'a'..='z' => {
                let v = (c as u32) - ('a' as u32) + 10;
                if base <= 36 { v } else { v }
            }
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
```

- [ ] **Step 6: Rewrite the digit arm in `tokenize`**

In `src/arith.rs::tokenize`, replace the existing `'0'..='9' =>` arm
(lines 20-28) with:

```rust
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
```

- [ ] **Step 7: Rewrite each operator arm to handle multi-char tokens**

In `src/arith.rs::tokenize`, update the existing operator arms. Each
needs to peek for second-char alternatives.

Replace the `'+' =>` arm (line 54):

```rust
            '+' => {
                chars.next();
                match chars.peek() {
                    Some('+') => { chars.next(); out.push(ArithToken::PlusPlus); }
                    Some('=') => { chars.next(); out.push(ArithToken::PlusEq); }
                    _ => out.push(ArithToken::Plus),
                }
            }
```

Replace the `'-' =>` arm:

```rust
            '-' => {
                chars.next();
                match chars.peek() {
                    Some('-') => { chars.next(); out.push(ArithToken::MinusMinus); }
                    Some('=') => { chars.next(); out.push(ArithToken::MinusEq); }
                    _ => out.push(ArithToken::Minus),
                }
            }
```

Replace the `'*' =>` arm:

```rust
            '*' => {
                chars.next();
                match chars.peek() {
                    Some('*') => { chars.next(); out.push(ArithToken::Power); }
                    Some('=') => { chars.next(); out.push(ArithToken::StarEq); }
                    _ => out.push(ArithToken::Star),
                }
            }
```

Replace the `'/' =>` arm:

```rust
            '/' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::SlashEq);
                } else {
                    out.push(ArithToken::Slash);
                }
            }
```

Replace the `'%' =>` arm:

```rust
            '%' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::PercentEq);
                } else {
                    out.push(ArithToken::Percent);
                }
            }
```

Replace the `'=' =>` arm (currently errors; lines 70-79):

```rust
            '=' => {
                chars.next();
                if chars.peek() == Some(&'=') {
                    chars.next();
                    out.push(ArithToken::Eq);
                } else {
                    out.push(ArithToken::Assign);
                }
            }
```

Replace the `'<' =>` arm (lines 81-89):

```rust
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
```

Replace the `'>' =>` arm (lines 90-98) symmetrically (`>` `>>` `>>=`
`>=`).

Replace the `'&' =>` arm (lines 99-108):

```rust
            '&' => {
                chars.next();
                match chars.peek() {
                    Some('&') => { chars.next(); out.push(ArithToken::AndAnd); }
                    Some('=') => { chars.next(); out.push(ArithToken::AmpEq); }
                    _ => out.push(ArithToken::Amp),
                }
            }
```

Replace the `'|' =>` arm (lines 109-118):

```rust
            '|' => {
                chars.next();
                match chars.peek() {
                    Some('|') => { chars.next(); out.push(ArithToken::OrOr); }
                    Some('=') => { chars.next(); out.push(ArithToken::PipeEq); }
                    _ => out.push(ArithToken::Pipe),
                }
            }
```

Add NEW arms for `^` and `~` (before the `other =>` catch-all):

```rust
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
```

- [ ] **Step 8: Build and run the tokenizer tests**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --lib arith::tests::tokenize_ 2>&1 | tail -20`
Expected: all 14 new tests pass; the 2 repurposed tests
(`tokenize_single_amp_is_bitwise_and`, `tokenize_single_pipe_is_bitwise_or`)
pass with their new assertions.

- [ ] **Step 9: Run the full arith test module to confirm no regression**

Run: `cargo test --lib arith::tests 2>&1 | tail -10`

Expected: tokenizer tests all pass. Some parser/eval tests will still
break (the `parse("--5")` / `eval_str("--5")` tests at lines 510 and
608) — those break because `--` is now a token. Those tests will be
fixed in Task 3 / Task 4 alongside the parser changes. For now, only
the tokenizer-level tests pass cleanly.

If `cargo test --lib arith` shows failures OUTSIDE the
`parse_unary_double_minus` / `eval_unary_minus` paths, STOP and
investigate.

- [ ] **Step 10: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean. The new token variants are constructed by tokenize
and used by tests — no dead-code warnings.

- [ ] **Step 11: Commit**

```bash
git add src/arith.rs
git commit -m "$(cat <<'EOF'
arith: tokenizer extension for bitwise / shift / assign / inc-dec / ** + non-decimal literals (v38 task 1)

Adds 19 new ArithToken variants (Amp, Pipe, Caret, Tilde, Shl, Shr,
Power, Assign, PlusEq, MinusEq, StarEq, SlashEq, PercentEq, ShlEq,
ShrEq, AmpEq, CaretEq, PipeEq, PlusPlus, MinusMinus). Every existing
single-char operator arm is rewritten to peek for second-char
alternatives. Numeric literal parsing handles hex (0x...), octal
(0...), and base-N (N#...) per bash. Removed three old "out of scope"
error sites (=, &, |) — those tokens are now legal. Repurposed two
existing tokenize_single_*_is_parse_error tests to assert the new
bitwise behavior. 14 new tokenizer unit tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: AST extension

**Files:**
- Modify: `src/arith.rs` (`ArithExpr` enum at lines 128-147; `ArithError` enum at lines 149-155; `Display` impl at lines 157-167)

**Note for implementer:** Pure data layer. Add new `ArithExpr` variants,
the `AssignOp` sub-enum, and two new `ArithError` variants with their
Display arms. No parser or evaluator changes — those land in Tasks 3
and 4. Build must stay clean after this task (with `#[allow(dead_code)]`
on the new variants if needed since the parser doesn't construct them
yet).

- [ ] **Step 1: Add the `AssignOp` enum**

In `src/arith.rs`, immediately above the `ArithExpr` enum (around line 127):

```rust
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
```

- [ ] **Step 2: Add the new `ArithExpr` variants**

In the `ArithExpr` enum, add these variants at the END (after `Ternary`):

```rust
    // v38 — bitwise binops:
    BitAnd(Box<ArithExpr>, Box<ArithExpr>),
    BitOr(Box<ArithExpr>, Box<ArithExpr>),
    BitXor(Box<ArithExpr>, Box<ArithExpr>),
    BitNot(Box<ArithExpr>),
    Shl(Box<ArithExpr>, Box<ArithExpr>),
    Shr(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — power (right-associative):
    Pow(Box<ArithExpr>, Box<ArithExpr>),
    // v38 — assignment (LHS must be a Var; enforced at parse time):
    Assign { name: String, op: AssignOp, rhs: Box<ArithExpr> },
    // v38 — pre/post inc/dec (LHS must be a Var):
    PreInc(String),
    PreDec(String),
    PostInc(String),
    PostDec(String),
```

- [ ] **Step 3: Add the new `ArithError` variants**

In the `ArithError` enum (around line 149-155), add at the end:

```rust
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
```

- [ ] **Step 4: Extend the `Display` impl**

In the `Display` impl for `ArithError` (around line 157-167), add two
arms inside the match:

```rust
            Self::NegativeExponent => write!(f, "exponentiation with negative exponent"),
            Self::ShiftCountOutOfRange { count } =>
                write!(f, "shift count out of range: {count}"),
```

- [ ] **Step 5: Build and run the full test suite**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build. The new `ArithExpr` variants and `ArithError`
variants are constructed only at the test layer + future tasks.

Run: `cargo test --lib arith::tests 2>&1 | tail -10`
Expected: same state as end of Task 1 — tokenizer tests pass, the
`--5` parser/eval tests still fail (Task 3/4 territory). No NEW
failures.

- [ ] **Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean. `#[allow(dead_code)]` on `AssignOp` silences the
variant-unused lint until Task 3 constructs them.

If clippy reports dead-code on `BitAnd`/`BitOr`/etc. variants, add
`#[allow(dead_code)]` directly on those variants too, with a comment
pointing to Task 3. (Variants inside a `pub enum` typically don't get
the lint, but binary crates sometimes do.)

- [ ] **Step 7: Commit**

```bash
git add src/arith.rs
git commit -m "$(cat <<'EOF'
arith: AST extension for bitwise / power / assign / inc-dec (v38 task 2)

Adds 12 new ArithExpr variants (BitAnd/BitOr/BitXor/BitNot/Shl/Shr/Pow/
Assign/PreInc/PreDec/PostInc/PostDec) plus AssignOp sub-enum (Set, Add,
Sub, Mul, Div, Mod, Shl, Shr, BitAnd, BitXor, BitOr). Two new
ArithError variants: NegativeExponent and ShiftCountOutOfRange{count}.
Display impl extended. AST enforces "LHS must be Var" structurally —
Assign carries a String name (not a sub-expression), and PreInc/PreDec/
PostInc/PostDec each carry a String name directly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Parser extension

**Files:**
- Modify: `src/arith.rs` (`parse_expr` at lines 201-242; `parse_prefix` at lines 244-273; existing test at line ~510 needs updating)

**Note for implementer:** The Pratt-parser precedence table is
renumbered to accommodate the new operators. The existing BPs (2-14)
shift to make room. Three special-cased constructs in `parse_expr`:
postfix `++`/`--` at the top of the loop, assignment dispatch before
the standard BP table, and power `**` with right-associative BPs.

The existing test at line 510 (`parse("--5")`) will need updating
because `--` is now tokenized as `MinusMinus` (prefix decrement)
which requires a variable name, not a number literal.

- [ ] **Step 1: Write the failing parser tests**

Append to the `tests` module in `src/arith.rs` (after the existing
parse_* tests, before the eval_* tests). The helper functions `n` and
`v` already exist (lines 461-462):

```rust
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
            name: "a".to_string(),
            op: AssignOp::Set,
            rhs: Box::new(ArithExpr::Assign {
                name: "b".to_string(),
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
                ArithExpr::Assign { name, op, rhs } => {
                    assert_eq!(name, "a", "for input {input}");
                    assert_eq!(op, expected_op, "for input {input}");
                    assert_eq!(*rhs, ArithExpr::Num(1), "for input {input}");
                }
                other => panic!("expected Assign for {input}, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_pre_post_inc_dec() {
        assert_eq!(parse("++a").unwrap(), ArithExpr::PreInc("a".to_string()));
        assert_eq!(parse("--a").unwrap(), ArithExpr::PreDec("a".to_string()));
        assert_eq!(parse("a++").unwrap(), ArithExpr::PostInc("a".to_string()));
        assert_eq!(parse("a--").unwrap(), ArithExpr::PostDec("a".to_string()));
    }
```

- [ ] **Step 2: Update the existing `--5` parser test**

Find the existing test at around line 510 (`parse_unary_double_minus`
or similar — search for `"--5"`):

```rust
    #[test]
    fn parse_unary_double_minus() {
        assert_eq!(parse("--5").unwrap(), ArithExpr::Neg(Box::new(ArithExpr::Neg(n(5)))));
    }
```

Replace with two tests reflecting the new semantics:

```rust
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
```

- [ ] **Step 3: Run the new tests + updated existing test to verify they fail**

Run: `cargo test --lib arith::tests::parse_ 2>&1 | tail -20`
Expected: ~10 new tests fail (parser doesn't yet handle the new
constructs). The `parse_double_minus_with_number_is_prefix_dec_error`
test may already pass (if the tokenizer emits `MinusMinus` followed
by `Number` and the parser doesn't have a `MinusMinus` prefix arm,
that's a parse error).

- [ ] **Step 4: Add an `assign_op_from_token` helper**

In `src/arith.rs`, near the `BinOpEntry` type alias (around line 186-188),
add:

```rust
/// Maps an assignment token to its corresponding AssignOp variant.
/// Returns None for non-assignment tokens.
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
```

- [ ] **Step 5: Rewrite `parse_expr` with the new precedence table**

In `src/arith.rs`, replace the entire `parse_expr` function (lines
201-242) with:

```rust
    fn parse_expr(&mut self, min_bp: u8) -> Result<ArithExpr, ArithError> {
        let mut lhs = self.parse_prefix()?;
        loop {
            let op = match self.peek().cloned() {
                Some(t) => t,
                None => break,
            };

            // 1. Postfix ++/-- (BP 27 — highest). Must come before infix
            //    handling so a++ + 1 parses as (a++) + 1.
            if matches!(op, ArithToken::PlusPlus | ArithToken::MinusMinus) && 27 >= min_bp {
                self.bump();
                let name = match lhs {
                    ArithExpr::Var(name) => name,
                    _ => return Err(ArithError::Parse(
                        "postfix ++/-- requires variable on LHS".to_string()
                    )),
                };
                lhs = match op {
                    ArithToken::PlusPlus => ArithExpr::PostInc(name),
                    _ => ArithExpr::PostDec(name),
                };
                continue;
            }

            // 2. Assignment (lbp = 2, rbp = 1 — right-associative).
            //    LHS must be a Var.
            if let Some(assign_op) = assign_op_from_token(&op) {
                if 2 < min_bp { break; }
                self.bump();
                let name = match lhs {
                    ArithExpr::Var(name) => name,
                    _ => return Err(ArithError::Parse(
                        "assignment requires variable on LHS".to_string()
                    )),
                };
                let rhs = self.parse_expr(1)?;  // rbp = 1 allows cascading assigns
                lhs = ArithExpr::Assign { name, op: assign_op, rhs: Box::new(rhs) };
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
```

- [ ] **Step 6: Update `parse_prefix` to handle new prefix operators**

In `src/arith.rs::parse_prefix` (lines 244-273), update the existing
unary `Minus` / `Plus` / `Bang` arms to use the new unary BP (26
instead of 14), and add three new arms for `Tilde`, `PlusPlus`, and
`MinusMinus`:

```rust
    fn parse_prefix(&mut self) -> Result<ArithExpr, ArithError> {
        match self.bump() {
            Some(ArithToken::Number(n)) => Ok(ArithExpr::Num(n)),
            Some(ArithToken::Ident(s)) => Ok(ArithExpr::Var(s)),
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
                // ++name: prefix increment requires identifier next.
                match self.bump() {
                    Some(ArithToken::Ident(name)) => Ok(ArithExpr::PreInc(name)),
                    _ => Err(ArithError::Parse(
                        "prefix ++ requires variable".to_string()
                    )),
                }
            }
            Some(ArithToken::MinusMinus) => {
                // --name: prefix decrement requires identifier next.
                match self.bump() {
                    Some(ArithToken::Ident(name)) => Ok(ArithExpr::PreDec(name)),
                    _ => Err(ArithError::Parse(
                        "prefix -- requires variable".to_string()
                    )),
                }
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
```

- [ ] **Step 7: Run the new parser tests to verify they pass**

Run: `cargo test --lib arith::tests::parse_ 2>&1 | tail -20`
Expected: all new parser tests pass + the updated `parse_double_minus_*`
tests pass.

- [ ] **Step 8: Run the full arith test module**

Run: `cargo test --lib arith::tests 2>&1 | tail -10`
Expected: all tests pass EXCEPT possibly `eval_unary_minus` which uses
`"--5"` and will break for the same reason. That gets fixed in Task 4.

If other tests fail unexpectedly, STOP and investigate.

- [ ] **Step 9: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean. After Task 3 wires the AST variants into the parser,
the `#[allow(dead_code)]` annotations from Task 2 may need to come off
(except for the evaluator-only variants — those go in Task 4).

If clippy reports dead-code on `BitAnd`/`BitOr`/etc. variants, those
are now constructed by the parser, so remove their annotations. If
clippy reports unused on `AssignOp` variants, similarly remove. The
exact dead-code state depends on Task 2's choices — adjust to keep
clippy clean.

- [ ] **Step 10: Commit**

```bash
git add src/arith.rs
git commit -m "$(cat <<'EOF'
arith: parser extension with renumbered precedence + new operators (v38 task 3)

Pratt precedence table renumbered: assignment at BP 2 (right-assoc),
ternary at BP 3, ||/&& at 4-7, bitwise |/^/& at 8-13, ==/!= at 14-15,
relational at 16-17, shift at 18-19, +/- at 20-21, * / / / % at 22-23,
** at 25 (right-assoc), unary prefix at BP 26, postfix ++/-- at BP 27.
Three special-cased loop branches: postfix inc/dec (top), assignment
dispatch via assign_op_from_token, power right-assoc. parse_prefix
gains Tilde (BitNot), PlusPlus (PreInc), MinusMinus (PreDec) arms;
existing unary arms updated to recurse at BP 26. LHS-must-be-Var
enforced at parse time for assignment and postfix ops. 8 new parser
unit tests; existing parse("--5") test updated to expect parse error
+ a new test using "- -5" for explicit double-negation.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Evaluator + signature change

**Files:**
- Modify: `src/arith.rs` (`eval` function at line 278; tests module helper at line 575; existing `eval_unary_minus` test at line 608)
- Modify: `src/expand.rs` (two call sites: lines 188 and 262)
- Modify: `src/param_expansion.rs` (one call site: line 136)

**Note for implementer:** The biggest task in v38 — touches 3 files,
changes `arith::eval`'s signature from `&Shell` to `&mut Shell`,
adds 12 new eval arms, and updates 3 call sites. The existing test
helper `eval_str(s: &str, shell: &Shell)` needs `&mut Shell` too.

- [ ] **Step 1: Change `arith::eval` signature to `&mut Shell`**

In `src/arith.rs::eval` (line 278), update the function signature:

```rust
pub fn eval(expr: &ArithExpr, shell: &mut Shell) -> Result<i64, ArithError>
```

All existing recursive `eval(a, shell)?` calls compile unchanged
because `&mut Shell` reborrows.

- [ ] **Step 2: Update 3 call sites**

In `src/expand.rs`, find line 188 (`match crate::arith::eval(expr, shell)`).
The `shell` variable is already `&mut Shell` in `expand`, so the call
already passes mutable. Confirm no change needed — the signature
change is transparent.

Same for `src/expand.rs:262` in `expand_assignment`.

Same for `src/param_expansion.rs:136` in `eval_arith_word`.

If any call site passes `&shell` or `shell.clone()` explicitly, fix
it to pass `shell` directly.

- [ ] **Step 3: Update the test helper signature**

In `src/arith.rs` tests module (around line 573-577), update:

```rust
    fn eval_str(s: &str, shell: &mut Shell) -> Result<i64, ArithError> {
        eval(&parse(s).unwrap(), shell)
    }
```

Then sweep the test module for ALL `let s = Shell::new();` (immutable)
and update them to `let mut s = Shell::new();`. The call sites change
from `eval_str("...", &s)` to `eval_str("...", &mut s)`.

There are many existing tests (perhaps 40+). Use a search-and-replace:
- `let s = Shell::new();` → `let mut s = Shell::new();`
- `&s)` → `&mut s)` (in the contexts of `eval_str` calls)

Be careful not to accidentally change unrelated `&s` (e.g. in other
test files or in production code). Limit the change to the
`src/arith.rs` tests module.

- [ ] **Step 4: Update the existing `eval_unary_minus` test (line 608)**

In the tests module, find the existing test:

```rust
    #[test]
    fn eval_unary_minus() {
        let s = Shell::new();
        assert_eq!(eval_str("-5", &s).unwrap(), -5);
        assert_eq!(eval_str("--5", &s).unwrap(), 5);
    }
```

Replace the `"--5"` assertion to use `"- -5"`:

```rust
    #[test]
    fn eval_unary_minus() {
        let mut s = Shell::new();
        assert_eq!(eval_str("-5", &mut s).unwrap(), -5);
        // v38: "--5" now parses as prefix-decrement on a literal → error.
        // Use "- -5" (with space) for explicit double-negation.
        assert_eq!(eval_str("- -5", &mut s).unwrap(), 5);
    }
```

- [ ] **Step 5: Add the read/write helpers**

In `src/arith.rs`, just above the `eval` function (around line 277),
add:

```rust
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
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) {
    shell.set(name, value.to_string());
}
```

- [ ] **Step 6: Write failing evaluator tests**

Append to the `src/arith.rs` tests module (after the existing eval_*
tests):

```rust
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
    fn eval_post_inc_returns_old_value() {
        let mut s = Shell::new();
        s.set("a", "5".to_string());
        assert_eq!(eval_str("a++", &mut s).unwrap(), 5);
        assert_eq!(s.lookup_var("a"), Some("6".to_string()));
    }
```

- [ ] **Step 7: Run tests to verify failure**

Run: `cargo test --lib arith::tests::eval_ 2>&1 | tail -20`
Expected: most new tests fail with "non-exhaustive patterns" or
"unreachable" because eval doesn't yet handle the new variants.

- [ ] **Step 8: Add the new eval arms**

In `src/arith.rs::eval`, find the existing match block. Add new arms
at the end (before the final `}`):

```rust
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
        ArithExpr::Assign { name, op, rhs } => {
            let rhs_val = eval(rhs, shell)?;
            let new_val = match op {
                AssignOp::Set    => rhs_val,
                AssignOp::Add    => read_var_i64(shell, name)?.wrapping_add(rhs_val),
                AssignOp::Sub    => read_var_i64(shell, name)?.wrapping_sub(rhs_val),
                AssignOp::Mul    => read_var_i64(shell, name)?.wrapping_mul(rhs_val),
                AssignOp::Div => {
                    let lhs = read_var_i64(shell, name)?;
                    if rhs_val == 0 { return Err(ArithError::DivisionByZero); }
                    lhs.wrapping_div(rhs_val)
                }
                AssignOp::Mod => {
                    let lhs = read_var_i64(shell, name)?;
                    if rhs_val == 0 { return Err(ArithError::ModuloByZero); }
                    lhs.wrapping_rem(rhs_val)
                }
                AssignOp::Shl => {
                    let lhs = read_var_i64(shell, name)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError::ShiftCountOutOfRange { count: rhs_val });
                    }
                    lhs.wrapping_shl(rhs_val as u32)
                }
                AssignOp::Shr => {
                    let lhs = read_var_i64(shell, name)?;
                    if !(0..64).contains(&rhs_val) {
                        return Err(ArithError::ShiftCountOutOfRange { count: rhs_val });
                    }
                    lhs.wrapping_shr(rhs_val as u32)
                }
                AssignOp::BitAnd => read_var_i64(shell, name)? & rhs_val,
                AssignOp::BitXor => read_var_i64(shell, name)? ^ rhs_val,
                AssignOp::BitOr  => read_var_i64(shell, name)? | rhs_val,
            };
            write_var_i64(shell, name, new_val);
            Ok(new_val)
        }
        ArithExpr::PreInc(name) => {
            let new_val = read_var_i64(shell, name)?.wrapping_add(1);
            write_var_i64(shell, name, new_val);
            Ok(new_val)
        }
        ArithExpr::PreDec(name) => {
            let new_val = read_var_i64(shell, name)?.wrapping_sub(1);
            write_var_i64(shell, name, new_val);
            Ok(new_val)
        }
        ArithExpr::PostInc(name) => {
            let old_val = read_var_i64(shell, name)?;
            write_var_i64(shell, name, old_val.wrapping_add(1));
            Ok(old_val)
        }
        ArithExpr::PostDec(name) => {
            let old_val = read_var_i64(shell, name)?;
            write_var_i64(shell, name, old_val.wrapping_sub(1));
            Ok(old_val)
        }
```

- [ ] **Step 9: Run the new evaluator tests**

Run: `cargo test --lib arith::tests::eval_ 2>&1 | tail -20`
Expected: all new tests pass.

- [ ] **Step 10: Remove any remaining `#[allow(dead_code)]` from Task 2**

Now that eval reads `AssignOp` and the new `ArithExpr` variants, any
`#[allow(dead_code)]` on those items should be removed.

- [ ] **Step 11: Run the full lib + integration test suite**

Run: `cargo test 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output (no failures).

- [ ] **Step 12: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 13: Commit**

```bash
git add src/arith.rs src/expand.rs src/param_expansion.rs
git commit -m "$(cat <<'EOF'
arith: evaluator extension + &mut Shell migration (v38 task 4)

eval signature changes from &Shell to &mut Shell to support
assignment and inc/dec. 3 call sites updated (src/expand.rs:188,
src/expand.rs:262, src/param_expansion.rs:136) — all already have
&mut Shell available. 12 new eval arms covering bitwise binops
(BitAnd/BitOr/BitXor/BitNot), shifts (Shl/Shr with range check),
power (Pow with negative-exponent check), Assign (all 11 AssignOp
variants), and PreInc/PreDec/PostInc/PostDec. read_var_i64 /
write_var_i64 helpers handle the read-modify-write pattern.
Wrapping arithmetic throughout (matches existing i64 wrap-on-overflow).
~16 new evaluator unit tests. Existing eval_unary_minus test
updated to use `- -5` (with space) instead of `--5` which is now
prefix-decrement.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Integration tests

**Files:**
- Create: `tests/arith_completion_integration.rs`

**Note for implementer:** Binary-driven tests via piped stdin. Same
harness pattern as v33/v34/v37 integration test files. Each test
spawns a fresh `huck` process.

- [ ] **Step 1: Create `tests/arith_completion_integration.rs`**

Create the file with:

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
fn arith_hex_literal() {
    let (out, _) = run("echo $((0x10))\nexit\n");
    assert!(out.lines().any(|l| l == "16"), "stdout: {out}");
}

#[test]
fn arith_octal_literal() {
    let (out, _) = run("echo $((010))\nexit\n");
    assert!(out.lines().any(|l| l == "8"), "stdout: {out}");
}

#[test]
fn arith_base_n_binary() {
    let (out, _) = run("echo $((2#1010))\nexit\n");
    assert!(out.lines().any(|l| l == "10"), "stdout: {out}");
}

#[test]
fn arith_mixed_bases() {
    // 0x10 (16) + 010 (8) + 2#10 (2) = 26
    let (out, _) = run("echo $((0x10 + 010 + 2#10))\nexit\n");
    assert!(out.lines().any(|l| l == "26"), "stdout: {out}");
}

#[test]
fn arith_bitwise_and() {
    let (out, _) = run("echo $((0xF0 & 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn arith_bitwise_or() {
    let (out, _) = run("echo $((0xF0 | 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "255"), "stdout: {out}");
}

#[test]
fn arith_bitwise_xor() {
    let (out, _) = run("echo $((0xFF ^ 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "240"), "stdout: {out}");
}

#[test]
fn arith_left_shift() {
    let (out, _) = run("echo $((1 << 8))\nexit\n");
    assert!(out.lines().any(|l| l == "256"), "stdout: {out}");
}

#[test]
fn arith_power() {
    let (out, _) = run("echo $((2 ** 10))\nexit\n");
    assert!(out.lines().any(|l| l == "1024"), "stdout: {out}");
}

#[test]
fn arith_assignment_persists_to_var() {
    // $((a = 5)) should print 5 AND set $a to 5.
    let (out, _) = run("echo $((a = 5))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().any(|l| **l == *"5"), "stdout: {out}");
    // Both 5's appear.
    let count = lines.iter().filter(|l| ***l == *"5").count();
    assert_eq!(count, 2, "expected two '5' lines, got {count}; stdout: {out}");
}

#[test]
fn arith_compound_assignment() {
    let (out, _) = run("a=3\necho $((a += 4))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let sevens = lines.iter().filter(|l| ***l == *"7").count();
    assert_eq!(sevens, 2, "expected two '7' lines; stdout: {out}");
}

#[test]
fn arith_post_increment_in_expression() {
    // a=5; $((a++ + 1)) = old(5) + 1 = 6; then $a = 6.
    let (out, _) = run("a=5\necho $((a++ + 1))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let sixes = lines.iter().filter(|l| ***l == *"6").count();
    assert_eq!(sixes, 2, "expected two '6' lines; stdout: {out}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test arith_completion_integration 2>&1 | tail -10`
Expected: all 12 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/arith_completion_integration.rs
git commit -m "$(cat <<'EOF'
test: arithmetic completion integration coverage (v38 task 5)

12 binary-driven tests covering all v38 forms: hex/octal/base-N
literals, bitwise & | ^ <<, power **, assignment, compound assign
(+=), and post-increment in an expression with variable persistence.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Docs + verify

**Files:**
- Modify: `docs/bash-divergences.md` (M-55, M-56, M-57 entries + changelog)
- Modify: `README.md` (v38 row)

- [ ] **Step 1: Mark M-55 fixed v38**

Find the M-55 entry in `docs/bash-divergences.md` (around line 164):

```markdown
- **M-55: Bitwise operators `&`/`|`/`^`/`~`/`<<`/`>>`** — `[deferred]` high. huck: parse error. bash: full bitwise.
```

Replace with:

```markdown
- **M-55: Bitwise operators `&`/`|`/`^`/`~`/`<<`/`>>`** — `[fixed v38]` high. All six bitwise operators supported in `$((…))`. Shift counts must be in `[0, 64)` — out-of-range produces `ShiftCountOutOfRange` error (deliberate divergence from bash's C-undefined behavior). Bundled with `**` exponentiation (right-associative; negative exponents error). v38 also closes M-56 and M-57.
```

- [ ] **Step 2: Mark M-56 fixed v38**

Find the M-56 entry (around line 165):

```markdown
- **M-56: Assignment operators `=`/`+=`/`-=`/`*=`/`/=`/`%=`/`++`/`--`** — `[deferred]` medium. huck: bare `=` errors; `++`/`--` silently parse as double unary. bash: assignment mutates the shell var.
```

Replace with:

```markdown
- **M-56: Assignment operators `=`/`+=`/`-=`/`*=`/`/=`/`%=`/`<<=`/`>>=`/`&=`/`^=`/`\|=`/`++`/`--`** — `[fixed v38]` medium. All 11 assignment operators + prefix/postfix `++`/`--` supported. `arith::eval` signature now takes `&mut Shell`. LHS must be a variable (enforced at parse time): `(a + b) = 5` rejects with parse error. v38 also closes M-55 and M-57.
```

- [ ] **Step 3: Mark M-57 fixed v38**

Find the M-57 entry (around line 166):

```markdown
- **M-57: Non-decimal literals (`0x…`, `0…`, `N#…`)** — `[deferred]` medium. huck: hex/octal/base# all rejected. bash: full numeric base support.
```

Replace with:

```markdown
- **M-57: Non-decimal literals (`0x…`, `0…`, `N#…`)** — `[fixed v38]` medium. Hex (`0x…` / `0X…`), octal (`0…` with digits 0-7), and base-N (`N#…` for 2 ≤ N ≤ 64) all supported. Bash's full digit alphabet (0-9, a-z, A-Z, @, _) implemented; for bases ≤ 36 letters are case-insensitive, for bases > 36 they're distinct. v38 also closes M-55 and M-56.
```

- [ ] **Step 4: Add a changelog row**

At the bottom of the `## Change log` section, append:

```markdown
- **2026-05-28**: M-55 (bitwise operators), M-56 (assignment + inc/dec), and M-57 (non-decimal literals) shipped together as v38 — closes the arithmetic-feature cluster started by v22. Bundled `**` exponentiation. `arith::eval` signature changed from `&Shell` to `&mut Shell` (3 call sites updated). Pratt-parser precedence table renumbered to match bash's documented order. Shift counts out of `[0, 64)` produce explicit errors (deliberate divergence from bash's C-undefined behavior).
```

- [ ] **Step 5: Update the README**

Find the v37 row in `README.md`'s version table. Add a new row AFTER it:

```markdown
| v38       | Arithmetic completion (M-55 + M-56 + M-57 + `**`)              |
```

Match column alignment with the surrounding rows.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test 2>&1 | grep -E "^test result" | tail -30`
Expected: all suites pass. New baseline ~1476 (1426 from v37 +
~50 new across Tasks 1/3/4/5).

If PTY suite shows its v29-era flake, re-run in isolation; pre-existing.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 8: Confirm working tree clean**

Run: `git status`
Expected: clean (after the commit in Step 9).

- [ ] **Step 9: Commit the docs**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-55 + M-56 + M-57 fixed v38; v38 in README

Closes the arithmetic-feature cluster started by v22. M-55 (bitwise),
M-56 (assignment + inc/dec), M-57 (non-decimal literals), and bundled
** all shipped together. Changelog entry documents the &mut Shell
migration and Pratt-parser precedence renumbering.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Hand back to the parent session for the final code-reviewer dispatch + merge to main.**
