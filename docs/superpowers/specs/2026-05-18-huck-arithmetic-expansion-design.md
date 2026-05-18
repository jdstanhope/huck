# huck v11: Arithmetic Expansion

**Date:** 2026-05-18
**Status:** Design

## Goal

Add `$((expr))` arithmetic expansion to huck. The inner expression
evaluates to a signed 64-bit integer using the core C-style operator
set; the result is substituted into the surrounding word as decimal
text. Expressions can reference shell variables by name.

## Scope

**In scope:**
- Integer literals (decimal only): `0`, `42`, `-5`
- Arithmetic operators: `+`, `-` (binary and unary), `*`, `/`, `%`
- Comparison operators: `==`, `!=`, `<`, `<=`, `>`, `>=`
- Logical operators: `&&`, `||`, `!` (short-circuit for `&&` / `||`)
- Ternary: `cond ? then : else` (right-associative)
- Parenthesized sub-expressions
- Variable references: bare `name` or `$name` (treated identically)
- i64 with wrapping overflow (matches bash)
- Errors print to stderr, expansion yields empty string, `$?` set to 1
- Available anywhere a `$VAR` can appear: command args, redirect
  targets, assignment RHS, inside double quotes

**Out of scope (deferred):**
- Bitwise operators: `&`, `|`, `^`, `~`, `<<`, `>>`
- Assignment operators inside expressions: `=`, `+=`, `-=`, etc.
- Pre/post increment: `++var`, `var--`
- Comma operator
- Exponentiation: `**`
- Numeric base prefixes: `0x...` (hex), `0...` (octal), `base#N`
- Recursive variable evaluation (bash re-evaluates a variable's
  value as an arithmetic expression; we treat values as integer
  literals only)
- Floating point

## Architecture

A new `src/arith.rs` module owns the AST, the parser, the evaluator,
and the error type. The lexer in `src/lexer.rs` detects `$(( ... ))`
at token time, scans the inner text with paren-depth tracking,
hands the inner string to `arith::parse`, and stores the resulting
`ArithExpr` in a new `WordPart::Arith` variant. At expand time,
`arith::eval(&ArithExpr, &Shell)` returns the integer; the rendered
text becomes part of the surrounding `Field`.

### AST and tokens

```rust
pub enum ArithExpr {
    Num(i64),
    Var(String),
    Neg(Box<ArithExpr>),    // unary -
    Not(Box<ArithExpr>),    // unary !
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
    And(Box<ArithExpr>, Box<ArithExpr>),   // short-circuit &&
    Or(Box<ArithExpr>, Box<ArithExpr>),    // short-circuit ||
    Ternary(Box<ArithExpr>, Box<ArithExpr>, Box<ArithExpr>),
}
```

Inner tokens (used by the arith parser only):

```rust
enum ArithToken {
    Number(i64),
    Ident(String),
    LParen, RParen,
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    AndAnd, OrOr, Bang,
    Question, Colon,
}
```

Whitespace between tokens is skipped. Inside the arith parser, `$`
preceding an identifier is silently consumed (so `$x` and `x` tokenize
identically to `Ident("x")`).

### Parser

Pratt (precedence-climbing) recursive descent. Operator precedence,
lowest to highest (matches C and bash):

| Level | Operators                        | Associativity |
|-------|----------------------------------|---------------|
| 1     | `?:`                             | right         |
| 2     | `\|\|`                           | left          |
| 3     | `&&`                             | left          |
| 4     | `==`, `!=`                       | left          |
| 5     | `<`, `<=`, `>`, `>=`             | left          |
| 6     | `+`, `-` (binary)                | left          |
| 7     | `*`, `/`, `%`                    | left          |
| 8     | `-`, `!` (unary)                 | right         |
| 9     | primary: Number, Ident, `(expr)` |               |

Public entry: `pub fn parse(input: &str) -> Result<ArithExpr, ArithError>`.

### Evaluator

`pub fn eval(expr: &ArithExpr, shell: &Shell) -> Result<i64, ArithError>`.

- `Num(n)` → `n`.
- `Var(name)` → `shell.get(name)`. Unset or empty → `0`. Otherwise
  parse the string as `i64` (decimal); on parse failure return
  `NotAnInteger { var, value }`. (Note: no recursive evaluation —
  the value must be a plain integer literal.)
- All arithmetic uses `wrapping_add`, `wrapping_sub`, `wrapping_mul`,
  `wrapping_neg`, and `wrapping_div`/`wrapping_rem` for `/`/`%`.
