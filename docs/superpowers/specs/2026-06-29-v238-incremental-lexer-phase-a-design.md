# v238 — Incremental Lexer, Phase A: `next_token` behind the `Vec<Token>` API

**Iteration:** v238. **Umbrella design:**
`docs/superpowers/specs/2026-06-29-incremental-lexer-rearch-design.md`
(this is Phase A of that multi-phase re-architecture).

## Goal

Reimplement the batch tokenizer as an **incremental, pull-based** engine
(`Lexer::next_token`) **without changing observable behavior or any public
signature**. The whole input is still tokenized into a `Vec<Token>` by the
existing entry points; internally that vec is now produced one token at a time.
This proves the incremental engine in isolation before the parser is ever
touched (Phase B).

## Core invariant — genuinely one token at a time (NO TRICKS)

This is the whole point of the iteration and is non-negotiable:

1. **`Lexer::next_token` scans incrementally from the `CharCursor`.** It consumes
   only as much input as the next token (or one source unit) requires, then
   returns. It MUST NOT pre-run the batch tokenizer and hand out from a
   pre-built vec — that defeats the purpose and is explicitly forbidden. At any
   point mid-stream, input past the cursor is unread.
2. **`tokenize()` is built BY calling `next_token`.** The public
   `tokenize`/`tokenize_with_opts`/`tokenize_partial`/`tokenize_no_brace` all
   route through `tokenize_partial_inner`, whose body is nothing but a loop that
   calls `lx.next_token()` and pushes each returned token into the result vec.
   There is no separate batch path; the vec IS the drained `next_token` stream.
