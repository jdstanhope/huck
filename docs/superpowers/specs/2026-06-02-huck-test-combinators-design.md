# huck v75 — Test Combinators (M-25)

**Status:** design complete; ready for plan.
**Date:** 2026-06-02.
**Closes:** M-25 (`test -a`/`-o`/`(`/`)` combinators), high-priority Tier-2 entry deferred since the original 2026-05-23 audit.

## Goal

Extend the `test` / `[` builtin with bash-compatible AND/OR combinators and parenthesized grouping. After v75, `[ -f a -a \( -r b -o -w c \) ]` works as a nested boolean expression. The existing POSIX 0-4-arg shortcut algorithm is preserved for backward compatibility; the new recursive-descent parser handles 5+ args AND any short-form case that falls through with a parse error.

## Out of scope

- `[[ ]]` extended test (M-14, already shipped v30). Bash uses `&&`/`||` there, not `-a`/`-o`. v75 only touches `test` / `[`.
- New unary or binary primitives (M-27 covers `-p`/`-S`/`-b`/`-c`/`-O`/`-G`/`-N`/`-k`/`-u`/`-g`/`-t` — still deferred).
- `-v VAR` (M-26, separate deferral).

## Architecture

### Grammar

Bash/POSIX grammar for `test`:

```
EXPR    ::= EXPR -o ANDEXPR | ANDEXPR
ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR
UNEXPR  ::= ! UNEXPR | PRIMARY
PRIMARY ::= ( EXPR ) | <unary-op> <word> | <word> <binop> <word> | <word>
```

Precedence (low to high): `-o` < `-a` < `!` < parens < unary/binary primaries.

### Parser

New private `Parser` struct in `src/test_builtin.rs`:

```rust
struct Parser<'a> {
    args: &'a [String],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn parse_expr(&mut self) -> Result<bool, String>;     // OR-chain
    fn parse_and(&mut self) -> Result<bool, String>;      // AND-chain
    fn parse_unary(&mut self) -> Result<bool, String>;    // ! prefix
    fn parse_primary(&mut self) -> Result<bool, String>;  // ( ) / op / word
}
```

- **`parse_expr`** parses an `ANDEXPR`, then while the next token is `-o`, consume it and OR with another `ANDEXPR`. Left-associative.
- **`parse_and`** parses a `UNEXPR`, then while the next token is `-a`, AND with another `UNEXPR`. Left-associative.
- **`parse_unary`** consumes any number of leading `!` tokens and applies negation to the `PRIMARY` they wrap.
- **`parse_primary`** dispatches:
  - If next token is `(`: consume, recurse via `parse_expr`, expect `)`, error if missing.
  - If `pos+1` is a known unary op (`is_unary_op`) AND `pos+2` exists: apply unary.
  - If `pos+2` is a known binary op (`is_binary_op`) AND `pos+3` exists: apply binary.
  - Otherwise: consume one word, return non-empty-string truthiness.

The existing `apply_unary` / `apply_binary` / `is_unary_op` / `is_binary_op` / `negate` helpers stay unchanged. The parser simply orchestrates dispatch.

### Backward compatibility (POSIX short-form)

The current `evaluate` handles 0-4 args via POSIX § 4.62's argument-count algorithm. Every existing test passes under that path. The new parser would naturally treat `-a` as the AND operator, breaking the 1-arg case `[ -a ]` (which must return true — `-a` is a non-empty string in 1-arg position, NOT a unary-op error).

**Strategy**: keep the existing short-form algorithm for 0-4 args. For 5+ args, dispatch to the parser. For 2-4 args, attempt the short-form first; if it returns `Err`, fall through to the parser (this catches forms like `[ ( -n a ) ]` that the short-form rejects but the grammar accepts).

```rust
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    // POSIX 0/1-arg shortcuts — required for correctness ([ -a ] is true).
    match args.len() {
        0 => return Ok(false),
        1 => return Ok(!args[0].is_empty()),
        _ => {}
    }
    // 2-4 args: try the POSIX short-form first. If it errors, fall
    // through to the grammar parser (handles nested-paren cases that
    // the short-form rejects).
    if args.len() <= 4
        && let Ok(b) = evaluate_short_form(args)
    {
        return Ok(b);
    }
    let mut p = Parser { args, pos: 0 };
    let result = p.parse_expr()?;
    if p.pos != args.len() {
        return Err(format!("{}: unexpected argument", args[p.pos]));
    }
    Ok(result)
}

fn evaluate_short_form(args: &[String]) -> Result<bool, String> {
    // …extract the existing 2/3/4-arg match arms verbatim…
}
```

