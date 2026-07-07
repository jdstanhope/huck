# v267 — Lexer history prune + forward-progress guard

**Date:** 2026-07-07
**Status:** design approved, ready for planning
**Scope:** `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`

## Motivation

The huck lexer's `history: Vec<Token>` is a **pure forward lookahead buffer**, not
a replay/backscan log. Every non-test read is at `self.pos`, `self.pos + 1`, or
`self.pos + n`; nothing ever reads behind `pos`. `rewind` does not read old tokens
— it `history.truncate(m.pos)` and re-lexes forward from a byte checkpoint. So the
consumed prefix `history[0..pos]` is dead weight, retained only because `pos` is an
absolute `Vec` index and dropping the prefix would misalign it.

Two consequences, both observed during v266:

1. **Unbounded memory.** `history` is append-only and never pruned, so a long
   sourced script / `-c` string driven on one shared lexer grows `history` without
   bound (one `Token` per token in the whole input).
2. **Runaway growth on a stuck scan.** A raw drain (`while lx.next()? …`) over a
   zero-width *opener signal* (`$((`→`ArithOpen`, `$(`→`CmdSubOpen`, `${`→`ParamOpen`,
   backtick→`BeginBacktick`) with no parser to consume it and push a mode re-emits
   the same signal forever — the cursor never advances past the `$`. This grew
   `history` to ~120 M tokens / 4.8 GB and OOM-killed the v266 resume.

A cap on the lookahead *distance* (`history.len() - pos`) does **not** catch (2):
during a drain, `pos` advances in lockstep with each push, so the distance stays
≈1 while absolute length explodes. The failure is absolute append-only growth, not
lookahead bloat.

This iteration adds two small, self-contained front-end robustness pieces:
**(1)** prune the consumed prefix at safe boundaries (bounds memory), and **(2)** a
forward-progress guard on the scan loop (bounds CPU/memory against the runaway).

Deferred out of v266 (user-agreed) so the oracle deletion could land first; the
oracle is now gone and the atom lexer + `parser.rs` are the sole front-end, so this
work is unblocked.

## Goals

