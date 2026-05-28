# huck v38 — Arithmetic Completion (M-55 + M-56 + M-57 + `**`) Design

**Goal:** Close out the arithmetic-feature cluster started by v22 (basic
`$((…))`). v38 reaches bash-arith feature parity by adding:

- **M-55**: bitwise operators `&`, `|`, `^`, `~`, `<<`, `>>` inside `$((…))`.
- **M-56**: assignment operators `=`, `+=`, `-=`, `*=`, `/=`, `%=`, `<<=`,
  `>>=`, `&=`, `^=`, `|=`, plus prefix/postfix `++` and `--`.
- **M-57**: non-decimal literals — hex (`0x…`), octal (`0…`), and base-N
  (`N#…` for 2 ≤ N ≤ 64).
- **`**`** exponentiation operator (bundled — not in M-55's list but a
  natural arith-completion gap).

**Why:** Three high-/medium-impact `[deferred]` items that all live in
`src/arith.rs`. Bundling them into one iteration closes the whole arith
feature group with a coherent set of changes to the tokenizer, AST,
parser, and evaluator. After v38, every operator bash's `$((…))`
supports works in huck (modulo the deliberate scope-outs below).

## Forms

### M-55 — bitwise operators

| Operator | Meaning | Example |
|---|---|---|
| `a & b` | bitwise AND | `0xF0 & 0x0F` → 0 |
| `a \| b` | bitwise OR | `0xF0 \| 0x0F` → 255 |
| `a ^ b` | bitwise XOR | `0xFF ^ 0x0F` → 240 |
| `~a` | bitwise NOT (prefix) | `~0` → -1 |
| `a << b` | left shift | `1 << 8` → 256 |
| `a >> b` | arithmetic right shift | `(-8) >> 1` → -4 |

### `**` exponentiation

| Operator | Meaning | Example |
|---|---|---|
| `a ** b` | power; right-associative | `2 ** 10` → 1024 |

### M-56 — assignment and inc/dec

| Operator | Meaning |
|---|---|
| `a = expr` | set `a` to expr |
| `a += expr` | `a = a + expr` |
| `a -= expr` | `a = a - expr` |
| `a *= expr` | `a = a * expr` |
| `a /= expr` | `a = a / expr` |
| `a %= expr` | `a = a % expr` |
| `a <<= expr` | `a = a << expr` |
| `a >>= expr` | `a = a >> expr` |
| `a &= expr` | `a = a & expr` |
| `a ^= expr` | `a = a ^ expr` |
| `a \|= expr` | `a = a \| expr` |
| `++a` | prefix increment: increment then return new value |
| `--a` | prefix decrement: decrement then return new value |
| `a++` | postfix increment: return old value then increment |
| `a--` | postfix decrement: return old value then decrement |

Each assignment / inc / dec writes the variable back to the shell via
`shell.set(name, value.to_string())`. The result of the expression is
the new value (for `=`/compound/pre-inc/pre-dec) or the old value (for
post-inc/post-dec).

### M-57 — non-decimal literals

| Literal | Base | Example |
|---|---|---|
| `0x…` / `0X…` | 16 | `0x10` = 16, `0XFF` = 255 |
| `0…` (≥2 digits, all 0-7) | 8 | `010` = 8, `0777` = 511 |
| `N#…` (2 ≤ N ≤ 64) | N | `2#1010` = 10, `16#FF` = 255, `64#1Az_` = … |
| (anything else) | 10 | `42` = 42, `0` = 0 |

**Base-N digit alphabet** (matches bash):
- `0`-`9` → 0-9
- `a`-`z` → 10-35
- `A`-`Z` → 36-61
- `@` → 62
- `_` → 63

For bases ≤ 36, both `a`-`z` and `A`-`Z` are accepted as 10-35 (case-
insensitive). For bases > 36, the cases are distinct. Invalid digits
for the chosen base produce a parse error.

## Semantics

### Shift semantics

- Left shift `a << b` and arithmetic right shift `a >> b` use Rust's
  `wrapping_shl(b as u32)` / `wrapping_shr(b as u32)`.
