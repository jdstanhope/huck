# v255 — Standalone arith command `(( … ))` on the atom-command path

**Status:** design approved (2026-07-03)
**Arc:** Phase C, Stage 2 — porting deferred construct families onto the dormant
atom-command parser (`command_atoms` flag, default `false`), one family per
iteration, byte-identical to the `command.rs` oracle, marching toward the finale
(flip `command_atoms` live + delete the forward-scanning production scanners).

## Summary

Port the **standalone arithmetic-evaluation command** `(( expr ))` (bash's
`Command::Arith`) onto the atom-command parser. Today the atom path DEFERS it
(`parser.rs:2024-2028` → `UnsupportedCommand`); v255 replaces that deferral with a
real parse that is byte-identical to the oracle.

**Dormant + differential + parse-time only.** `command_atoms` stays `false`; the
production path still uses the Word-lexer's pre-scanned `TokenKind::ArithBlock` →
`Command::Arith`, untouched. The atom path builds the *same* `Command::Arith(body:
Word)` AST. The arith body is evaluated at runtime by existing production code —
v255 changes only how the command is *delimited and assembled* at parse time.

**Scope (confirmed):** standalone `(( expr ))` only. Explicitly OUT:
- C-style `for (( init; cond; step ))` (→ v256; it reuses this machinery but is
  unambiguous after `for`, so it's a separate iteration).
- Any change to runtime arithmetic evaluation.

## Background — why this needs the parser, not the lexer

The oracle's Word-lexer scans command-position `(( … ))` as a single pre-lexed
`TokenKind::ArithBlock(String)` — a **forward scan to the matching `))`** — then
`command.rs:1023-1029` converts it to `Command::Arith(arith_string_to_word(text))`
and wraps trailing redirects. That forward scan is exactly what THE RULE forbids on
the atom path (and is the string-based approach I rejected back in v243).

So on the atom path the lexer stays dumb: it already emits command-position `((` as
**two glued `Op(LParen)` atoms** (no `Blank` between them — see `parser.rs:2017`),
and `parse_command` already distinguishes them (`peek(LParen) && peek2(LParen)`,
`parser.rs:2024`). The disambiguation and assembly are entirely the **parser's**
job.

## The glued-vs-spaced discriminator (load-bearing)

Command-position `((` is genuinely ambiguous — it can be an arith command
(`(( expr ))`) OR a nested subshell (`( ( … ) )`). The lexer cannot decide (deciding
needs a forward scan for a matching `))`), so it emits two `Op(LParen)` and lets the
parser decide. The decision keys on whether the two opening parens are **adjacent**:

- **Glued `((`** (no whitespace between): the atom scanner emits `Op(LParen)`,
  `Op(LParen)` back-to-back. `parse_command`'s `peek2 == Op(LParen)` fires →
  **speculative arith** (with a backoff — see below).
- **Spaced `( (`**: the atom scanner emits `Op(LParen)`, **`Blank`**, `Op(LParen)`.
  `peek2` is `Blank`, not `Op(LParen)`, so the `((`-check FAILS and control falls
  through to the existing single-`(` subshell path (`parser.rs:2030`). Parsed as a
  nested subshell — **never** arith.

This is exactly bash's own rule: `((` is the arith-command operator only when the
parens are adjacent; a space makes them two independent subshell opens. The atom
`Blank` token *is* the adjacency signal that the oracle's Word-lexer encodes
structurally (it lexes glued `((` as one `ArithBlock` and spaced `( (` as two
`(` operators). Concretely: `( ( 3 * 4 ) )` is a nested subshell on both paths;
`((3 * 4))` is an arith command on both paths.

Even a glued `((` is only *speculatively* arith — see the backoff below.

## Architecture

**Files:** `parser.rs` only (the `parse_arith_command` dispatch + opener/bail
handling + differential tests). `Command::Arith` and `Mode::Arith` already exist.
No lexer change (see "opener handling"). `command.rs` untouched (empty diff).

Replace the `parser.rs:2024-2028` deferral with a call to `parse_arith_command`:

```rust
fn parse_arith_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    let mark = iter.mark();                 // BEFORE any consume/push (v246 pattern)
    iter.next_kind()?;                      // consume first `(`  (buffered Op(LParen))
    iter.next_kind()?;                      // consume second `(`
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: false, body_started: true });
    let result = parse_arith_body(iter, false);   // reuse v246's body assembler
    iter.pop_mode();                        // pop on ALL paths (mirrors parse_arith_expansion)
    match result? {
        ArithBodyOutcome::Closed(body) => maybe_wrap_redirects(Command::Arith(body), iter),
        ArithBodyOutcome::Bail         => { iter.rewind(&mark); parse_subshell(iter) }
    }
}
```

### Opener handling — no lexer change

`scan_step_arith` (lexer.rs:1959) consumes the `$((` opener only in its
`!body_started` branch (guarded by a `$`-assert). By consuming the two buffered
`Op(LParen)` atoms first and pushing `Mode::Arith { body_started: true }`, that
branch is skipped and scanning begins directly at the body — so the `$`-assert is
never reached and no lexer change is needed. The two `Op(LParen)` are Command-mode
tokens the parser peeked at dispatch; consuming them advances the read position past
`((`, so the next pull enters `scan_step_arith`'s body loop.

### Body assembly — reuse v246's `parse_arith_body`

`parse_arith_body` (parser.rs:1226) is reused verbatim. It pulls body atoms
(literal runs + embedded `$…`/`${…}`/`$(…)`/`` `…` ``/`$((…))` expansions, each
`quoted: true` as in the oracle's `arith_string_to_word`) while tracking the running
grouping `paren_depth`, and returns:
- `Closed(Word)` on the matching `))` — a depth-0 `)` **followed by** `)`
  (`ArithClose`).
- `Bail` on a depth-0 `)` **not** followed by `)` (`ArithBail`).

The resulting body `Word` is the same one `$((expr))` produces, so
`Command::Arith(body)` matches the oracle's `Command::Arith(arith_string_to_word(text))`.

### Close → arith command

`ArithClose` → `maybe_wrap_redirects(Command::Arith(body), iter)`, so a trailing
redirect wraps (`(( x )) >out`), exactly like the oracle (`command.rs:1029`) and
every other atom-path compound.

### Bail → nested subshell (backoff)

`ArithBail` → `rewind(&mark)` (back to before `((`) + `parse_subshell`. After the
rewind, `parse_subshell` consumes the first `(` as the subshell opener and the
second `(` begins a nested subshell (`( (…) )`), matching bash's arith-command
backoff. Examples: `((cmd); cmd2)`, `((echo hi) )`, `(( 3*4 ) )` (glued open,
spaced close), `((a) && (b))`.

### Progress / OOM safety

The bail→rewind path is verified non-looping: after `rewind`, `parse_subshell`
consumes the first `(` and makes forward progress, and it does not peek2-for-`((`,
so it cannot re-enter `parse_arith_command` at the same position — no speculation
loop. Every `parse_arith_body` path consumes input or returns `Closed`/`Bail`.

### The v248 mark-after-peek hazard does not bite

The dispatch peeks `((` before the `mark` (unavoidable — same as v246), but the only
thing re-scanned after a `rewind` is the `((` opener, which is pure operator
tokenization (`(` → `Op(LParen)` regardless of `cmd_at_word_start`). The mutated
word-start flag cannot change it. This is why v246's peek-then-mark-then-bail is
safe, and it is proven by v246's `$( (…) )` reparse.

## Differential corpus

All `diff_cmd` (atom `new_seq` AST == oracle `old_seq` AST) unless marked
`diff_err` (both paths return the SAME `Result`).

**Arith command (glued, closes cleanly → `Command::Arith`):**
- `(( 1 + 2 ))`, `((1+2))`, `(( x = 5 ))`, `(( x++ ))`
- `(())`, `(( ))` — empty body
- `(( (1+2) * 3 ))` — inner grouping parens (depth tracked, not a bail)
- `(( a[0] + 1 ))`, `(( $x + 1 ))` — embedded expansion in the body
- `(( 1 + 2 )) >out`, `(( 1 )) && echo hi` — trailing redirect / pipeline / list

**Bail → nested subshell (`ArithBail` → rewind → `parse_subshell`):**
- `((cmd); cmd2)` — canonical backoff (depth-0 `)` not followed by `)`)
- `((echo hi) )` — glued open, inner closes with a single `)`
- `(( 3*4 ) )` — glued open, **spaced close**
- `((a) && (b))` — `)` after `a` at depth 0 not followed by `)`

**Spaced → subshell (never speculates; existing path):**
- `( ( 3 * 4 ) )`, `( (echo hi) )`, `( ( a ) )`

**Two-Err parity (`diff_err`):**
- `((1+2)` — unterminated (glued open, EOF before the matching `))`). Verify at
  plan time whether the oracle closes this as an error or bails, and pin whichever
  it does.

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` / `diff_err`.
- `command.rs` diff-vs-main stays EMPTY.
- Both `command_atoms` sites (lexer.rs:811, lexer.rs:4167) stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green.
- `cargo build -p huck-syntax` → 0 warnings.
- Progress/OOM: the bail→rewind path verified non-looping (above).

## Task decomposition

- **T1** — `parse_arith_command`: the `parser.rs:2024` dispatch replacement +
  mark/consume/push/`parse_arith_body`/pop + `ArithClose`→`Command::Arith` +
  `maybe_wrap_redirects`. Corpus: the arith-command cases + the spaced-subshell
  cases (which must keep flowing to the existing subshell path unchanged).
- **T2** — the bail→subshell backoff cases (`((cmd); cmd2)`, `((echo hi) )`,
  `(( 3*4 ) )`, `((a) && (b))`) + the unterminated `diff_err` parity. This is where
  `ArithBail`→`rewind`→`parse_subshell` is exercised and the non-loop guarantee is
  asserted.
- **T3** — composition (pipelines / `&&`/`||` / `;` lists / trailing redirects) +
  adversarial corpus + flip the pre-existing deferral test(s) that assert
  command-position `((` returns `UnsupportedCommand` to `diff_cmd`.

## Live-flip carry-forwards

None anticipated beyond the standing set. Any divergence discovered during
implementation (e.g. an unterminated-arith error-kind mismatch, or a
glued-open/spaced-close reconciliation) is pinned as a carry-forward with a test and
recorded in the ledger, per the v248–v254 convention. C-style `for (( … ))` remains
deferred (→ v256).
