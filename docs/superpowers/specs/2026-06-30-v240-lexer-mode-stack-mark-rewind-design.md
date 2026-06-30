# v240 — Lexer mode stack + `mark`/`rewind` machinery (design)

**Goal:** Add the foundational mode-stack and checkpoint machinery to the
`Lexer` — a `modes: Vec<Mode>` stack with push/pop and a `mark`/`rewind` that
resets the char cursor to a byte offset and re-lexes from there — **without
changing any behavior**. This is Stage 1, iteration 1 of the Phase C
parser-driven front-end roadmap
(`2026-06-30-phase-c-parser-driven-frontend-roadmap.md`).

**Architecture:** `Mode::Command` runs today's `scan_step` body unchanged, so the
production token stream is byte-identical; the stack and `mark`/`rewind` are new
*dormant* machinery exercised only by unit tests. `mark`/`rewind` exploit v237
spans (every token carries its byte offset/line/column) to locate the rewind
point without snapshotting the live cursor separately.

**Tech stack:** Rust, `crates/huck-syntax/src/lexer.rs`.

## Global Constraints

- **Byte-identical.** `cargo test --workspace` green + the full
  `tests/scripts/*_diff_check.sh` release sweep byte-identical; 0 warnings.
- **Dormant.** No production code path pushes a non-`Command` mode and nothing
  calls `mark`/`rewind` in production this iteration — the new API is reached
  only by unit tests. The parser is **not** touched.
- **No `TokenKind` changes.** Word-part token kinds arrive with the first real
  mode (a later iteration), not here.
- **No flag→struct refactor.** The ~10 existing `Lexer` flags stay as fields and
  remain `Command`-mode's state; they are *snapshotted* by `mark`, not
  restructured.

## Non-goals (explicitly deferred)

- Real divergent modes (`CommandSub`, `Arith`, `ParamExpansion`, …) and their
  per-mode scan logic — later Stage-1 iterations.
- The `((` arith-vs-subshell disambiguation (needs `Arith`/`Subshell` modes).
- Parser use of the stack / `mark` / `rewind` — Stage 2.
- Heredoc-body interaction with `mark`/`rewind` (see §5).

## 1. The `Mode` enum

Define the full forward-looking set so the vocabulary and the dispatch site exist
from the start, but implement only `Command`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Command,        // default: today's scan_step behavior (the ONLY mode implemented in v240)
    Subshell,       // ( … )                       — placeholder
    CommandSub,     // $( … ) / `…`                — placeholder
    ParamExpansion, // ${ … }                      — placeholder
    Arith,          // $(( … )) / (( … )) / $[ … ] — placeholder
    ArrayLiteral,   // a=( … )                     — placeholder
    DoubleBracket,  // [[ … ]]                     — placeholder
    Regex,          // RHS of =~                    — placeholder
    HeredocBody,    // <<EOF …                      — placeholder
}
```

The placeholder variants are never the active mode in production in v240. If one
is ever the top of stack when `scan_step` runs (only reachable from a test that
mis-drives the lexer), `scan_step` hits `unreachable!("Mode::{:?} not implemented
until its Phase C iteration", m)` — fail-loud rather than silently scanning as
`Command`. Tests exercise push/pop and `mark`/`rewind` with these values **without
pulling a token while one is active** (the mode-stack snapshot/restore is what
they verify, not divergent scanning).

## 2. The mode stack

Add to `Lexer`:

```rust
modes: Vec<Mode>,   // initialized to vec![Mode::Command] in new()/new_live()/from_tokens()
```

API (`pub(crate)` — the parser will drive it in Stage 2; this iteration only
tests call it):

```rust
fn current_mode(&self) -> Mode            // *self.modes.last() (the stack is never empty)
fn push_mode(&mut self, m: Mode)          // self.modes.push(m)
fn pop_mode(&mut self) -> Mode            // pop; debug_assert the stack stays non-empty (Command is the floor)
```

`scan_step` gains a single dispatch at its top:

```rust
match self.current_mode() {
    Mode::Command => { /* the existing scan_step body, unchanged */ }
    other => unreachable!("Mode::{other:?} not implemented until its Phase C iteration"),
}
```

The existing body is moved verbatim under the `Command` arm (a pure wrapping; no
logic edits), so output is identical.

## 3. The `Mark` and `mark()` / `rewind()`

```rust
#[derive(Debug, Clone)]
pub(crate) struct Mark {
    pos: usize,                 // pull index (self.pos) at mark time
    resume: (usize, u32, u32),  // (offset, line, column) to resume scanning from
    // scalar scanning-state snapshot (Command-mode flags):
    brace_expand: bool,
    has_token: bool,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    opts: LexerOptions,
    alias_trailing_eligible: bool,
    modes: Vec<Mode>,           // full mode-stack snapshot
}
```

`mark(&self) -> Mark`:
- `pos = self.pos`.
- `resume = if self.pos < self.history.len() { let s = self.history[self.pos].span; (s.offset, s.line, s.column) } else { (self.cursor.offset(), self.cursor.line(), self.cursor.column()) }`.
  This is the key move: when lookahead is buffered, the resume point is the span
  of the next-to-hand-out token; otherwise it's the live cursor. (The
  `CharCursor`'s `offset()`/`line()`/`column()` already point at the start of the
  next char to produce, *before* any peeked-but-unconsumed char, so a pending
  1-char peek does not corrupt the resume point.)
