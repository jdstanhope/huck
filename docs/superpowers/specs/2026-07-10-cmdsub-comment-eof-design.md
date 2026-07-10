# v281 — Fix #109: a comment/empty `$(…)` (and `<(…)`) body at EOF mis-parses

**Issue:** [#109](https://github.com/jdstanhope/huck/issues/109) (bug, divergence, sev:low).

## Problem

A `$( … )` command substitution whose body is only a `#` comment (or only
whitespace) and whose input ends before the closing `)` is mis-parsed as a hard
syntax error where bash reads it as an unterminated substitution and continues.

```sh
echo "[$(# c with ) paren
echo yo)]"
```

- **bash 5.2**: `[yo]`, rc 0 — the word-start `#` comment runs to end-of-line
  (so the `)` inside it does not close the substitution), the newline ends the
  comment, `echo yo` runs, and the real `)` closes the substitution.
- **huck (stdin / REPL)**: `syntax error: expected a command` (×2), rc 2.

A comment-only body reaching EOF fails the same way:

```sh
echo $(
# just a comment
)
```
bash → empty output, rc 0; huck (stdin) → `syntax error: expected a command`, rc 2.

### Scope of the divergence

The bug is **only** on the stdin/REPL line-reader path. File mode and `-c` mode
already parse both cases correctly (`[yo]` / empty), because they parse the whole
buffer at once. Confirmed on `main`:

| input | file / `-c` | stdin |
|---|---|---|
| `echo "[$(# c with ) paren` ⏎ `echo yo)]"` | `[yo]` ✓ | syntax error ✗ |
| `echo $(` ⏎ `# comment` ⏎ `)` | empty ✓ | syntax error ✗ |

## Root cause

The stdin/REPL reader (`crates/huck-cli/src/repl.rs`) reads one physical line at
a time and calls `continuation::classify` on the accumulated buffer. On
`Incomplete` it appends the next line to the **parse buffer joined with a real
newline** (`repl.rs:483` — only the separate *history* string uses the
collapsing joiner), then re-classifies. So the comment-ending newline is
preserved for the eventual parse; the reader just has to recognize the first
line as *incomplete* rather than *error*.

`classify` maps parse outcomes: lex-level "unterminated" and structural
`ParseError::UnterminatedSubshell` → `Incomplete` (keep reading); any other
`ParseError` → `Error` (stop). For `$(`, when the body has no command and input
ends before `)`, `parse_command_sub` (`parser.rs:1589`) skips leading
`Blank`/`Newline` atoms, sees the next atom is not `)` (it is EOF / `None`), and
falls into the `else` arm calling `parse_subshell_sequence`, whose first
`parse_command_then_pipeline` returns `ParseError::MissingCommand`. That
propagates out of `parse_command_sub` unchanged → `classify` returns `Error` →
the reader stops before reading the closing line.

The **parallel bare-subshell path does not have this bug**: `parse_subshell`
(`parser.rs:4664`) guards `peek == None` → `UnterminatedSubshell` *before*
delegating to `parse_subshell_sequence`. So `( #comment` and multi-line
`(# c with ) paren` ⏎ `echo yo)` already work (verified: `unexpected end of
input` and `yo` respectively). `parse_command_sub` — and its twin
`parse_process_sub` (`parser.rs:1668`) — simply lack the equivalent guard.

## Design

Add the same `peek == None → UnterminatedSubshell` guard that `parse_subshell`
already uses, to both substitution-body parsers, so all three paths behave
identically. No `classify` change (it already maps `UnterminatedSubshell` →
`Incomplete(Subshell)`).

### Change 1 — `parse_command_sub` (`crates/huck-syntax/src/parser.rs`)

After the leading `Blank`/`Newline` skip loop (ends ~line 1618) and **before**
the empty-`)` check (~line 1619), insert:

```rust
    // A body that is only whitespace/comments reaching EOF before `)` is an
    // UNTERMINATED substitution, not a missing command — mirror parse_subshell's
    // guard (~4664) so the REPL/stdin reader keeps reading instead of erroring.
    if iter.peek_kind()?.is_none() {
        iter.pop_mode();
        return Err(ParseError::UnterminatedSubshell);
    }
```