- **Pragmatic memory bound:** a long script / long `-c` / long REPL session keeps
  `history` flat (≈ one logical command's worth of tokens), instead of growing with
  total input length.
- **Runaway safety net:** a scan that produces tokens without consuming input errors
  out cleanly and quickly instead of OOMing.
- Zero behavioral change — parse results, error messages, and bash-diff output are
  byte-identical before and after.

## Non-goals

- **No hard per-command ceiling.** A single gigantic logical command (e.g.
  `a=(…10⁶ elements…)`, or a `cmd; cmd; …` mega-one-liner) still uses O(command)
  memory *during its own parse* — the parser legitimately needs those tokens
  buffered. This is the accepted "pragmatic bound" (not a strict `history.len() ≤ N`
  invariant at all times).
- **No absolute-index / base-offset rebasing.** The design in the pure-lookahead
  memory anticipated making `pos`/`Mark.pos` absolute with a `base` offset and
  translating every access site. That is unnecessary (see Key Invariants) and is
  explicitly avoided.
- **No mark-tracking / RAII on `Mark`.** Not needed — pruning happens only at
  call sites where no mark is outstanding.
- **No REPL-path change.** `process_line` (shell.rs) already builds a fresh lexer
  per line, so the interactive/piped-stdin REPL never accumulates history.

## Key invariants (established by inspection during design)

These are what make the simple design correct; the implementation must not violate
them, and the plan should re-verify them.

1. **`history` is pure forward lookahead.** All ~9 non-test reads are at
   `pos`/`pos+1`/`pos+n`; the rest are `push`. Nothing reads behind `pos`.
2. **`last_significant_kind` is frontier-anchored.** It scans `history.iter().rev()`
   and stops at the first non-`Blank`/`Newline` token — always at/near the frontier,
   never in the prunable prefix. Prefix pruning cannot affect it.
3. **Marks are short-lived and never straddle a `parse_sequence`/command boundary.**
   The parser calls `mark()` in exactly two places — `parse_arith_expansion`
   (parser.rs ~1535) and `parse_arith_command` (~1612). In both, the mark is held
   only across `parse_arith_body` (which delimits an arith body and never recurses
   into `parse_sequence`); the `rewind`/`parse_subshell`/`parse_command_sub` all run
   *after* the mark resolves. Therefore **no mark is ever outstanding at the top of
   `parse_one_unit` or at the connector-loop boundary of `parse_and_or_opts`,
   including nested (subshell / `$(…)` / function-body) calls.**
4. **Heredoc bodies are not stored in `history[0..pos]`.** Atom-path bodies go to
   a separate order-keyed `parsed_heredoc_bodies: Vec<Word>`, drained at unit end by
   `take_heredoc_bodies` and attached by `attach_heredoc_bodies`. The only
   history-index-bearing structure is `pending_heredocs` (keyed by `token_idx`),
   which is a **legacy field only read, never populated, in the atom production
   path**. The prune nonetheless guards on both heredoc queues being empty, so it is
   safe even if that ever changes.

## Piece 1 — history prune (bounds memory)

### Method

```rust
/// Drop the consumed prefix history[0..pos] and reset pos to 0, bounding the
/// buffer to the live (unconsumed) tail. Acts only once pos crosses a modest
/// threshold, to avoid churn.
///
/// PRECONDITION (guaranteed at every call site): no `Mark` is outstanding — a
/// Mark stores an absolute pos this would invalidate. See invariant 3.
/// No-op for a replay lexer, and skipped while any heredoc body is pending
/// (invariant 4).
pub(crate) fn maybe_prune_history(&mut self) {
    if self.replay
        || self.pos < HISTORY_PRUNE_THRESHOLD
        || !self.pending_heredocs.is_empty()        // token_idx would go stale
        || !self.atom_pending_heredocs.is_empty()   // a body is mid-collection
    {
        return;
    }
    self.history.drain(0..self.pos);
    self.pos = 0;
}
```

`HISTORY_PRUNE_THRESHOLD` = **1024** (a named `const`; realizes the "at most ~1000
tokens" target). Because `pos` is a *relative* index and nothing reads behind it,
`drain(0..pos)` + `pos = 0` needs no base offset and no access-site changes: the
small live tail shifts to the front and every existing `history[pos]` /
`history.get(pos + n)` read stays valid. `drain(0..pos)` moves only the live tail
(`history.len() - pos`), which is small (lazy lookahead), so cost is negligible.

### Call sites

1. **Top of `parse_one_unit`** (parser.rs ~3317) — fires once per top-level unit in
   the source/`-c`/`source` reader, the only driver that reuses one lexer across many
   commands. First call is a no-op (`pos == 0`); each later unit prunes the prior
   unit's consumed tokens. Bounds the realistic long-script case.
2. **Top of `parse_and_or_opts`'s connector loop** (parser.rs ~2856, at the top of
   the `loop`, after the leading-`Blank` skip and before the stop checks) — fires per
   command within a sequence. Bounds a long single-line `;`/`&&`/`||`-chain within one
   unit. Also runs for nested sequences (subshell / `$(…)` / function bodies), which
   is safe by invariants 3–4.

Both call the same `maybe_prune_history()`; the threshold + heredoc guards make each
call cheap and unconditionally safe.

### Safety argument

At each call site: no mark is outstanding (invariant 3), nothing reads behind `pos`
(invariants 1–2), and no heredoc body depends on `history` indices (invariant 4,
plus the explicit guard). Dropping `history[0..pos]` and resetting `pos = 0` leaves
`pos` pointing at the same frontier token, so every consumer — including an *outer*
`parse_and_or` frame mid-parse — continues correctly from the frontier. Marks taken
*after* a prune get fresh post-prune positions; none taken *before* survive across a
boundary.

## Piece 2 — forward-progress guard (bounds CPU/memory)

### Progress metric — injected-aware

Add a monotonic `consumed: u64` to `CharCursor`, incremented in `next()`. The plan
must confirm `next()` is the **sole** char-consumption primitive (peek does not
consume; `seek`/`rewind` reposition, not consume) — if any path consumes chars
without going through `next()`, it must bump `consumed` too, or the guard could
false-stall. This counts **both** main-string and injected (alias-body) chars, so
consuming a long injected alias body is real progress. Using the raw
main-string `offset()` would false-stall on a big alias body (its offset is static
while injected chars are consumed); the counter avoids that. `rewind`'s backward
`seek` does not touch `consumed` (it is monotonic over `next()` calls only), and the
subsequent re-lex increments it normally.

### The check

`next_token` and `fill_to` call a thin wrapper instead of `scan_step` directly:

```rust
fn scan_step_guarded(&mut self) -> Result<Step, LexError> {
    let before = self.cursor.consumed;
    let step = self.scan_step()?;
    if matches!(step, Step::Produced) {
        if self.cursor.consumed == before {   // produced a token, consumed nothing
            self.stall_steps += 1;
            if self.stall_steps > SCAN_STALL_CAP {
                return Err(LexError::NoProgress);
            }
        } else {
            self.stall_steps = 0;
        }
    }
    Ok(step)
}
```

`stall_steps: u32` is a `Lexer` field — the runaway spans multiple `next_token` /
`fill_to` calls, so the counter must persist across them. In normal parser-driven
flow a zero-width opener emits once, the parser pushes its mode, and the next
`scan_step` (in the new mode) consumes chars → `stall_steps` resets to 0. Only an
un-consumed re-emit (no parser, or truly pathological input) lets it climb.

`SCAN_STALL_CAP` = **1024** (named `const`). Legitimate flow never exceeds ~1
consecutive zero-consume `Produced` step, so 1024 is far above any real run while
still turning the 4.8 GB / 120 M-token runaway into an immediate, bounded error.

`LexError::NoProgress` is a new variant → maps to `ParseError::Lex` (via the existing
`?`→`ParseError::Lex` path), which REPL continuation classification treats as a hard
error (correct — a stalled lex is not "needs more input").

## Testing

No bash-observable surface changes, so no new `*_diff_check.sh` harness. Correctness
is proven by the existing suites staying green plus targeted Rust unit tests.

**Piece 1 — prune correctness & bound:**

1. **Memory bound (headline):** drive a large multi-unit input (e.g. 5000 `echo N`
   lines) through the `parse_one_unit` loop on one lexer; assert
   `scanned_token_count()` (test-only, = `history.len()`) stays ≤ `THRESHOLD + small`
   at every unit boundary rather than growing ~linearly. Same for a long single-line
   `;`-chain (exercises the `parse_and_or_opts` site).
2. **Parse-identity:** the same inputs yield exactly the expected `Sequence`s
   (pruning must not alter results). The full existing suite is the broad
   no-regression gate; this adds focused cases.
3. **Heredoc across a prune point:** `cat <<EOF; <enough `;`-commands to cross
   THRESHOLD>\nbody\nEOF` — assert the body still attaches (prune skips while
   `atom_pending_heredocs` is non-empty, resumes after).
4. **Mark/rewind across pruning:** `$( (echo x) )` and `((cmd); c2)` embedded in a
   threshold-crossing sequence; assert the `ArithBail` rewind still yields the
   correct cmdsub/subshell parse (no mark stranded by a prune).
5. **Nested prune:** a threshold-crossing `;`-chain inside `$( … )` / `( … )` / a
   function body parses correctly and stays bounded.

**Piece 2 — progress guard:**

6. **Runaway → clean error (regression test for the original OOM):** a raw drain
   over an un-parser-driven zero-width opener (`$((` at command position) returns
   `Err(LexError::NoProgress)` within ~`SCAN_STALL_CAP` steps — bounded, not an OOM.
7. **No false positive:** `$((1+2))`, `${x}`, `$(echo hi)`, backticks, and deep
   nesting all parse (broadly covered by the suite; one focused case).
8. **Injected-aware metric:** an alias whose body exceeds `SCAN_STALL_CAP` tokens
   (e.g. `alias x='a a a … 1500×'; x`) parses without a false `NoProgress`.

**Gates:** huck-syntax + huck-engine suites green (guarded, per-crate, single-
threaded per the memory), 0 warnings, and the full bash-diff sweep unchanged at
156/2.

## Footprint

All in `lexer.rs` + `parser.rs`:

- `HISTORY_PRUNE_THRESHOLD` (1024) + `SCAN_STALL_CAP` (1024) consts.
- `maybe_prune_history()` method + 2 call sites (`parse_one_unit`, `parse_and_or_opts`).
- `CharCursor.consumed: u64` field + increment in `next()`.
- `Lexer.stall_steps: u32` field + `scan_step_guarded` wrapper wired into the two
  driver loops (`next_token` ~4319, `fill_to` ~4355).
- `LexError::NoProgress` variant (+ its `ParseError::Lex` mapping message).

## Risks & mitigations

- **A prune corrupts an outstanding mark or pending heredoc.** Mitigated by
  invariants 3–4 and the explicit heredoc guard; test 3 and 4 exercise these.
- **The guard false-positives on a legitimate input.** Mitigated by the
  injected-aware metric and the large cap; tests 6–8 cover the boundary.
- **Hot-path cost.** `maybe_prune_history` is a threshold-gated integer compare per
  command (drains rarely); the `consumed` increment is one add per char; the
  `scan_step_guarded` wrapper is a couple of comparisons per scan step. All
  negligible; the existing suites' runtime is the check.