- Snapshot the scalar flags + `modes.clone()`.
- `debug_assert!(self.current.is_empty() && self.parts.is_empty() && !self.has_token)`
  — `mark` is only valid at a **pull boundary** (no word being accumulated),
  which is where the parser will mark. This keeps the `Mark` from needing to
  clone the word-accumulation buffers.

`rewind(&mut self, m: &Mark)`:
- `self.history.truncate(m.pos); self.pos = m.pos;` — discard buffered/produced
  tokens at/after the mark.
- Reset the cursor: set `self.cursor` to `m.resume` (offset/line/column) and
  **clear its peek buffer** (`peeked = None`). (Add a `CharCursor::seek(offset,
  line, column)` helper that sets `pos`/`line`/`column` and clears `peeked`.)
- Restore the scalar flags, `self.opts`, and `self.modes = m.modes.clone()`.
- Leave `self.current`/`self.parts` as-is (empty by the mark-time invariant).

After `rewind`, the next `next_token()` finds `self.pos == self.history.len()`,
so it scans from the restored cursor under the now-current mode — re-lexing the
same bytes.

### Behavior on a replay (`from_tokens`) lexer
`replay == true` lexers never scan; `history` is fixed. `mark`/`rewind` still
work as pure `pos` save/restore: `rewind` resets `self.pos = m.pos` and does
**not** truncate `history` (debug_assert `m.pos <= history.len()`), so replay
consumers can also speculate. (Not used in v240 production; specified for
completeness and tested.)

## 4. Visibility & placement

`Mode`, `Mark`, and the methods are `pub(crate)` (the Stage-2 parser, same crate,
will use them). All new items live in `lexer.rs` alongside `Lexer`. No changes to
`command.rs`, `errors.rs`, or any engine crate.

## 5. Known edge / deferred interactions

- **Heredocs.** A `mark` taken before a redirect line and a `rewind` after the
  newline that triggers `collect_heredoc_bodies` would cross deferred body
  collection. v240 does not exercise this (no production marks; the `((` case is
  heredoc-free). The interaction is an explicit open question for the
  `HeredocBody` iteration; v240 adds a doc-comment noting `mark`/`rewind` must not
  span heredoc-body collection until that is designed.
- **`pending_heredocs`** is intentionally **not** in the `Mark` snapshot for the
  same reason (out of scope; would be added when heredocs enter the stack).

## 6. Testing (the validation)

New unit tests in `lexer.rs` (`#[cfg(test)]`). All assert against the *live*
(non-replay) lexer unless noted:

1. **rewind reproduces tokens (same mode).** Lex `echo one two; echo three`,
   `mark` at start, pull 4 tokens, `rewind`, assert the next 4 pulls are
   byte-identical (kind + span) to the first 4.
2. **rewind across buffered lookahead.** `peek` (forces a buffered token), then
   `mark`, pull 2, `rewind`, assert re-pull matches — proves `resume` uses the
   buffered token's span, not the advanced cursor.
3. **rewind restores cursor line/column.** Multi-line input; mark on line 1, pull
   across a newline, rewind, assert a re-pulled token's span line/column equals
   the original (cursor `seek` restored line/col).
4. **mode stack push/pop + depth.** `push_mode`/`pop_mode` change
   `current_mode()` and depth as expected; `Command` is the floor.
5. **mark/rewind restore the mode stack.** Push `Arith`, `mark`, push
   `CommandSub`, `rewind`, assert the stack is back to `[Command, Arith]` (no
   token pulled while `Arith`/`CommandSub` active).
6. **mark/rewind restore scalar flags.** Set `dbracket_depth`/`expect_regex` via
   a `[[ … =~ … ]]` lex, `mark`, pull further, `rewind`, assert the flags match
   the snapshot.
7. **replay lexer mark/rewind.** `from_tokens([...])`, mark, pull, rewind, re-pull
   matches; `history` not truncated.
8. **`Command`-only dispatch is byte-identical.** `next_token` drain over a
   representative corpus equals the pre-v240 `tokenize` output (covered
   transitively by the existing suite; add one direct equality test).

The full `cargo test --workspace` suite + the `*_diff_check.sh` release sweep are
the byte-identical oracle for the `scan_step` wrapping. These unit tests are also
the seed of the Stage-1 "parser-simulating" test pattern (drive the lexer the way
the parser will), which grows as real modes are added.

## 7. Done when

- `Lexer` has `modes`, `push_mode`/`pop_mode`/`current_mode`, `Mark`,
  `mark`/`rewind`, and `CharCursor::seek`; `scan_step` dispatches on the mode with
  `Command` = the unchanged body.
- All 8 unit tests pass; `cargo test --workspace` green; full harness sweep
  byte-identical; 0 warnings.
- No changes outside `lexer.rs`; no production caller of the new API.