3. **The only buffering is bounded overflow, not the input.** `history` holds at
   most the tokens of the single source unit currently being produced (a brace
   expansion `{a,b,c}` yields N Words in one step) plus tokens scanned while
   stalled on a not-yet-complete heredoc (see Mechanics #3). It never holds the
   whole input. Removing the buffer entirely is impossible only because one
   source unit can yield N tokens; that is inherent, not a trick.

A reviewer must be able to confirm, from the diff, that the cursor advances
lazily and that `tokenize` contains a `while/loop { … next_token() … }` and no
other token source.

## Non-goals (explicitly deferred)

- **No mode stack.** The clean `Mode`-stack decomposition is Phase A.2.
- **No parser changes.** `command::parse` still receives a `Vec<Token>`.
- **No behavior change.** Output must be **byte-identical** to today's lexer for
  every input. This is a pure internal refactor.
- No change to `LexerOptions`, `Token`, `Span`, error types, or the
  `tokenize` / `tokenize_with_opts` / `tokenize_partial` / `tokenize_no_brace`
  signatures.

## What changes

The core `tokenize_partial_inner(input, opts, brace_expand) -> (Vec<Token>,
Option<(LexError, usize)>)` is the only function restructured. Today it is a
single closure-wrapped `loop` over a `CharCursor`, with ~10 local mode flags and
a local `tokens` vec. It becomes a `Lexer` driver:

```rust
struct Lexer<'a> {
    cursor: CharCursor<'a>,
    opts: LexerOptions,
    brace_expand: bool,
    history: Vec<Token>,     // produced tokens (replaces the local `tokens` vec)
    pos: usize,              // index of the next token next_token() will hand out
    // --- former locals, now fields (verbatim semantics) ---
    parts: Vec<WordPart>,
    current: String,
    has_token: bool,
    token_start: usize,
    token_start_line: u32,
    token_start_col: u32,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    pending_heredocs: VecDeque<PendingHeredoc>,
}

enum Step { Produced, Eof }   // one scan iteration's outcome

impl<'a> Lexer<'a> {
    fn new(input: &'a str, opts: LexerOptions, brace_expand: bool) -> Self;

    /// Exactly ONE iteration of the old `loop` body: advances the cursor and
    /// appends 0..N tokens to `self.history`. The old `continue` becomes
    /// `Ok(Step::Produced)`, the old `break` becomes `Ok(Step::Eof)`, and `?`
    /// errors propagate unchanged.
    fn scan_step(&mut self) -> Result<Step, LexError>;

    /// Hand out the next token, scanning only when the buffer runs dry (or when
    /// the next buffered token is a heredoc still awaiting body backfill).
    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        loop {
            if self.pos < self.history.len() && !self.backfill_pending_at(self.pos) {
                let t = self.history[self.pos].clone();
                self.pos += 1;
                return Ok(Some(t));
            }
            match self.scan_step()? {           // scans rest-of-line + newline,
                Step::Eof => return Ok(None),   // backfilling any pending heredoc
                Step::Produced => continue,
            }
        }
    }
    /// True iff a pending heredoc targets `idx` (its body is not yet collected).
    fn backfill_pending_at(&self, idx: usize) -> bool {
        self.pending_heredocs.iter().any(|ph| ph.token_idx == idx)
    }
}
```

`tokenize_partial_inner` becomes a thin drain loop:

```rust
fn tokenize_partial_inner(input, opts, brace_expand) -> PartialTokens {
    let mut lx = Lexer::new(input, opts, brace_expand);
    let mut out = Vec::new();
    loop {
        match lx.next_token() {
            Ok(Some(t)) => out.push(t),
            Ok(None) => return (out, None),
            Err(e) => return (out, Some((e, lx.cursor.offset()))),
        }
    }
}
```

All four entry points keep calling `tokenize_partial_inner` and are unchanged.

### Mechanics that must be preserved

1. **0-token iterations.** Whitespace / a consumed-but-not-emitting char is an
   old `continue`; `next_token` loops on `Step::Produced` until a token lands or
   EOF — so callers still never see "empty" steps.
2. **N-token iterations.** Brace expansion emits N Words in one step (it pushes
   N to `history`); the next N `next_token` calls drain them without re-scanning.
3. **Heredoc body backfill — readiness/stall rule (correctness-critical).** A
   `<<DELIM` emits a `Heredoc` token into `history[i]` with an EMPTY body and
   records a pending entry; the body is collected and backfilled into
   `history[i]` only at the NEXT newline (which may be several tokens later:
   `cat <<EOF; echo hi⏎<body>`). In the batch lexer this was invisible because
   the result vec was built after all backfills. In a streaming lexer it is a
   trap: if `next_token` hands out `history[i]` (the empty-body Heredoc) before
   the backfill runs, the token is wrong and output is NOT byte-identical.
   **Rule:** `next_token` MUST NOT hand out `history[pos]` while a pending
   heredoc has `token_idx == pos`; instead it keeps calling `scan_step` (which
   scans the rest of the line, then the newline that backfills it) until the
   pending entry for `pos` is cleared, then hands out the now-complete token.
   Token ORDER is preserved (later same-line tokens are buffered during the
   stall and handed out after the Heredoc). This stall is bounded by the heredoc
   body length — still incremental, never whole-input. At EOF with a heredoc
   still pending, `scan_step` returns `UnterminatedHeredoc` exactly as today.
4. **`take_fd_prefix!`** (replace a digit/`{n}` Word with a `RedirFd` at the same
   span by popping `history.last()`) becomes a method/macro over `self.history`;
   identical logic.
5. **Partial + error contract.** On a lex error mid-stream, the tokens already
   in `history` (and drained to `out`) are kept, and the error + byte offset
   (`lx.cursor.offset()`) are returned — exactly as the closure did.
6. **`=~` regex operand / `expect_regex`** branch at the top of the old loop runs
   at the top of `scan_step` (before the cursor read), preserving the operand's
   first-byte offset.

### Recursion (command substitution) — unchanged in Phase A

`parse_substitution_body` / `scan_cmdsub_body` / `scan_backtick_substitution`
still call `tokenize_with_opts` + `command::parse` internally. That now spins up
a nested `Lexer`, which is fine and requires no change. (Removing this recursion
is Phase B, not here.)

## Verification

The contract is **byte-identical token output**, so the existing suite is the
oracle:

- `cargo test --workspace` (3864 tests) — any structural divergence in the token
  stream breaks parser/expansion/executor tests.
- All 152 `tests/scripts/*_diff_check.sh` harnesses (release binary) — assert
  byte-identical bash↔huck output across the surface.
- **New focused `next_token` unit tests** in `lexer.rs` that VALIDATE THE
  INCREMENTAL API DIRECTLY (not just via `tokenize`). These are a required
  deliverable, per the multi-token mandate:
  1. **Multi-token sequence by hand.** For `echo foo | grep bar`, call
     `next_token()` repeatedly and assert it returns the EXACT ordered sequence
     `Word(echo), Word(foo), Op(Pipe), Word(grep), Word(bar)`, then `None` — one
     explicit `assert_eq!` per call, proving repeated single-token reads work.
  2. **Equivalence over a spread.** For a table of inputs (plain words; single +
     double quotes; `${x}` / `${x:-y}`; `$(cmd)`; `$((1+2))`; `` `cmd` ``;
     `[[ $x =~ ^a ]]`; a heredoc `cat <<EOF\n…\nEOF`; brace expansion `a{1,2}b`;
     `2>&1` fd-prefix redirect; multi-line with `\n`), assert that draining
     `next_token()` to `None` yields a `Vec<Token>` EQUAL to `tokenize()` of the
     same input — same kinds AND same spans.
  3. **N-token step.** `a{1,2,3}b` — assert successive `next_token()` calls
     return the three expanded Words one at a time (the buffered overflow drains
     across calls, not all at once).
  4. **0-token step.** Leading/embedded runs of spaces/tabs — assert
     `next_token()` skips them and returns the next real token (no empty/None
     mid-stream).
  5. **Heredoc readiness.** `cat <<EOF; echo hi\nbody\nEOF\n` — assert the
     `Heredoc` token handed out by `next_token` already carries the COMPLETE
     body (proving the stall rule), and that the surrounding tokens
     (`Word(cat)`, `Op(;)`, `Word(echo)`, `Word(hi)`, `Newline`) come out in the
     right order.
  6. **Partial + error parity.** A mid-stream lex error (e.g. `echo "unterm`)
     drained via `next_token` returns the same tokens-so-far and the same error
     byte offset as `tokenize_partial`.

## Risks & mitigations

- **Control-flow fidelity.** The old body has many `continue`/`break`/early
  `return Err`. Mechanical mapping (`continue`→`Ok(Step::Produced)`,
  `break`→`Ok(Step::Eof)`, `?` unchanged) must be exact. Mitigation: convert in
  one pass, lean on the byte-identical oracle, and add the direct
  `next_token`-vs-`tokenize` equality test.
- **Borrow checker.** Macros that referenced several locals at once now touch
  `self` fields; some arms may need local rebinds to satisfy the borrow checker
  (e.g. copy `self.opts` into a local before a `&mut self.history` call).
  Mitigation: keep helper scanners taking `&mut CharCursor` + values (as today),
  not `&mut self`.
- **No silent perf cliff.** `next_token` clones each token out of `history`
  (`Token: Clone`). Acceptable (one clone per token, same as moving out of a
  vec); revisit only if profiling shows it matters.

## Definition of done

- `Lexer::next_token` reads ONE token at a time directly from the cursor (no
  batch-then-dole-out), and `tokenize_partial_inner` is nothing but a loop that
  builds its `Vec<Token>` by calling `next_token`. All four public entry points
  unchanged in signature and behavior.
- Heredoc readiness/stall rule implemented (no empty-body Heredoc ever handed
  out).
- All six `next_token` unit tests above pass — including the explicit multi-token
  sequence test and the drain-equals-`tokenize` equivalence test.
- `cargo test --workspace` green (3864+ new total), all 152 harnesses green
  (release binary), 0 warnings.
- No `Mode` stack, no parser change, no behavior change.