- Shift count `b` must be in `[0, 64)`. Out-of-range counts produce
  `ArithError::ShiftCountOutOfRange { count }`. This is a deliberate
  divergence from bash (which exposes C-undefined behavior for
  out-of-range shifts); we choose explicit errors for predictability.

### Power semantics

- `a ** b` evaluates as `base.wrapping_pow(exp as u32)`.
- Negative exponents produce `ArithError::NegativeExponent` (matches
  bash's "math: negative exponent" error).
- Overflow wraps (uses `wrapping_pow`, matches bash's wrap-on-overflow
  behavior for the i64 integer arith).

### Assignment LHS requirement

- The LHS of every assignment (`=`/compound) and every inc/dec must be
  a single variable name (`ArithExpr::Var`). Expressions like
  `(a + b) = 5` are rejected at parse time with
  `ArithError::Parse("assignment requires variable on LHS")`.
- The AST enforces this structurally: `ArithExpr::Assign { name: String,
  op, rhs }` and `ArithExpr::PreInc(String)` / `PreDec` / `PostInc` /
  `PostDec` all carry a `String` (the variable name), not a sub-expression.

### Read-modify-write

- `read_var_i64(shell, name)`: looks up via `shell.lookup_var(name)`,
  unwraps to `""` for unset, then parses as `i64`. Empty string → `0`.
  Non-integer string → `ArithError::NotAnInteger { var, value }`.
- `write_var_i64(shell, name, value)`: calls `shell.set(name,
  value.to_string())`.

### Operator precedence (lowest to highest)

| Level | Operators | Associativity |
|---|---|---|
| 1 | `=` `+=` `-=` `*=` `/=` `%=` `<<=` `>>=` `&=` `^=` `\|=` | right |
| 2 | `?:` ternary | right |
| 3 | `\|\|` | left |
| 4 | `&&` | left |
| 5 | `\|` (bitwise OR) | left |
| 6 | `^` (bitwise XOR) | left |
| 7 | `&` (bitwise AND) | left |
| 8 | `==` `!=` | left |
| 9 | `<` `<=` `>` `>=` | left |
| 10 | `<<` `>>` | left |
| 11 | `+` `-` (binary) | left |
| 12 | `*` `/` `%` | left |
| 13 | `**` | right |
| 14 | `-` `+` `!` `~` (unary), prefix `++` `--` | — |
| 15 | postfix `++` `--` | — |

Matches bash's documented precedence with one exception: bash documents
postfix inc/dec as the highest level; this design places them at level
15 (above unary) which is functionally equivalent for the bash
grammar's actual reachable parses.

### `arith::eval` signature change

The existing signature `pub fn eval(expr: &ArithExpr, shell: &Shell)
-> Result<i64, ArithError>` changes to `&mut Shell` because assignment
and inc/dec mutate variables. Both call sites
(`src/expand.rs::WordPart::Arith` handling around line 188, and
`src/param_expansion.rs::eval_arith_word` from v33) already have
`&mut Shell` available — the migration is a single-character change at
each call site.

## Tokenizer

### New `ArithToken` variants

```rust
pub(crate) enum ArithToken {
    // Existing (v22):
    Number(i64), Ident(String),
    LParen, RParen,
    Plus, Minus, Star, Slash, Percent,
    Eq, Ne, Lt, Le, Gt, Ge,
    AndAnd, OrOr, Bang,
    Question, Colon,
    // New (v38):
    Amp, Pipe, Caret, Tilde,       // & | ^ ~
    Shl, Shr,                       // << >>
    Power,                          // **
    Assign,                         // =
    PlusEq, MinusEq, StarEq,
    SlashEq, PercentEq,             // += -= *= /= %=
    ShlEq, ShrEq,                   // <<= >>=
    AmpEq, CaretEq, PipeEq,         // &= ^= |=
    PlusPlus, MinusMinus,           // ++ --
}
```

### Tokenizer arm rewrites

The existing single-char arms need extended peek logic. The table
below shows the new behavior per character:

| Char | Outcome by next-char peek |
|---|---|
| `+` | `++` → `PlusPlus`; `+=` → `PlusEq`; else `Plus` |
| `-` | `--` → `MinusMinus`; `-=` → `MinusEq`; else `Minus` |
| `*` | `**` → `Power`; `*=` → `StarEq`; else `Star` |
| `/` | `/=` → `SlashEq`; else `Slash` |
| `%` | `%=` → `PercentEq`; else `Percent` |
| `=` | `==` → `Eq`; else `Assign` (was: error) |
| `<` | `<<=` → `ShlEq`; `<<` → `Shl`; `<=` → `Le`; else `Lt` |
| `>` | `>>=` → `ShrEq`; `>>` → `Shr`; `>=` → `Ge`; else `Gt` |
| `&` | `&&` → `AndAnd`; `&=` → `AmpEq`; else `Amp` (was: error) |
| `\|` | `\|\|` → `OrOr`; `\|=` → `PipeEq`; else `Pipe` (was: error) |
| `^` | `^=` → `CaretEq`; else `Caret` (new arm) |
| `~` | `Tilde` (new arm) |
| `!` | `!=` → `Ne`; else `Bang` (unchanged) |
| `(` | `LParen` |
| `)` | `RParen` |
| `?` | `Question` |
| `:` | `Colon` |

Existing error messages on `=`, `&`, `|` disappear; those tokens now
have legal meanings.

### Numeric literal parsing

Replaces the existing digit-only arm at lines 20-28:

```rust
'0'..='9' => {
    // Read greedy leading decimal digits.
    let mut digits = String::new();
    while let Some(&d) = chars.peek() {
        if d.is_ascii_digit() { digits.push(d); chars.next(); } else { break; }
    }
    let n: i64 = if chars.peek() == Some(&'#') {
        // Base-N: `N#digits`. Leading `digits` is the base in decimal.
        chars.next();
        let base: u32 = digits.parse()
            .map_err(|_| ArithError::Parse(format!("invalid base: {digits}")))?;
        if !(2..=64).contains(&base) {
            return Err(ArithError::Parse(format!("base must be 2-64, got {base}")));
        }
        parse_base_n_digits(&mut chars, base)?
    } else if digits == "0" && matches!(chars.peek(), Some('x') | Some('X')) {
        // Hex: 0x... / 0X...
        chars.next();
        parse_hex_digits(&mut chars)?
    } else if digits.len() > 1 && digits.starts_with('0') {
        // Octal: 010 → 8. All digits must be 0-7.
        i64::from_str_radix(&digits, 8)
            .map_err(|_| ArithError::Parse(format!("invalid octal literal: {digits}")))?
    } else {
        digits.parse()
            .map_err(|_| ArithError::Parse(format!("integer literal out of range: {digits}")))?
    };
    out.push(ArithToken::Number(n));
}
```

Two helper functions added in `src/arith.rs`:

```rust
/// Parses hex digits 0-9, a-f, A-F. Returns i64 value.
fn parse_hex_digits(chars: &mut Peekable<Chars<'_>>) -> Result<i64, ArithError>;

/// Parses base-N digits per bash's alphabet (0-9, a-z, A-Z, @, _).
/// `base` must be in [2, 64].
fn parse_base_n_digits(chars: &mut Peekable<Chars<'_>>, base: u32) -> Result<i64, ArithError>;
```

## AST

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithExpr {
    // Existing (v22):
    Num(i64), Var(String),
    Neg(Box<ArithExpr>), Not(Box<ArithExpr>),
    Add(Box, Box), Sub(Box, Box), Mul(Box, Box), Div(Box, Box), Mod(Box, Box),
    Eq(Box, Box), Ne(Box, Box), Lt(Box, Box), Le(Box, Box), Gt(Box, Box), Ge(Box, Box),
    And(Box, Box), Or(Box, Box),
    Ternary(Box, Box, Box),
    // New (v38):
    BitAnd(Box, Box), BitOr(Box, Box), BitXor(Box, Box), BitNot(Box),
    Shl(Box, Box), Shr(Box, Box),
    Pow(Box, Box),
    Assign { name: String, op: AssignOp, rhs: Box<ArithExpr> },
    PreInc(String), PreDec(String),
    PostInc(String), PostDec(String),
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
```

**Total new variants**: 6 bitwise + 1 power + 1 Assign + 4 inc/dec = 12
+ 1 `AssignOp` sub-enum.

The `Assign { name: String, ... }` representation (and the four
inc/dec variants carrying `String` directly) enforces "LHS must be a
variable" at the AST level — the parser cannot construct an
ill-formed `Assign` node.

## `ArithError` extensions

```rust
pub enum ArithError {
    // Existing: Parse, DivisionByZero, ModuloByZero, NotAnInteger
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
}
```

Display impl gains:

```rust
NegativeExponent => write!(f, "exponentiation with negative exponent"),
ShiftCountOutOfRange { count } => write!(f, "shift count out of range: {count}"),
```

## Parser

The `parse_expr` Pratt loop is rewritten with the renumbered precedence
table (see Semantics §"Operator precedence"). Three special-cased
constructs:

1. **Postfix `++`/`--`** — handled at the top of the loop (BP 15);
   requires `lhs` to be `ArithExpr::Var(name)`. Constructs
   `PostInc(name)` / `PostDec(name)`.

2. **Assignment** — handled before the standard BP table; matches any
   assignment token via a helper `assign_op_from_token`. Right-assoc
   (rbp = 1, lbp = 2). Requires `lhs` to be `ArithExpr::Var(name)`.

3. **Power `**`** — handled as a special case with right-assoc BPs
   (lbp = 25, rbp = 24).

`parse_prefix` gains arms for `Tilde` (→ `BitNot`), `PlusPlus` (→
`PreInc(name)` after consuming the following `Ident`), and
`MinusMinus` (→ `PreDec(name)`). Existing unary `Minus`/`Plus`/`Bang`
arms update their recursion BP from 14 → 26 (the renumbered unary
level).

## Evaluator

### Signature

```rust
pub fn eval(expr: &ArithExpr, shell: &mut Shell) -> Result<i64, ArithError>
```

### Call-site updates

- `src/expand.rs:188` — the `WordPart::Arith` handler passes `shell`
  (which is already `&mut Shell` in `expand`).
- `src/param_expansion.rs::eval_arith_word` — passes its `shell: &mut
  Shell` parameter through.

Both call sites need the literal `&shell` → `shell` change (or
equivalent) — verify exact form at implementation time.

### Helpers

```rust
fn read_var_i64(shell: &Shell, name: &str) -> Result<i64, ArithError> {
    let raw = shell.lookup_var(name).unwrap_or_default();
    if raw.is_empty() { return Ok(0); }
    raw.parse::<i64>().map_err(|_| ArithError::NotAnInteger {
        var: name.to_string(),
        value: raw,
    })
}

fn write_var_i64(shell: &mut Shell, name: &str, value: i64) {
    shell.set(name, value.to_string());
}
```

### New eval arms

- **Bitwise binops** (BitAnd/BitOr/BitXor): `eval(a, shell)? <op> eval(b, shell)?`.
- **BitNot**: `!eval(e, shell)?`.
- **Shl/Shr**: check shift count is in `[0, 64)`; use
  `wrapping_shl/wrapping_shr(count as u32)`.
- **Pow**: check exponent is non-negative; use `base.wrapping_pow(exp
  as u32)`.
- **Assign**: read existing if compound (via `read_var_i64`), compute
  new value per `AssignOp`, write via `write_var_i64`, return new value.
- **PreInc/PreDec**: read, mutate, write, return new value.
- **PostInc/PostDec**: read, write modified, return old value.

## Error handling

| Condition | Behavior |
|---|---|
| `(a+b) = 5` (assignment LHS not Var) | `ArithError::Parse("assignment requires variable on LHS")` |
| `(a+b)++` (postfix LHS not Var) | `ArithError::Parse("postfix ++/-- requires variable")` |
| `2 ** -1` (negative exponent) | `ArithError::NegativeExponent` |
| `1 << 64` (shift count out of range) | `ArithError::ShiftCountOutOfRange { count: 64 }` |
| `a /= 0` (assignment division by zero) | `ArithError::DivisionByZero` |
| `08` (invalid octal digit) | `ArithError::Parse("invalid octal literal: 08")` |
| `1#0` or `65#0` (invalid base) | `ArithError::Parse("base must be 2-64, got 1")` |
| `8#9` (invalid digit for base) | `ArithError::Parse("invalid digit for base 8: '9'")` |

All errors propagate through `expand_modifier`'s existing
`arith::eval` callers as the standard `huck: arithmetic: <msg>` +
`$? = 1` path.

## Scope (in)

- All M-55 bitwise operators (`&`, `|`, `^`, `~`, `<<`, `>>`).
- All M-56 assignment operators including bitwise-assigns and inc/dec
  (pre/post).
- All M-57 non-decimal literal forms (hex, octal, N#... 2-64).
- `**` exponentiation (right-associative).
- `arith::eval` signature change to `&mut Shell` + the two call-site
  updates.
- Renumbered Pratt precedence table matching bash's documented order.

## Scope (out)

- **Comma operator** (`,` arith-sequence) — bash supports `(( a=1, b=2 ))`
  but it's rare. Defer to a future iteration if needed.
- **Float / fixed-point arithmetic** — bash arith is i64-only; huck
  matches.
- **`shopt -s` integer modes** — bash arith doesn't have these
  configurable.
- **C-undefined behavior for shifts ≥ 64** — huck errors deliberately
  (predictable); bash exposes platform-dependent behavior.
- **`integer` / `declare -i` variables** — variables that auto-arith-
  evaluate on assignment. Separate concern, deferred.

## Testing

### Tokenizer unit tests (~14 in `src/arith.rs` tests module)

Names listed in the design's Section-5 test plan. Cover all new
tokens + literal forms + base-N edge cases + distinguish-from-old
tokens (e.g. `==` vs `=`, `<<` vs `<` vs `<<=`).

### Parser unit tests (~8)

- Precedence correctness for bitwise, shift, power.
- Right-assoc verification for `**` and assignment.
- LHS-must-be-Var guards for assignment and postfix inc/dec.
- All compound-assignment forms parse to the right `AssignOp`.

### Evaluator unit tests (~16)

- Each new binop.
- Shift bounds and arithmetic right shift sign behavior.
- Power with zero / positive / negative exponents.
- Assignment mutates shell state.
- Compound assigns read-then-modify-then-write.
- Pre vs post inc/dec return-value distinction.

### Integration tests (~12 in `tests/arith_completion_integration.rs`)

End-to-end via piped scripts. Covers each form via `$((…))` in echo
arguments + state-mutation verification via reading `$var` afterward.

**Total new tests**: ~50. Baseline goes from 1426 → ~1476.

## Documentation

- `docs/bash-divergences.md`:
  - **M-55** → `[fixed v38]` with full operator list (including the
    bundled `**`).
  - **M-56** → `[fixed v38]` with full assignment + inc/dec list.
  - **M-57** → `[fixed v38]` (hex + octal + N#... 2-64).
  - **`**` exponentiation** — mentioned in M-55's description as
    "bundled in v38".
  - **L-03** (already documents non-integer arith errors) — no change
    needed; the existing behavior is preserved.
  - Changelog row.
- `README.md`: new v38 row.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | Tokenizer extension: new `ArithToken` variants + single/multi-char arm rewrites + numeric literal parsing (hex/octal/N#...) + ~14 tokenizer unit tests | Pure lexer task. |
| 2 | AST extension: new `ArithExpr` variants + `AssignOp` enum + `ArithError::NegativeExponent` + `ArithError::ShiftCountOutOfRange` + Display impl updates | Pure data layer. |
| 3 | Parser extension: renumber precedence table + assignment dispatch + inc/dec pre/post + power right-assoc + Tilde prefix + ~8 parser unit tests | TDD: parser tests first. |
| 4 | Evaluator: signature change to `&mut Shell` + new arms + read/write helpers + ~16 evaluator unit tests + update 2 call sites (`src/expand.rs`, `src/param_expansion.rs`) | Most ripply — touches 3 files. |
| 5 | Integration tests (~12 in `tests/arith_completion_integration.rs`) | Same harness as v33/v34/v37. |
| 6 | Docs (M-55/M-56/M-57 → fixed v38 + changelog + README v38) + full-suite verify | Mechanical close-out. |

Process: subagent-driven per `[[huck-iteration-workflow]]` on
`v38-arith-completion` branch. Final code-reviewer pass over the whole
branch diff before `merge --no-ff` into `main`.
