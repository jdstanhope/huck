# v279 — Remove the last lexer forward-scanner: parser assembles `${…}` bad-subst raw

**Issue:** [#107](https://github.com/jdstanhope/huck/issues/107)
**Date:** 2026-07-10
**Type:** Behavior-preserving refactor — delete the last lexer forward-scanner; the
parser assembles the bad-subst raw. Corrective **only** on nested-quote bad-subst
edges. **Not** a #84 fix (see Background).

## Background

The v240–v268 parser-driven front-end arc made the **happy path** of `${…}`
tokenization atom-based: `scan_step_param_head` (lexer.rs:1709) and
`scan_step_param_operand` (lexer.rs:2262) emit one small atom per call, and the
parser (`parse_param_expansion`, parser.rs:922) owns structure, nesting, and
delimiter-matching. On valid input the lexer never forward-scans for `}`.

**One forward-scanner survived: bad-substitution recovery.** When a `${…}` is
malformed, `scan_step_param_head`'s `emit_bad_subst!` macro (lexer.rs:1753) calls
`scan_braced_operand` (lexer.rs:6867), which re-implements depth/quote/`$(…)`/
`$'…'` matching to run forward to the closing `}` — purely to reconstruct the
verbatim `${…}` raw (via `cursor.slice_from(start_off)`) for the
`ParamBadSubst { raw }` diagnostic. That hand-rolled matcher is quote-naive (it
documents the limitation inline at :6868–6872).

The parser already half-owns this: for `${}` it reconstructs the raw itself
(`raw: "${}"`, parser.rs:1108) rather than using the lexer's scanned raw. Today's
lexer/parser split for bad-subst is an accident of history, not a real boundary.

**Relationship to #84 (corrected after empirical check).** `scan_braced_operand`
fires *only* on the bad-substitution path (`emit_bad_subst!`). #84's tracked
repros (`"${foo%*'a'*}"`, `echo "${dbg-'"'hey}"`) are **valid** `%`/`-`
expansions that run through the **happy-path** operand scanner
(`scan_step_param_operand`), not `scan_braced_operand`. So this change does **not
close #84**; it removes one *instance* of the same bug class (a quote-naive brace
scanner) from the bad-subst path. #84 stays open.

## Goal

Delete the forward-scanner and give the **parser** ownership of the `${…}`
grammar *and* the bad-subst raw. The lexer's only remaining job is tokenization:
emit atoms, and where it cannot form a valid param token, emit a zero-width
marker and keep tokenizing toward `}` — never scanning for the delimiter itself.

**This is behavior-preserving for the inputs that route through
`scan_braced_operand`.** Today those inputs (`${}`, `${x@Z}`, `${@Z}`, `${#x@}`,
`${$'y'}`, `${a$'b'}`, `${!x@Z}`, `${x@}`, …) emit the **verbatim source slice**
`source[${ ..= }]` as the raw. The parser reproduces the *identical* slice via
`source_span` — same bytes, because both cut verbatim from `${` to `}`. The only
place output legitimately **changes** is nested-quote bad-subst edges where
`scan_braced_operand`'s quote-naive matcher finds the *wrong* `}` and
over-consumes; the parser's correct atom-level matching finds the right one.
Those (few) cases are a correction, adjudicated vs bash.

Note several of these inputs (`${x@Z}`, `${x@}`, `${!x@Z}`) are cases where huck
already **over-flags** bad-subst relative to bash (rc=1 vs bash rc=0) — those are
pre-existing `@`-transform / indirect divergences (backlog), and this refactor
**preserves** them exactly. Fixing them is out of scope.

### Non-goals (explicitly out of scope)

- The other two deferred param-expansion debts (the three overlapping
  double-quote-context flags; the four parallel engine expansion sites in
  `expand.rs`). See the `huck-param-expansion-debt` memory — only the
  forward-scan removal is in scope here.
