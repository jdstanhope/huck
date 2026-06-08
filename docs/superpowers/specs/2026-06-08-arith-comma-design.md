# huck v112 — arithmetic comma operator (M-108) Design

**Status:** approved design, ready for implementation plan.
**Implements:** the comma (`,`) operator in shell arithmetic — new **M-108**
(Tier-2). It is the next gap surfaced by `mise ` + `<TAB>` completion after v111
gave huck `getopts`.
**Why now:** bash_completion's `__reassemble_comp_words_by_ref` (line 255) runs
`for ((i = 0, j = 0; i < ${#COMP_WORDS[@]}; i++, j++))`. huck has no arithmetic
comma operator, so it errors `huck: ((: unexpected character: ','`. That aborts
the word-reassembly, leaving `words`/`cword` unpopulated, which then makes the
downstream `_upvars -v … "$cword" …` call (line 339) see mangled arguments and
emit `bash_completion: : cword: invalid option` / `: : invalid option`.
**All three reported errors are one root cause** — adding the comma operator
clears them.
**Branch (impl):** `v112-arith-comma`.

## Background — the contract (verified against bash)

| Expression | bash result |
|---|---|
| `(( a=1, b=2 )); echo "$a $b"` | `1 2` (both set; `((…))` rc from last value) |
| `echo $((1, 2, 3))` | `3` (value = last operand) |
| `echo $(( (1,2) + 3 ))` | `5` (comma works inside parens; `(1,2)`=2) |
| `for ((i=0,j=0; i<3; i++,j++)); do echo "$i:$j"; done` | `0:0 / 1:1 / 2:2` |
| `a=9; echo $(( a=1, 2 )); echo "$a"` | `2` then `1` (comma below assignment: `(a=1), 2`) |

Semantics: `L , R` evaluates `L` (keeping its side effects, e.g. assignments),
discards `L`'s value, evaluates `R`, and yields `R`'s value. It is the
**lowest-precedence** arithmetic operator (below assignment), left-associative.

## Architecture — a thin outer layer (NOT binding-power renumbering)

huck's arithmetic parser (`src/arith.rs`) is a Pratt parser, `parse_expr(min_bp)`
(`~:435`), with carefully tuned binding powers (assignment lbp 2 / rbp 1, ternary
3, power 25/24, postfix `++` 27, …). Slotting comma *below assignment* by
renumbering every BP is risky. Instead, add comma as a **wrapper layer** above
`parse_expr`, since comma is genuinely a sequence at the outermost level:

```rust
// Parse a comma-separated sequence: each operand is a full expression
// (parse_expr(0)); the sequence value is the last. Left-associative.
fn parse_comma_expr(&mut self) -> Result<ArithExpr, ArithError> {
    let mut lhs = self.parse_expr(0)?;
    while self.peek() == Some(&ArithToken::Comma) {
        self.bump();
        let rhs = self.parse_expr(0)?;
        lhs = ArithExpr::Comma(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}
```

Because comma is parsed *outside* `parse_expr`, it cannot interfere with any
existing precedence, and `a = 1, 2` correctly parses as `(a=1), 2` (the
`parse_expr(0)` for the first operand consumes `a=1` fully, then the `,` is seen
by `parse_comma_expr`).

### The two entry points that must call `parse_comma_expr`
All arithmetic — `(( ))`, `$(( ))`, and every clause of a C-style `for` header —
funnels through `pub fn parse(input)` (`src/arith.rs:~384`), which calls
`parse_expr(0)` at `~:387`; and `eval_arith_word` (`src/expand.rs:~113`) calls
`arith::parse`. So:
1. **`arith::parse`** (`~:387`): change `let expr = p.parse_expr(0)?;` →
   `let expr = p.parse_comma_expr()?;`. This single change covers `(( a,b ))`,
   `$(( a,b ))`, and the for-header init/cond/update clauses (each is parsed via
   `arith::parse`).
2. **The parenthesized-group prefix** (`src/arith.rs:~564-565`,
   `Some(ArithToken::LParen) => { let inner = self.parse_expr(0)?; … }`): change
   that inner `self.parse_expr(0)?` → `self.parse_comma_expr()?`, so `(1,2)+3`
   works (the group evaluates the comma-list, yields the last).

## Components

