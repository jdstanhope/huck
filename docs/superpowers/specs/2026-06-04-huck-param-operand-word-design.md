# huck v84 — parse `${…}` operands as words, not commands Design

**Status:** approved design, ready for implementation plan.
**Fixes:** the `${var:+(…)}` / `${var:-(…)}` parse bug (a parenthesized/metachar operand fails with "invalid operator in parameter-expansion operand"). Discovered loading a stock Debian `~/.bashrc` (its PS1 uses `${debian_chroot:+($debian_chroot)}`).
**Branch (impl):** `v84-param-operand` (created from `main` at plan time).

## Root cause

`parse_braced_operand(body)` (`src/lexer.rs:1300`) parses the operand of a
brace modifier by running the **full command tokenizer** on it
(`tokenize(body)`) and then rejecting any non-`Word` token:

```rust
Token::Op(_) | Token::Newline | Token::Heredoc { .. } | Token::ArithBlock(_)
    => return Err(LexError::InvalidBraceOperand),
```

So any shell metacharacter in the operand — `(`, `)`, `|`, `;`, `&`, `<`, `>` —
becomes an operator token and trips `InvalidBraceOperand`. But in bash a
`${var:OP word}` operand is a **word**: those metacharacters are literal there;
only `$…` / `` `…` `` / quotes are special. `${x:+(a|b;c)}` (x set) → literal
`(a|b;c)`; `${x:-a b}` (x unset, unquoted) → fields `a` `b` (split downstream).

## Scope — one shared function fixes everything

`parse_braced_operand` is the shared operand parser for ALL operand-bearing
brace modifiers (confirmed callers in `src/lexer.rs`):
- `modifier_with_operand` → `:-`/`-` (UseDefault), `:=`/`=` (AssignDefault),
  `:?`/`?` (ErrorIfUnset), `:+`/`+` (UseAlternative).
- `${v/pat/repl}` substitution → both `pattern` and `replacement`.
- `${v:off:len}` substring → both `offset` and `length` (arith).

Fixing this one function corrects all of them, all bash-correctly:
- default/assign/error/alternative WORD operands → metachars literal ✓.
- substitution `replacement` → word ✓; `pattern` → glob chars (`*`/`?`/`[`) and
  `(`/`)` stay literal in the parsed word (the glob engine handles them at match
  time), matching bash (no extglob) ✓.
- substring `offset`/`length` → now allows parenthesized arith (`${x:(1+2):3}`)
  since `(` is literal text fed to the arith parser at expansion ✓.

## Fix

Replace `parse_braced_operand`'s tokenize-then-reject body with a single-pass
**operand-word parser** that walks the (already-extracted) body char-by-char and
builds `WordPart`s, treating metacharacters as literal:

- `$` → `read_dollar_expansion(chars, &mut parts, quoted=false)` — handles
  `$name`, `${…}` (incl. nested modifiers), `$(…)`, `$((…))`, `$'…'`.
- `` ` `` → backtick command substitution (same path the bare-word lexer uses).
- `'…'` → single-quoted literal span → a `quoted` Literal part.
- `"…"` → double-quoted span → expansions active inside, `quoted` parts (so
  `${x:-"a b"}` stays a single field).
- `\c` → escaped literal character.
- any other char (including `(` `)` `|` `;` `&` `<` `>` and spaces) → accumulate
  into an **unquoted** Literal run.

Reuse the existing expansion/quote helpers (`read_dollar_expansion`, the
single-/double-quote scan loops) rather than duplicating them; factor a small
`parse_operand_word(body) -> Result<Word, LexError>` (name at implementer's
discretion). `scan_braced_operand` (extracts the body to the matching `}`,
depth/quote-aware) is UNCHANGED.

**Why this preserves splitting:** literal text (including spaces) lands in
**unquoted** Literal parts, so an unquoted `${x:-a b}` still field-splits to
`a` `b` at expansion time (splitting is downstream and unchanged). Quoted spans
produce `quoted` parts that suppress splitting, matching bash.

## Behavior changes to pin (tests)

- `${x:+(a|b;c)}` → literal `(a|b;c)` (was a syntax error).
- The stock Debian PS1 line `${debian_chroot:+($debian_chroot)}` parses.
- `${x:-foo | bar}` → operand `foo | bar` (literal), not a parse error.

Two existing lexer unit tests assert the OLD behavior and MUST be updated:
- `parse_braced_operand("foo | bar")` currently asserts
  `Err(LexError::InvalidBraceOperand)` → change to `Ok` with literal
  `foo | bar`.
- the `"foo bar"` test asserts a specific 3-part structure (`foo` / space /
  `bar`); the new parser yields a single literal run `foo bar`. Update the
  assertion to check the expanded/flattened text (behavior), not the part
  count. (Functionally identical: both field-split to `foo` `bar`.)

Keep the existing `scan_braced_operand` tests (nesting/quote/unterminated) — that
function is unchanged.

## Out of scope

- Word-splitting semantics (already correct; downstream of the lexer).
- `!`-negation parsing; tool-init `[[ ]]` operators (separate, see prior notes).
- The `InvalidBraceOperand` variant may become unused after this change — if so,
  remove it (and its `Display`/tests) or keep if still produced elsewhere
  (`grep` at implementation time).

## Testing

1. **Unit tests** (`src/lexer.rs`): `parse_operand_word`/`parse_braced_operand`
   on: `(a)`, `a|b`, `a;b`, `a(b)c`, `($x)` (expansion + parens), `"a b"`
   (quoted → single part), `'a|b'` (single-quoted literal), `${y:-z}` (nested),
   `` `cmd` ``, empty `""`, `foo bar` (→ splittable). Plus the two updated tests.
2. **Integration tests** (`tests/param_operand_integration.rs`, binary-driven):
   `${x:+($x)}` with x set/unset; the Debian PS1 assignment then `echo "$PS1"`
   doesn't error; `${x:-(a|b)}` literal; `${x:-a b}` unquoted splits to two args;
   `${x:-"a b"}` quoted stays one; `${v/(x)/y}` substitution with literal parens;
   `${x:(1+2):3}` parenthesized substring offset.
3. **bash-diff harness** `tests/scripts/param_operand_diff_check.sh` (huck's
   11th): the above, byte-identical to bash 5.2.

## File-change map

| File | Change |
|------|--------|
| `src/lexer.rs` | rewrite `parse_braced_operand` to parse the operand as a word (new `parse_operand_word` char-walk reusing `read_dollar_expansion` + quote loops); update 2 operand unit tests; remove `InvalidBraceOperand` if now unused |
| `tests/param_operand_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/param_operand_diff_check.sh` | NEW — huck's 11th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | brief entry for the fix (a new low/medium entry or a note); changelog; summary stamp; README v84 row |