Renaming the existing 2-4 arm bodies into `evaluate_short_form` is a mechanical refactor that preserves behavior. The new `evaluate` is the dispatcher.

### Special-case behaviors

- **`(` and `)` as operands in primary position**: `parse_primary` consumes them as group markers. In any other position (e.g., as a string operand to `=`), they're plain words. This works because `parse_primary` only triggers the paren branch when `pos` is at the start of a primary expression — `(` in the RHS of a binary op is consumed by the word-fetch, not the paren branch.
- **`-a` / `-o` as unary operators**: currently neither is in `is_unary_op`. `-a` historically meant "file exists" but POSIX deprecated it in favor of `-e`; bash retains `-a` as both unary file-test AND binary AND. Our 2-arg short-form recognizes `-a` as unary (if added to `is_unary_op`); the grammar parser treats `-a` as binary in operator position. The position-based disambiguation falls out of the grammar.

  **Decision for v75**: do NOT add `-a` as a unary operator. POSIX prefers `-e`; bash's `-a` unary is deprecated. If the user writes `[ -a foo ]`, the short-form falls through to the parser, which sees `-a foo` and rejects as "unary operator expected". Users who want file-exists write `-e`.

  Wait — that breaks bash compat for `[ -a foo ]`. Let me reconsider. Bash 5.2: `[ -a /tmp ]` → exit 0 (true, `/tmp` exists). If we don't support this, the bash-diff harness will fail.

  **Revised decision**: add `-a` to `is_unary_op` (file-exists, same as `-e`). The 2-arg short-form handles `[ -a /tmp ]`. In the grammar parser, `-a` in operator position is consumed by `parse_and`'s look-ahead; in primary position, `parse_primary` sees `-a` followed by a word and treats it as unary. This dual interpretation matches bash.

- **Negation chains** `[ ! ! -n a ]`: `parse_unary` consumes leading `!` greedily and flips the parity each time. Two `!`s cancel.

- **Empty group** `[ ( ) ]`: `parse_primary` enters the paren branch, calls `parse_expr`. `parse_expr` calls `parse_and` → `parse_unary` → `parse_primary` with `pos` at `)`. `parse_primary` has nothing to match and returns `Err("expression expected")`. The error propagates up.

- **Unbalanced parens** `[ ( -n a ]`: `parse_primary` consumes `(`, recurses, returns from the recursion with `pos` at end-of-args. Looks for `)`, doesn't find it → `Err("missing ')'")`.

- **Operator at end** `[ -n a -a ]` (4 args, falls through from short-form): grammar consumes `-n a`, sees `-a`, calls `parse_unary` → `parse_primary`. `parse_primary` has no more args → `Err("expression expected")`.

## Error handling

| Path | Error | Status |
|------|-------|--------|
| Empty paren group `[ ( ) ]` | `expression expected` | 2 (existing test usage-error code) |
| Missing close paren `[ ( -n a ]` | `missing ')'` | 2 |
| Trailing combinator `[ -n a -a ]` | `expression expected` | 2 |
| Unknown operator (existing) | `<op>: unknown operator` | 2 |
| Unbalanced extra args after parse | `<arg>: unexpected argument` | 2 |

All existing error paths preserved.

## Testing

### Unit tests (in-source)

Extend `mod test_builtin_tests` at the bottom of `src/test_builtin.rs` with ~20 new tests:

