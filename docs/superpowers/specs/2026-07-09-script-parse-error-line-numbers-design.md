# v277 — Script parse error at a unit boundary: restore incremental execution + correct line

**Issue:** [#86](https://github.com/jdstanhope/huck/issues/86) — *Script parse error aborts
already-parsed earlier commands and reports the wrong line* (`divergence` / `bug` / `sev:medium`).

**Goal:** When a script/`source`d file has a parse error (e.g. an unterminated
quote) at the start of a unit, huck must execute the already-parsed earlier
units and report the error at the offending token's line — matching bash — rather
than discarding the preceding unit and reporting the end-of-file line.

## Background

huck reads a script one *unit* (one logical command) at a time in
`run_sourced_contents_in_sinks` (`crates/huck-engine/src/builtins.rs`). The loop
parses a unit with `parser::parse_one_unit`, executes it, then peeks the next
token to find the unit boundary. That reader already carries the v239 recovery
machinery (commit `3022aa0`): when the token *after* a successfully-parsed unit
has a lex error, the `pending_lex_err` path reports it at the token's start line
and still executes the parsed unit.

The parser-driven front-end rearchitecture (v264 flip → oracle deletion → v268)
regressed this. Observed behavior:

```
$ printf "echo ok\n'unterminated\n" | huck script.sh
script.sh: line 3: syntax error: unterminated quote      # wrong line, and…
$                                                          # …"ok" never printed
```

bash:

```
$ bash --norc script.sh
ok
script.sh: line 2: unexpected EOF while looking for matching `''
```

The divergence has two halves — the earlier command doesn't run, and the line is
the EOF line (3) rather than the token's line (2).

## Root cause

In `parse_and_or_opts` (`crates/huck-syntax/src/parser.rs`), unit mode
(`stop_at_top_newline`) ends a unit when it consumes the terminating top-level
`Newline`. Immediately before breaking it calls
`collect_heredoc_bodies_after_newline`, which does `peek_kind()?` to check
whether a `HeredocBodyBegin` follows the newline. That peek **scans the next
unit's first token**. When that token is an unterminated quote the scan returns
`Err`, `?` propagates out of `parse_and_or_opts` → `parse_one_unit` returns
`Err(Lex)`, and the already-built AST for the current unit is thrown away. In the
reader, `parse_one_unit` returning `Err` lands in the generic error arm, which
reports at the cursor's post-scan position (EOF, line 3) and never executes the
lost unit.

The peek is only needed when a heredoc body is actually pending (a unit that used
`<<EOF`). In the common no-heredoc case it is pure over-scan.

## Design

Gate the heredoc-collection peek on actually-pending heredoc state, so a unit
that declared no heredoc never scans past its terminating newline.

### Component 1 — `lexer.rs`: expose pending-heredoc state

Add a predicate mirroring the check `maybe_prune_history` already performs:

```rust
/// True while one or more heredoc bodies are still pending collection (the
/// atom-path queue or the legacy queue is non-empty). Lets a caller that just
/// consumed a unit-terminating newline decide whether it must peek for a
/// `HeredocBodyBegin` — avoiding an over-scan into the *next* unit's first
/// token when no heredoc is pending.
pub(crate) fn has_pending_heredoc_body(&self) -> bool {
    !self.pending_heredocs.is_empty() || !self.atom_pending_heredocs.is_empty()
}
```

### Component 2 — `parser.rs`: gate `collect_heredoc_bodies_after_newline`

Short-circuit the loop on the predicate so `peek_kind()` is only reached when a
body is pending:

```rust
fn collect_heredoc_bodies_after_newline(iter: &mut Lexer) -> Result<(), ParseError> {
    while iter.has_pending_heredoc_body()
        && matches!(iter.peek_kind()?, Some(TokenKind::HeredocBodyBegin { .. }))
    {
        let body = parse_heredoc_body(iter)?;
        iter.push_heredoc_body(body);
    }
    Ok(())
}
```

Rust's `&&` short-circuits: when `has_pending_heredoc_body()` is `false`,
`peek_kind()?` is never evaluated, so the next unit's token is never scanned and
the unit ends cleanly at the newline. The reader's existing `pending_lex_err`
path then surfaces the error at the correct line and executes the parsed unit.

This also covers the narrower heredoc-then-bad-token case: after the pending body
is collected the queue drains to empty, so the loop stops before peeking the
user's next token.

### Data flow after the fix

For `echo ok\n'unterminated\n`:

1. `parse_one_unit` parses `echo ok`; the newline terminates it;
   `has_pending_heredoc_body()` is `false` → no peek → `parse_one_unit` returns
   `Ok(echo ok)`. The cursor sits at the start of line 2.
2. The reader records `tok_off_before = iter.cursor_pos()` (start of line 2),
   then `peek_span()` scans `'unterminated` → `Err` → `pending_lex_err =
   Some((le, tok_off_before))`.
3. The reader executes `echo ok` (prints `ok`), then reports the lex error at
   `line_of(tok_off_before)` = line 2, sets status 2, and restarts past that
   line.

Result: `ok`, error at line 2, rc 2 — bash-identical on stdout and rc.

## Error handling / edge cases

- **No-heredoc units** (common): no peek, unit ends at newline. Fixed.
- **Heredoc unit followed by a good unit** (`cat <<E … E` then `echo ok`): queue
  non-empty at the newline → the collect runs, body attaches, then the queue
  drains and the loop stops. No regression.
- **Heredoc unit immediately before a bad token**: body collected, queue drains,
  loop stops before peeking the bad token. Also fixed by the loop-condition gate.
- **Bad token as the very first token of the file**: unchanged — handled by the
  reader's leading newline-skip error arm, not by this path.

## Testing

- Un-`#[ignore]` `tests/script_line_numbers_integration.rs::lex_error_as_first_token_of_second_unit_reports_its_line`
  (asserts `ok` runs, stderr `line 2:`, rc 2). It must now pass.
- Add integration cases to the same file:
  - multi-unit before the error (`echo a⏎echo b⏎'bad`) → both run, `line 3:`, rc 2;
  - a heredoc unit followed by a good unit → no regression (body attaches, both run);
  - a heredoc unit immediately before a bad token → earlier unit runs, error at
    the bad token's line.
- Add `tests/scripts/script_parse_error_diff_check.sh` asserting byte-identical
  **stdout + rc** vs bash for the newline-separated cases. stderr wording diverges
  by design (huck `unterminated quote` vs bash `unexpected EOF while looking for
  matching`), so stderr is excluded from the byte-compare.
- Run the existing `script_line_numbers_integration`, `heredoc_integration`, and
  `linear_source_reader_integration` binaries to confirm no regression.

## Out of scope

The `;`-trailing-before-newline case (`echo ok;⏎'bad`), where huck groups the
trailing `;` with the next command into one unit, is entangled with the
intentional divergence in [#59](https://github.com/jdstanhope/huck/issues/59)
and is deliberately excluded from v277.