| Unit | File | Change |
|---|---|---|
| Lexer | `src/arith.rs` (tokenizer ~`:171` neighborhood) | recognize `,` → new `ArithToken::Comma` |
| Token | `src/arith.rs` (`enum ArithToken`) | add `Comma` variant |
| AST | `src/arith.rs` (`enum ArithExpr`, `~:320`) | add `Comma(Box<ArithExpr>, Box<ArithExpr>)` |
| Parser | `src/arith.rs` | new `parse_comma_expr`; call it from `parse` (`~:387`) and the `LParen` prefix (`~:565`) |
| Eval | `src/arith.rs` (`pub fn eval`, `~:602`) | `Comma(l, r) => { eval(l)?; eval(r) }` — evaluate l for side effects, discard, return r |

`eval` for `Comma` must evaluate `l` first (so its assignments/`++` take effect),
ignore its value, then evaluate and return `r`.

## Scoped out (minor deferred edge)
- A comma inside a **ternary middle branch** (`1 ? 2,3 : 4`): the ternary's
  `then`/`else` branches keep `parse_expr(0)` (no comma), so this would not
  parse. It is not used by bash_completion and is a rare edge; note it as a
  low/deferred sub-divergence on M-108 rather than special-casing the ternary.
  (Top-level, for-header, and parenthesized commas — everything needed — are
  covered.)
- No new operator precedence interactions beyond comma being outermost.

## Must-not-regress
- All existing arithmetic: assignment / compound-assign, ternary, `**`, `++`/`--`
  (pre/post), bitwise/logical/comparison, parenthesized grouping WITHOUT comma,
  `$(( ))` and `(( ))` rc semantics, C-style `for` headers without comma, arith
  subscripts (`a[i+1]`), `${var:off:len}` arithmetic offsets.
- A bare trailing/leading comma or empty operand should behave like bash (verify:
  `$(( 1, ))` — bash errors; `$(( ,1 ))` — bash errors; match the error path,
  i.e. `parse_expr(0)` on an empty operand already errors "unexpected …").

## Files & responsibilities

| File | Change |
|------|--------|
| `src/arith.rs` | `ArithToken::Comma` + lex `,`; `ArithExpr::Comma`; `parse_comma_expr`; call it from `parse` + `LParen`; eval arm |
| `tests/arith_comma_integration.rs` | NEW — `(( ))`/`$(( ))`/C-style-`for` comma cases |
| `tests/scripts/arith_comma_diff_check.sh` | NEW — 36th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-108 `[fixed v112]`; changelog; README row; Tier-2 count |

## Testing

1. **Unit** (`src/arith.rs` `#[cfg(test)]`): `parse("1,2,3")` shape; `eval` of
   `a=1,b=2` (both vars set, value 2), `1,2,3` → 3, `(1,2)+3` → 5, `a=1,2` → 2
   with `a==1` (comma below assignment), side-effect ordering (`i=0, i++` → i==1).
2. **Integration** (`tests/arith_comma_integration.rs`, binary-driven): `(( a=1,
   b=2 )); echo "$a $b"` → `1 2`; `echo $((1,2,3))` → `3`; `for ((i=0,j=0; i<3;
   i++,j++)); do echo "$i:$j"; done` → `0:0/1:1/2:2`; `echo $(( (1,2)+3 ))` → `5`.
   Verify each against the system bash first.
3. **36th bash-diff harness** `tests/scripts/arith_comma_diff_check.sh` —
   byte-identical fragments for all the contract cases.
4. **Regression**: full suite (2775+), all 36 harnesses, clippy clean. Pay special
   attention to the existing `arith`/`arith_dollar`/`arith_for` tests.
5. **Payoff**: `mise ` + `<TAB>` (or the `__reassemble_comp_words_by_ref` shape:
   `COMP_WORDS=(mise ""); for ((i=0,j=0; i<${#COMP_WORDS[@]}; i++,j++)); do :;
   done; echo ok`) no longer prints `((: unexpected character: ','`, and the
   downstream `_upvars` `invalid option` cascade is gone. Report before/after.

## Edge cases & notes
- **Comma value in `(( ))` rc**: `(( expr ))` returns rc 0 if the final value is
  non-zero, 1 if zero — unchanged; the "final value" of a comma-list is its last
  operand, which the eval arm already returns.
- **Whitespace**: `i = 0 , j = 0` (spaces around comma) must tokenize the same —
  the lexer skips whitespace already; just emit `Comma` for `,`.
- **Nested**: `((a,b),c)` and `a,(b,c)` both fold left correctly via
  `parse_comma_expr` + the paren group calling `parse_comma_expr`.