- `Div` and `Mod` with RHS == 0 return `DivisionByZero` / `ModuloByZero`.
- Comparison ops return `1` for true, `0` for false (bash semantics).
- `And` / `Or` short-circuit: if LHS determines result, RHS is not
  evaluated (so `0 && (1/0)` yields `0` without erroring).
- `Not(e)` → `1` if `e` is `0`, else `0`.
- `Ternary(c, a, b)` → evaluate `c`; if non-zero return `a`, else `b`.

### Error type

```rust
pub enum ArithError {
    Parse(String),                                // includes position
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
```

### Lexer integration

In `src/lexer.rs`, `read_dollar_expansion` (around line 266) currently
dispatches on the character after `$`. Add a new branch **before**
the existing `$(` case: if the next two chars are `((`, consume them
and scan the inner text.

Scan procedure: walk the input tracking paren depth (initially 1,
since we have consumed the opening `((`). On each `(` increment depth;
on each `)` decrement. When depth hits 0, the next char must be `)`
(closing the outer `))`); consume both and stop. If EOF arrives before
the closing pair, return `LexError::UnterminatedArith`.

After scanning, call `arith::parse(inner)`. On `Ok(expr)` push
`WordPart::Arith { expr, quoted }`. On `Err(e)` return
`LexError::ArithParse(e.to_string())`.

`quoted` reflects whether the `$((..))` appeared inside `"..."`,
mirroring the other expansion WordParts.

### New WordPart variant

```rust
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: Sequence, quoted: bool },
    Arith { expr: crate::arith::ArithExpr, quoted: bool },
}
```

### Expand integration

In `src/expand.rs`, add a new match arm in the per-WordPart loop:

```rust
WordPart::Arith { expr, quoted: _ } => {
    match arith::eval(expr, shell) {
        Ok(n) => {
            let s = n.to_string();
            current.push_str(&s, true);
        }
        Err(e) => {
            eprintln!("huck: arithmetic: {}", e);
            shell.set_last_status(1);
            // Append nothing — keep field empty if no other parts.
        }
    }
}
```

The rendered integer text always uses `quoted: true` in the `Field`
(no word-splitting on integer text; the `quoted` flag is moot for
content like `"-5"` that has no whitespace, but treating it as quoted
matches the rule for `Var { quoted: true }`).

`expand_assignment` gets the same arm (assignment RHS doesn't
word-split, so the only difference from `expand` is the absence of
IFS handling — irrelevant to Arith).

### Pattern-match sites

Code that matches on `WordPart` (today: `src/expand.rs`,
`src/command.rs`, `src/executor.rs`, lexer tests) gets a new `Arith`
arm. Specifically:

- `command.rs::word_is_identifier_so_far`: an `Arith` WordPart
  disqualifies a word from being an assignment-prefix identifier
  (so `$((x))` at the start of a word is treated as a regular
  argument, not as part of `FOO=...`). Function returns `false` for
  any non-Literal part (already the case for `Tilde`/`Var`/etc.).
- `command.rs::try_split_assignment`: only triggers when the first
  WordPart is a `Literal { quoted: false }` containing `=`. Arith
  parts don't affect this path.
- `executor.rs::pipeline_is_pure_builtin`: existing matcher already
  uses `if let WordPart::Literal { text, .. } = p` for program-name
  inspection; Arith parts naturally fail the match and the function
  conservatively returns false (allowing fork) — which is correct for
  a word like `$((1+1))` as a program name.

## Data flow examples

`echo $((1+2*3))`:

1. Lex: `echo` Word with one Literal; `$((1+2*3))` Word with one
   Arith part (AST: `Add(Num(1), Mul(Num(2), Num(3)))`).
2. Expand `$((1+2*3))`: eval → 7; render → "7" with `quoted: true`.
3. Field: `chars = "7"`, `quoted = [true]`.
4. glob_expand_fields: no metachars → "7".
5. Argv: `["echo", "7"]`.

`x=5; echo $((x*2 + 1))`:

1. Assignment sets `x` to `"5"`.
2. Lex `$((x*2 + 1))` → Arith AST `Add(Mul(Var("x"), Num(2)), Num(1))`.
3. Eval: lookup x = "5" → 5; 5*2+1 = 11. Render "11".
4. Argv: `["echo", "11"]`.

`echo $((1/0))`:

1. Lex: Arith AST `Div(Num(1), Num(0))`.
2. Eval returns `Err(DivisionByZero)`.
3. Expand: stderr `huck: arithmetic: division by zero`; last_status = 1;
   field stays empty.
4. Argv: `["echo", ""]` (since echo's word had only the Arith part,
   the field is one empty string).
5. `echo` runs with an empty argument and exits 0. `$?` after the
   command runs reflects echo's exit (0), NOT the arithmetic error
   — bash behaves the same way here. The last_status set during
   expansion is overwritten by the command's exit status.