### Change 2 — `parse_process_sub` (`crates/huck-syntax/src/parser.rs`)

`parse_process_sub` (~line 1668) is structurally identical (push `CommandSub`
mode → skip leading `Blank`/`Newline` → empty-`)` check → else
`parse_subshell_sequence`). Insert the **same** guard after its skip loop (ends
~line 1688) and before its empty-`)` check (~line 1689):

```rust
    if iter.peek_kind()?.is_none() {
        iter.pop_mode();
        return Err(ParseError::UnterminatedSubshell);
    }
```

### Behavior after the fix

- `classify("echo \"[$(# c with ) paren")` → `Incomplete(Subshell)` (was
  `Error`); the reader appends the next line and the full buffer parses to
  `[yo]`.
- `classify("echo $(")` and `classify("echo $(# c")` → `Incomplete(Subshell)`.
- **Bonus:** a single-line-at-true-EOF `echo $(#c` (whole-buffer/`-c`) now
  reports `unexpected end of input` (bash-like) instead of `expected a command`.
- Unchanged: empty `$()`/`$( )`/`<()` (hit `)` → empty body, `Complete`);
  `$( ; )` and other mid-body errors (peek is not `None` → not remapped, stay
  `Error`); every already-passing `$(cmd …)` case.

## Out of scope

- **Backticks `` `#c` ``** — comment handling inside backticks differs; not part
  of #109. File a separate issue only if it reproduces.
- **History-collapse cosmetics** — the interactive *history* joiner for a
  multi-line `$(…)` whose body contains a `#` comment would splice `; ` where a
  newline is semantically required (`joiner_for(Subshell)` is `"; "`). This is
  pre-existing, affects only recalled-history display (not parse or execution,
  which use the newline-joined buffer), and is not part of this fix.
- No `classify`/`continuation` logic change; no lexer change.

## Testing

- **Parser unit tests** (`crates/huck-syntax/src/parser.rs` test module): a
  comment-only `$(# c` body and an empty-at-EOF `$(` return
  `Err(UnterminatedSubshell)`; a process-sub `<(# c` likewise. Empty `$()` and
  `$( ; )` unchanged (`Complete` / `Error`).
- **`classify` unit tests** (`crates/huck-engine/src/continuation.rs` test
  module): `classify("echo $(# c")` and `classify("cat <(# c")` →
  `Incomplete(ContinuationReason::Subshell)`. (The double-quote-wrapped harness
  form `echo "[$(# c with ) paren` is covered byte-for-byte by the diff-check
  harness; its `classify` reason may surface as `Subshell` or `OpenQuote` — both
  are `Incomplete` and both continue — so it is not asserted at the unit level to
  avoid over-specifying which unterminated construct the parser reports first.)
- **Flip the #109 quarantine**: in `tests/scripts/cmdsub_comment_diff_check.sh`,
  change the `xfail "comment after open"` case back to a hard `check` and remove
  the now-unused `xfail()` helper + its `#109` comment. The harness must be
  8/8 green (the whole point of #109 closing).
- **Diff-check sweep**: `tests/scripts/run_diff_checks.sh` stays green
  (180 passed), now with `cmdsub_comment` passing on merit.
- Per-crate tests: `cargo test -p huck-syntax` and `cargo test -p huck-engine`
  (single-threaded, per the box constraint).

## Notes

- The merged PR auto-closes #109 (`Closes #109`). #109 is a real (not
  intentional) divergence, so no `docs/bash-divergences.md` entry is needed.
- The `cmdsub_comment_diff_check.sh` XFAIL was added in v280 (#110) as a
  self-flagging quarantine; this iteration removes it by fixing the underlying
  bug — the intended lifecycle.