1. `combinator_and_both_true` — `[ -n a -a -n b ]` → true.
2. `combinator_and_short_circuit_false` — `[ -z a -a -n b ]` → false.
3. `combinator_or_first_true` — `[ -n a -o -z b ]` → true.
4. `combinator_or_both_false` — `[ -z a -o -z b ]` → false.
5. `parens_group_changes_precedence` — `[ ( -z a -o -n b ) -a -n c ]` → true.
6. `parens_simple_wrapping` — `[ ( -n a ) ]` → true.
7. `nested_parens` — `[ ( ( -n a ) ) ]` → true.
8. `precedence_and_higher_than_or` — `[ -z a -o -n b -a -n c ]` is `-z a -o (-n b -a -n c)` → true.
9. `negation_of_combinator_lhs` — `[ ! -n a -a -n b ]` is `(NOT -n a) -a -n b` → false.
10. `double_negation` — `[ ! ! -n a ]` → true.
11. `empty_parens_error` — `[ ( ) ]` → Err containing `"expression"`.
12. `unbalanced_open_paren_error` — `[ ( -n a ]` → Err containing `")"`.
13. `unbalanced_close_paren_error` — `[ -n a ) ]` → Err (unexpected).
14. `dash_a_unary_two_arg` — `[ -a /tmp ]` → true via short-form (unary file-exists).
15. `combinator_with_binary_operands` — `[ a = a -a 1 -lt 2 ]` → true.
16. `mixed_unary_and_binary` — `[ -e /tmp -a 1 -lt 2 ]` → true.
17. `dangling_combinator_at_end_error` — `[ -n a -a ]` → Err.
18. `lone_paren_string_via_short_form` — `[ ( = ( ]` (3 args) → binary `=` via short-form → true.
19. `long_and_chain_left_associative` — `[ -n a -a -n b -a -n c -a -n d ]` → true.
20. `or_with_one_false_short_form` — confirms 3-arg short-form still works as before.

### Integration tests (`tests/test_combinators_integration.rs`, new)

~6 binary-driven scripts:

1. `if_with_and_combinator` — `if [ -n "a" -a -n "b" ]; then echo Y; fi` → prints `Y`.
2. `if_with_or_combinator` — `if [ -z "" -o -n "x" ]; then echo Y; fi` → prints `Y`.
3. `nested_parens_in_if` — `if [ ( -n a -o -n b ) -a -n c ]; then echo Y; fi` → `Y`.
4. `negated_combinator_in_if` — `if [ ! ( -z a -o -z b ) ]; then echo Y; fi` → `Y`.
5. `bracket_form_with_combinator` — `[ -n a -a -n b ] && echo Y` → `Y`.
6. `error_propagates_exit_status` — `[ ( ]` → non-zero exit.

### Bash-diff harness (`tests/scripts/test_combinators_diff_check.sh`, new)

Parallel to `ifs_diff_check.sh` — ~8 fragments verifying byte-identical output to bash 5.2.21:

```bash
fragments=(
    '[ -n a -a -n b ]; echo $?'
    '[ -z a -o -n b ]; echo $?'
    '[ ( -n a -o -n b ) -a -n c ]; echo $?'
    '[ ! -n a ]; echo $?'
    '[ ! -n a -a -n b ]; echo $?'
    '[ ( -z "" -a -n x ) -o -n y ]; echo $?'
    '[ -a /tmp ]; echo $?'
    '[ -n a -a -n b -a -n c -a -n d ]; echo $?'
)
```

## Documentation

`docs/bash-divergences.md`:
- Update **M-25**: `[deferred] high` → `[fixed v75]`. Body lists supported combinator set, precedence, parenthesized grouping, parser strategy (recursive descent with short-form fallback for backward compatibility).
- Add change-log entry dated 2026-06-02.

`README.md`: add row `| v75 | test combinators (M-25) |`.

## Risks

- **Existing test regression**: every existing `mod test_builtin_tests` test must still pass. **Mitigation**: 0-1 arg shortcut + 2-4 arg short-form-tried-first preserves all existing semantics.
- **`-a` dual role**: file-exists (unary) vs AND (binary). **Mitigation**: `is_unary_op` includes `-a`; 2-arg short-form handles unary case; grammar parser handles AND case in operator position.
- **Short-form fall-through edge cases**: a 3-arg call like `[ ( a ) ]` (3 args: `(`, `a`, `)`) — the short-form's 3-arg arm doesn't recognize this; it falls through to the grammar, which evaluates correctly. **Mitigation**: pinned by `parens_simple_wrapping` test (with the appropriate arg count adapted).
- **Error message exact text**: existing tests may assert on exact error strings. **Mitigation**: keep existing error wording; new errors use distinct strings (`"expression expected"`, `"missing ')'"`).