`FOO=$((2+2)); echo $FOO`:

1. Assignment: `expand_assignment` evaluates the Arith part to "4".
   `FOO` is set to "4".
2. `echo $FOO` expands to `echo 4`.

`"prefix $((1+2)) suffix"`:

1. Lex: one Word with three parts:
   - `Literal { text: "prefix ", quoted: true }`
   - `Arith { expr: Add(Num(1), Num(2)), quoted: true }`
   - `Literal { text: " suffix", quoted: true }`
2. Expand: field chars = "prefix 3 suffix", all chars `quoted: true`.
3. Argv: `["prefix 3 suffix"]` (one element).

## Error handling summary

| Condition                       | Behavior                                                            |
|---------------------------------|---------------------------------------------------------------------|
| Parse error (e.g. `1+`)         | `LexError::ArithParse` at command parse time; command does not run  |
| Unterminated `$((`              | `LexError::UnterminatedArith` at command parse time                 |
| Division by zero                | Stderr `huck: arithmetic: division by zero`; field empty; status=1  |
| Modulo by zero                  | Stderr `huck: arithmetic: modulo by zero`; field empty; status=1    |
| Variable not integer            | Stderr `huck: arithmetic: variable 'X' is not an integer: 'abc'`    |
| Overflow                        | Silent wrap (matches bash)                                          |

## Testing

**`src/arith.rs` unit tests:**
- Number parsing: `"42"` → `Num(42)`, `"0"` → `Num(0)`, leading whitespace tolerated
- Each operator: smoke test parse + eval
- Precedence: `1+2*3` → 7, `(1+2)*3` → 9, `1<2 && 3<4` → 1
- Associativity: `1-2-3` → -4 (left), `a?b:c?d:e` → right (parses as `a?b:(c?d:e)`)
- Unary: `-5` → -5, `--5` → 5, `!0` → 1, `!5` → 0
- Wrapping: `i64::MAX + 1` → `i64::MIN`
- Short-circuit: `0 && X` does not evaluate X (use a div-by-zero RHS)
- Variables: set, unset, empty, non-integer cases
- Errors: each variant constructible and Display-correct
- Whitespace tolerance: `"  1 + 2 "` parses

**`src/lexer.rs` tests:**
- `$((1+2))` → one Arith WordPart with expected AST
- `$(( (1+2) * 3 ))` → nested parens scanned correctly
- `$((1+2` <EOF> → `LexError::UnterminatedArith`
- `$((1+))` → `LexError::ArithParse`
- `$((x))` and `$(($x))` produce identical AST
- `"prefix $((1+2)) suffix"` → three parts, Arith with `quoted: true`
- Back-to-back: `$((1+2))$((3+4))`

**`src/expand.rs` tests:**
- `$((1+2))` → one Field with `chars = "3"`, all `quoted: true`
- `echo $((-5))` argv → `["echo", "-5"]`
- Error path: `$((1/0))` produces empty Field and sets `last_status = 1`
- Assignment: `FOO=$((2+2))` via `expand_assignment` returns `"4"`

**Integration tests (`tests/arith_integration.rs`):**
- `echo $((2+3*4))` → stdout `14`
- `x=5; echo $((x*2))` → `10`
- `FOO=$((1+1)); echo $FOO` → `2`
- `echo $((1/0))` → stderr contains "division by zero"

## File layout impact

- **New:** `src/arith.rs` (~400 lines including tests)
- **Modify:** `src/lexer.rs` — add `$((` scanning in `read_dollar_expansion`,
  add `Arith` variant to `WordPart`, add `UnterminatedArith` and
  `ArithParse` to `LexError`, update tests
- **Modify:** `src/expand.rs` — new `Arith` match arm in both `expand`
  and `expand_assignment`, new tests
- **Modify:** `src/command.rs` — `Arith` arm in any matcher
  (`word_is_identifier_so_far` returns false for non-Literal already,
  so a single `_` arm covers it; verify and add explicit arm if needed
  for clarity)
- **Modify:** `src/executor.rs` — `Arith` arm in any matcher
  (e.g. `pipeline_is_pure_builtin` if it uses `WordPart::Literal`
  patterns)
- **New:** `tests/arith_integration.rs`
- **Modify:** `Cargo.toml` — no new dependencies
- **Modify:** `README.md` — v11 row, features section, test count

## Open questions

None at design time.

## References

- POSIX 2008 Shell Command Language §2.6.4 Arithmetic Expansion
- bash(1) Arithmetic Evaluation section
- C99 §6.5 Expressions (operator precedence reference)
