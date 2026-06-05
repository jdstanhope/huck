# huck v92 — bare-word `[[ word ]]` truthiness Design

**Status:** approved design, ready for implementation plan.
**Implements:** the single-operand form of the `[[ ]]` extended test —
`[[ word ]]` ≡ `[[ -n word ]]` (true iff the operand is non-empty after
expansion). This is the piece v30 (M-14) never did: huck's `[[` parser requires
LHS + operator + RHS (or a recognized unary op), so a lone operand raises
`syntax error: unrecognised operator in '[[ ]]': ']]'`.
**Closes:** **M-14c** (new sub-entry under M-14).
**Branch (impl):** `v92-dbracket-bareword` (created from `main` at plan time).

## Why this matters

bash-completion and oh-my-posh use the bare-word test pervasively
(`[[ ${BASH_COMPLETION_DEBUG-} ]]`, `[[ $1 ]]`, `[[ $cur ]]`, …). Each
occurrence currently raises a syntax error, and because the `[[ … ]]` line then
fails to parse, the following `else`/`fi`/`}` become "unexpected" — a single
missing feature cascades into a wall of noise when sourcing a real `~/.bashrc`.
This one fix clears the dominant class of those errors.

## Verified bash 5.2 semantics (the contract)

- `[[ foo ]]` → rc 0 (non-empty literal).
- `[[ "" ]]` → rc 1 (empty string).
- `x=hi; [[ $x ]]` → rc 0; `x=""; [[ $x ]]` → rc 1; `unset x; [[ $x ]]` → rc 1.
- `[[ a && b ]]` → rc 0 (both operands non-empty); `[[ a && "" ]]` → rc 1.
- `[[ "" || z ]]` → rc 0 (right operand non-empty).
- `[[ ! "" ]]` → rc 0 (negation of an empty-string test).
- `[[ ! foo ]]` → rc 1.
- `[[ ( foo ) ]]` → rc 0 (grouped bare-word test).
- `[[ word == x ]]` → unchanged binary `==` comparison (operator still wins).
- A lone token that *looks* like an operator is still just a string:
  `[[ == ]]` → rc 0 (`-n "=="`, non-empty), matching bash.

The operand is subject to the same word/parameter expansion as any other `[[`
operand (it flows through `next_test_word` / the existing `TestExpr::Unary`
evaluation for `-n`).

## Section 1 — The parser fix (`src/command.rs`)

The bug lives in `parse_test_atom`. After the unary-op check, it consumes the
LHS word and then **unconditionally** calls `iter.next()` expecting an operator.
For `[[ foo ]]` the next token is the `]]` close (a `Token::Word("]]")`), which
matches no operator arm and falls through to
`Err(ParseError::TestExprBadOperator("]]"))`.

**Two changes, both in `src/command.rs`:**

1. **New helper** `next_is_test_binary_operator<I>(iter: &Peekable<I>) -> bool`
   that *peeks only* (consumes nothing) and returns `true` iff the next token is
   a recognized `[[` binary operator:
   - `Some(Token::Op(Operator::RedirIn))` or `Some(Token::Op(Operator::RedirOut))`
     (the `<` / `>` lexical-comparison operators), **or**
   - `Some(Token::Word(w))` whose `word_literal_text(w)` is one of:
     `==`, `=`, `!=`, `=~`, `-eq`, `-ne`, `-lt`, `-gt`, `-le`, `-ge`,
     `-nt`, `-ot`, `-ef`.
   - Anything else (`]]`, `)`, `&&`/`||` as `Op(And)`/`Op(Or)`, `None`, or a
     non-operator word) → `false`.

   This operator set must stay in lock-step with the arms of the existing
   operator `match` in `parse_test_atom`; a doc comment on both points at the
   other so future operator additions update both.

2. **Bare-word early return** in `parse_test_atom`, inserted *after* the LHS is
   consumed and *before* the operator `match`:

   ```rust
   iter.next();                 // consume LHS word (unchanged)
   let lhs = first_word;

   // Bash: `[[ word ]]` ≡ `[[ -n word ]]`. When no binary operator follows
   // (next token is `]]` / `)` / `&&` / `||` / end-of-input), the operand
   // alone is a non-empty-string test. See next_is_test_binary_operator —
   // keep its operator set in sync with the match arms below.
   if !next_is_test_binary_operator(iter) {
       return Ok(TestExpr::Unary {
           op: TestUnaryOp::StringNonEmpty,
           operand: lhs,
       });
   }

   let op_token = iter.next();  // existing match, now only reached with a real op
   match op_token { /* unchanged */ }
   ```

The existing operator `match` is **byte-unchanged**; it is simply now only
entered when an operator genuinely follows. Its `None` and bad-operator arms
remain as defensive fallbacks (effectively unreachable for well-formed input,
but retained — e.g. `[[ a == ]]` still reaches `next_test_word` and errors as
before).

### Why this composes for free