- Matching bash's exact *unterminated* wording ("unexpected EOF while looking
  for matching `}'"). huck keeps its own unterminated message; only the rc=2
  **class** is preserved.
- State-dependent transform edges (`${x@Z}`, `${x@}` are rc=0 depending on
  whether the variable is set) — separate semantics, not touched.

## Design

### Division of labor

- **Parser owns the grammar and the raw.** `parse_param_expansion` already
  dispatches on operator kind, consumes operands, and expects `ParamClose`. It
  gains: (a) driving to the matching `ParamClose` after a bad marker by counting
  `ParamOpen`(+1)/`ParamClose`(−1) to depth 0, and (b) assembling
  `raw = source[${.offset ..= }.offset]`.
- **Lexer owns tokenization only.** It still *decides* "these characters cannot
  form a valid param token here" (a character-level judgment the parser cannot
  make — the parser sees atoms, not chars), but its *action* on that judgment
  changes from "forward-scan + emit raw" to "emit a marker + keep tokenizing."

### Atom / marker changes

1. **`TokenKind::ParamBadSubst { raw: String }` → `ParamBadSubst`** (payload-free).
   The token still carries a `Span`; its offset records where badness was
   detected (used only by the defensive fallback, below). `raw` leaves the token.

2. **`emit_bad_subst!` macro (lexer.rs:1753) inverts.** New behavior:
   - push the zero-width `ParamBadSubst` marker at the current offset (cursor
     left sitting on the offending char);
   - return `Produced`. **No forward-scan, no raw.** The lexer does not change
     modes itself — the parser drives the tail (below).

3. **The parser drives the bad-tail with existing machinery — no new lexer
   mode.** Mode routing is already parser-driven, and the parser already owns a
   `Mode::ParamWordOperand` (routes to `scan_step_param_operand(None, '}')`) plus
   a `word_in_mode!` macro that parses a word-operand in a pushed mode and an
   `expect_close!` that consumes the `}`. So on the marker the parser reuses
   them: `word_in_mode!(ParamWordOperand{…})` consumes the rest of the body to
   the `}` — handling nested `${}`/`$()`/`$((`/backticks correctly (their `}` are
   nested closers, not ours), which is what keeps brace-matching right on the
   nested-quote edges the old scanner got wrong — and `expect_close!` consumes the
   closing `}`. The parsed word is
   **discarded**; only its span matters. **No `Mode::ParamExpansion` change, no
   `bad_tail` flag, no hand-rolled depth counter, no dedicated tail scanner.**

4. **Detection sites unchanged.** All ~10 `emit_bad_subst!` call sites (name
   phase: lexer.rs:1803/1809/1817/1886/1932; operand/operator phase:
   2124/2193/2201; plus `NameScan::BadSubst`) keep their *conditions*. Only the
   macro's action changes, in one place.

### Parser changes

- The `ParamBadSubst` consumers at parser.rs:988 (name position) and 1146
  (post-name position) change from "take `raw` off the atom" to: reuse
  `word_in_mode!(ParamWordOperand{ in_dquote: false, enclosing_dquote: quoted })`
  + `expect_close!` to consume through the matching `}` (discarding the word),
  then build `ParamModifier::BadSubst { raw }` with
  `raw = iter.source_span(start_off, close_off)`.
- `start_off` is the `${`'s offset (already recorded on the mode frame by
  `set_param_start_off_from_cursor`, parser.rs:957 — exposed via a getter, or
  captured locally as `cursor_pos − 2`). `close_off` is the `ParamClose` token's
  span offset (captured at `expect_close!`).
- The hardcoded `${}` raw (parser.rs:1108) collapses into this same path.
- If `word_in_mode!`/`expect_close!` are defined textually after parser.rs:988,
  hoist their definitions above the name-dispatch so the name-position site can
  use them.

### New seam: source slice

The parser needs the verbatim source between the opening `${` and the closing
`}`. Add one `pub(crate)` accessor on `Lexer` (it owns the `&str`), e.g.:

```rust
pub(crate) fn source_span(&self, start_off: usize, end_off: usize) -> &str
```

The `${` offset comes from the `ParamOpen` token's `Span`; the `}` offset from
the `ParamClose` token's `Span`. This generalizes today's hardcoded `"${}"`.

### Deletions

- `scan_braced_operand` (lexer.rs:6867) and its four exclusive verbatim helpers:
  `consume_backtick_verbatim`, `consume_paren_cmdsub_verbatim`, `scan_cmdsub_body`,
  `scan_backtick_body` — plus their `#[cfg(test)]` unit tests.
- `scan_braced_name_ext` and `scan_raw_ansi_c_body` **stay** — they scan a name
  lexeme / a bounded `$'…'` span, not to `}`.

## Behavioral taxonomy (empirically characterized against current huck + bash 5.2)

The inputs `scan_braced_operand` actually handles, and the required outcome:

| class | example inputs | current huck | required after refactor |
|-------|----------------|--------------|-------------------------|
| bad-subst, closed, no nested-quote ambiguity | `${}`, `${@Z}`, `${#x@}`, `${x@Z}`, `${x@}`, `${!x@Z}`, `${$'y'}`, `${a$'b'}` | rc=1, raw = verbatim `source[${ ..= }]` | **byte-identical** (parser rebuilds the same slice via `source_span`) |
| unterminated (no `}` before EOF) | `${x`, `${x:-foo`, `${x@Z` | rc=2 unterminated | **unchanged** — lexer's existing unterminated `LexError` propagates |
| nested-quote bad-subst edge (old scanner over-consumes) | e.g. a bad `${…}` whose body has `"…}…"` | possibly wrong `}` / over-consumed raw | **corrected** — parser matches the right `}`; adjudicate vs bash, record the delta |

Two terminal outcomes for a malformed `${…}`:

1. **Reaches the matching `}`** → bad-subst node; `raw = source[${ ..= }]`. For
   the first class above this is byte-identical to current huck (both cut the
   verbatim slice; the marker is emitted at the same offset and the parser reaches
   the same `}`).
2. **Hits EOF before `}`** → `scan_step_param_operand` returns an unterminated
   `LexError` (`UnterminatedBrace` / `UnterminatedQuote` depending on context, as
   it already does, lexer.rs:2276/2430) → parse error rc=2. **EOF wins over
   badness**; the parser **propagates the lexer's unterminated error**, it does
   NOT convert it into a partial bad-subst.

Out of scope / preserved-as-is: the `@`-transform and indirect over-flags
(`${x@Z}`, `${x@}`, `${!x@Z}` are huck rc=1 vs bash rc=0) are pre-existing
divergences (backlog) and must remain **exactly** as they are today; huck's raw
format for `$'…'` names (`${$'y'}` vs bash's normalized `${'y'}`) is likewise
unchanged.

### Defensive fallback (expected unused)

If the plan's characterization sweep surfaces a malformed-but-closed case the tail
cannot tokenize to its `}` (neither "reaches `}`" nor "EOF"), the parser may fall
back to `raw = source[${ ..= last-consumed]`. Any such case must be **flagged for
explicit adjudication**, not silently truncated. Not expected; a net, not a
mechanism.

## Verification

Primary safety net is **characterization** (this is a behavior-preserving
refactor), not bash-diff — because several in-scope inputs intentionally diverge
from bash (the preserved over-flags).

- **New `tests/scripts/param_badsubst_char_check.sh`** — a *characterization*
  harness: runs the first-class inputs above plus nested-quote probes through
  **huck**, asserting byte-identical stdout+stderr+rc against a recorded baseline
  captured from the pre-refactor binary. This proves the refactor preserves huck
  behavior on exactly the inputs `scan_braced_operand` handled. Any nested-quote
  case whose output changes is recorded and adjudicated vs bash in the harness
  comments (a deliberate correction, not a regression).
- For the subset where huck already **matches** bash (`${}`, `${@Z}`, `${#x@}`),
  the harness additionally asserts byte-identical vs bash.
- **`huck-syntax` unit suite** stays green; net test count drops only by the
  deleted `scan_braced_operand`/verbatim-helper unit tests. Every *behavioral*
  param-expansion test must keep passing unchanged — that is the core proof this
  is a pure refactor.
- `cargo fmt --all --check` clean; `cargo build -p huck` green;
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and
  `-p huck-engine` green (no `--workspace` — it OOMs the box).

## Risk

This touches the `${…}` subsystem the debt note flags as thrash-prone. Mitigation:
the change is bounded (one macro action, two parser consumers reusing existing
operand machinery, one accessor, five deletions — no new lexer mode), the
detection conditions are untouched, and the
bash-diff harness adjudicates every taxonomy edge before merge. The whole-branch
opus review is the net for missed sites.