`parse_test_atom` sits below the precedence layers `parse_test_or` →
`parse_test_and` → `parse_test_not`. Those layers already split on `&&`/`||`
(`Op(And)`/`Op(Or)`), `!`, and `(`/`)`. Because the bare-word return fires
exactly when the next token is *not* a binary operator, the surrounding layers
keep working unchanged:

- `[[ a && b ]]` → `parse_test_and` calls atom for `a` (next is `&&` → not a
  binary op → `-n a`), consumes `&&`, calls atom for `b` (next is `]]` → `-n b`).
- `[[ ! foo ]]` → `parse_test_not` strips `!`, atom yields `-n foo`, then `Not`.
- `[[ ( foo ) ]]` → grouping consumes the parens, inner atom yields `-n foo`.
- The unary path (`-f x`, `-n x`, `-z x`, `-v x`, …) is reached *before* this
  code via `try_unary_op` and is untouched.

No evaluation-side changes: `TestExpr::Unary { op: StringNonEmpty, .. }` is the
exact node `[[ -n word ]]` already produces, and its evaluator already performs
the non-empty test on the expanded operand.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | NEW `next_is_test_binary_operator` peek-helper; bare-word early return in `parse_test_atom`; unit tests for the new parse shapes |
| `tests/dbracket_bareword_integration.rs` | NEW — binary-driven integration tests (rc assertions) |
| `tests/scripts/dbracket_diff_check.sh` | EXTEND the existing v87 `[[` harness with bare-word fragments (no new harness file) |
| `docs/bash-divergences.md` | NEW sub-entry **M-14c `[fixed v92]`**; changelog entry; bump the Tier-2 fixed count; log newly-discovered deferrals (see below) |
| `README.md` | v92 iteration row |

## Testing

1. **Unit tests** (`src/command.rs`): assert the parsed `TestExpr` shape —
   `[[ foo ]]` → `Unary{StringNonEmpty, "foo"}`; `[[ a && b ]]` →
   `And(Unary{-n,a}, Unary{-n,b})`; `[[ word == x ]]` → `Binary{StringEq,..}`
   (regression guard that the operator path still wins); `[[ ! foo ]]` →
   `Not(Unary{-n,foo})`; `[[ ( foo ) ]]` → grouped `Unary{-n,foo}`.
2. **Integration tests** (`tests/dbracket_bareword_integration.rs`, run the
   `huck` binary, assert exit code): `[[ foo ]]`→0; `[[ "" ]]`→1;
   `x=hi; [[ $x ]]`→0; `x=; [[ $x ]]`→1; `[[ -n foo && foo ]]`→0;
   `[[ "" || foo ]]`→0; `[[ foo && "" ]]`→1; `[[ ! "" ]]`→0.
3. **bash-diff harness** — extend `tests/scripts/dbracket_diff_check.sh` with
   fragments whose `echo $?` (or output) is byte-identical to bash 5.2:
   `[[ foo ]]; echo $?`, `[[ "" ]]; echo $?`, `s=x; [[ $s ]]; echo $?`,
   `e=; [[ $e ]]; echo $?`, `[[ a && b ]]; echo $?`, `[[ "" || z ]]; echo $?`,
   `[[ ! "" ]]; echo $?`, `[[ ( a ) ]]; echo $?`, and a `[[ word == x ]]`
   regression fragment.

## Newly-discovered deferrals to record (docs-only, not v92 code)

While diagnosing the interactive `source ~/.bashrc` failures, several unrelated
gaps surfaced. They are **out of scope for v92's code** but should be logged in
`docs/bash-divergences.md` as `[deferred]` Tier-2 entries so they're tracked:

- `command CMD` **bare form** (without `-v`/`-V`) — currently unsupported.
- `${var@OP}` parameter **transforms** (`@Q`, `@U`, `@L`, `@P`, …).
- `${arr[@]:-word}` / other `:OP` **modifiers applied to arrays** (M-82
  follow-on; currently errors "modifier … not supported on array").
- arithmetic `${...}` / `arr[i]` **inside `(( ))`** evaluation.
- `export -f` / `export -a` flags.

(Severity/ranking to be assigned when each is picked up; logged here so the
paper trail is complete.)

## Edge cases & notes

- **Empty body** `[[ ]]` is still an error (`EmptyDoubleBracket`) — the
  `is_test_expr_stop` guard at the top of `parse_test_atom` fires before any
  operand is read, so the bare-word path is never reached with zero operands.
- **Operator-looking lone token** `[[ == ]]` → `-n "=="` → rc 0, matching bash
  (a single token is always a string, never an operator).
- **`[[ a == ]]`** (operator present, RHS missing) is unchanged: the binary
  path is entered (`==` is a binary op), `next_test_word` finds the `]]`
  terminator and errors as before.
- **No regression surface for existing tests**: every two-/three-token `[[ ]]`
  test still takes the operator path; only the previously-erroring single-token
  form changes behavior.
