# v259 — Iteration A carry-forward fixes (CF2, CF3, CF4) on the atom-command path

**Status:** design approved (2026-07-05)
**Arc:** Phase C — clearing the accumulated live-flip carry-forwards before the finale
(flip `command_atoms` live + delete the forward-scanning scanners). Stage 2 (porting
the deferred families) is COMPLETE as of v258; the atom path has full grammar coverage.
This is the first of the reconciliation iterations. From the verified carry-forward
inventory (`huck_carryforward_inventory.md`, 2026-07-05): **Iteration A = the three
independent trivial/small parser+lexer hygiene/parity fixes** (CF2, CF3, CF4). The
higher-risk items — CF1 fill-walk (Iteration B) and the CF6+CF7 arith quote sub-mode
(Iteration C) — are their own later iterations.

## Summary

Three independent fixes making the dormant atom-command path (`new_seq`,
`command_atoms` default `false`) byte-identical to the `command.rs` oracle (`old_seq`)
for three known divergences. All three were verified against a fresh current-tree
differential probe (2026-07-05) and their fix sites confirmed in the code.

- **CF2** — the atom `parse_sequence` leaks collected heredoc bodies on an early parse
  error (state hygiene; defensive for the live-flip driver).
- **CF3** — an even count of leading `!` before a compound command yields a bare
  compound on the atom path vs `Pipeline{negate:false,[compound]}` on the oracle.
- **CF4** — `$"…"` locale quoting keeps a spurious leading `$` on the atom path vs the
  oracle dropping it.

**Dormant + differential.** `command_atoms` stays `false`. `command.rs` is EMPTY-diff:
all three touch only the atom path or an atom-only classifier; the oracle's
`parse_sequence`, pipeline-wrapping, and `scan_dollar_expansion` are untouched.

## CF2 — heredoc-body queue hygiene (`parser.rs`)

**Root cause.** `parse_sequence` (parser.rs:2729) drains `iter.take_heredoc_bodies()`
only on the SUCCESS path (:2751). The early `Err` returns — `skip_newlines?` (:2734),
`parse_and_or?` (:2738), and the `;;`/`;&`/`;;&` stray-terminator `UnexpectedToken`
(:2747) — leave any body already pushed mid-parse (via
`collect_heredoc_bodies_after_newline`, :1427, called from `skip_newlines`) sitting in
the Lexer-owned `parsed_heredoc_bodies` Vec. Dormant today because every test builds a
fresh Lexer (`Lexer::new_live_atoms`); it becomes a real defect when the live-flip
driver reuses one Lexer across input lines after a parse error that pushed a body — the
next parse's `take_heredoc_bodies()` returns the stale body prepended, and
`fill_sequence` mis-binds it (or the surplus is silently dropped by the iterator). The
oracle has no analogous field (it collects bodies inline against the live token stream),
so this is an atom-path-only hazard.

**Chosen approach — entry-reset.** Discard `take_heredoc_bodies()` at the TOP of
`parse_sequence`, before `skip_newlines`:

```rust
pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    // v259 CF2: discard any heredoc bodies leaked by a prior parse that errored
    // after pushing (take_heredoc_bodies drains only on this fn's success path).
    // Safe: the atom parse_sequence is the single non-reentrant top-level entry.
    let _ = iter.take_heredoc_bodies();
    skip_newlines(iter)?;
    ...
```

Safe because the atom `parse_sequence` (single-arg) is the single non-reentrant
top-level entry — its only caller is the live-atoms harness/driver entry
(parser.rs:3937); compound bodies recurse via `parse_and_or` and the
`parse_*_sequence` family, never through `parse_sequence`. The reset is idempotent and
a no-op when the queue is already clean. (The two-argument `parse_sequence(iter,
stop_at)` in `command.rs` is the separate oracle function and is untouched.)

*Rejected:* draining on each early `return Err` — three-plus sites, easy to miss one on
a later edit; the single entry choke-point is more robust.

**Test.** A bespoke reused-Lexer regression test (the differential harness builds a
fresh Lexer per call, so it cannot express cross-call state). Build ONE Lexer via the
live-atoms constructor; run `parse_sequence` on an input that errors AFTER pushing a
heredoc body — `cat <<E\nx\nE\n;;` (the heredoc body is collected, then the `;;`
stray terminator triggers `UnexpectedToken`); then run a second `parse_sequence` for a
clean `echo hi` on the SAME Lexer and assert the result equals a fresh-Lexer parse of
`echo hi` (no leaked body attached). Without the fix the second parse sees the stale
body; with it the queue is clean.

## CF3 — even-bang before a compound (`parser.rs`)

**Root cause.** `finish_pipeline` (parser.rs:2429) applies the oracle's wrapping rule
but wraps a COMPOUND first-stage only when `negate` is true (arm :2448,
`cmd if negate => Pipeline{negate:true, [cmd]}`; the fallthrough `cmd => cmd` returns
the bare compound). `parse_pipeline` computes `negate = bangs % 2 == 1` (:2419), so for
an EVEN bang count `negate` is `false` and `! ! { a; }` falls to the bare-compound arm.
The oracle wraps ANY nonzero-bang compound in a one-element `Pipeline{negate, [cmd]}`
regardless of parity. Probed general across every compound type — brace, subshell, if,
while, for, case, `[[ ]]`, `(( ))`, coproc — all diverge identically (bare vs
`Pipeline{negate:false,[compound]}`); the odd-bang control `! { a; }` already agrees
(`Pipeline{negate:true,[BraceGroup]}`), and the zero-bang `{ a; }` already agrees
(bare). This is pre-existing general `finish_pipeline` behavior, not v257-introduced.

**Chosen approach.** Thread a `had_bangs: bool` into `finish_pipeline` and widen the
compound-wrap condition:

```rust
fn finish_pipeline(
    iter: &mut Lexer,
    first: Command,
    negate: bool,
    had_bangs: bool,
) -> Result<Command, ParseError> {
    ...
    if !matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
        return Ok(match first {
            Command::Simple(_)          => Command::Pipeline(Pipeline { negate, commands: vec![first] }),
            cmd if negate || had_bangs  => Command::Pipeline(Pipeline { negate, commands: vec![cmd] }),
            cmd                         => cmd,
        });
    }
    ...
}
```

`negate` stays correct parity, so an even-bang compound wraps as
`Pipeline{negate:false,[cmd]}` — matching the oracle. Callers (the only two, confirmed
by grep):
- `parse_pipeline` (:2423) passes `bangs > 0`.
- `parse_coproc_body` (:2369) passes `false` — a coproc body counts no leading
  pipeline-negation `!` (a leading `!` there is the program name, v257), so its existing
  `negate: false` and `had_bangs: false` leave compound coproc bodies returned as-is.

*Rejected:* wrapping in `parse_pipeline` before the `finish_pipeline` call —
`finish_pipeline` owns both the wrapping rule and the `|`-multi-stage loop, so the flag
belongs there; splitting the decision would duplicate the rule.

**Test.** `diff_cmd` for `! ! { a; }`, `! ! (a)`, `! ! if x; then y; fi`,
`! ! while x; do y; done`, `! ! for i in a; do y; done`, `! ! case x in a) :; esac`,
`! ! [[ x ]]`, `! ! (( 1 ))`, `! ! coproc cat`. Regression `diff_cmd` (must stay green):
`! { a; }` (odd → negate:true wrap), `{ a; }` (0-bang → bare), `! ! a` (simple → wraps).

## CF4 — `$"…"` locale quoting (`lexer.rs`)

**Root cause.** `emit_unquoted_dollar_atom` (lexer.rs:3854) — the unquoted `$`
classifier — has no `Some('"')` arm, so `$"` falls to the `_ =>` catch-all: it consumes
the lone `$` and emits `DollarLit { quoted: false }` (rendered as `Literal "$"`), after
which the scanner opens a bare double-quote span. Result: `[Literal "$",
Quoted{Double,…}]`. The oracle's `scan_dollar_expansion` (lexer.rs:5134) has
`Some('"') if !quoted => {}` — an empty arm that DROPS the `$` (pushes nothing) and
leaves the `"` unconsumed for the caller's normal double-quote handler, yielding
`Quoted{Double,…}` with no leading `$`. `$"…"` is bash locale-translation quoting;
huck has no message catalog, so the translation is the identity `$"…" ≡ "…"`.

**Chosen approach.** Add a `Some('"')` arm to `emit_unquoted_dollar_atom` that consumes
only the `$` and emits the zero-width `BeginDquote`, leaving the `"` unconsumed:

```rust
// `$"…"` — bash locale quoting; identity here, so `$"…" ≡ "…"`. Drop the `$`
// and emit BeginDquote (cursor left on `"`), exactly mirroring a bare `"`; the
// parser's Mode::DoubleQuote consumes the `"` and scans the body. (Oracle:
// scan_dollar_expansion's `Some('"') if !quoted => {}`.)
Some('"') => {
    self.cursor.next(); // consume `$` only
    self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
}
```

This produces exactly what a bare `"` at that position produces, so `$"hi"` →
`Quoted{Double,[Literal "hi" quoted:true]}`. It fixes BOTH command position and the
`=~` regex operand, because `scan_step_regex` reuses `emit_unquoted_dollar_atom`.

*Rejected:* consume `$` and emit no token — risks violating the "a `Step::Produced`
scan step emitted a token" expectation; emitting `BeginDquote` is the clean, in-contract
equivalent of a bare `"`.

**Scope note.** The INSIDE-double-quote case (`"a$"b"c"`) already matches the oracle on
both paths (a `$"` inside a double-quoted span is a literal `$`, handled by the separate
dquote `$` classifier) — verified by probe; no change there.

**Test.** `diff_cmd` for `echo $"hi"`, `echo $"a"$"b"` (multiple), `[[ $x =~ $"abc" ]]`
(regex operand). Regression `diff_cmd`: `echo "a$"b"c"` (inside-dquote, unchanged) stays
green.

## Architecture / files

- `crates/huck-syntax/src/parser.rs` — CF2 (entry-reset in `parse_sequence`) + CF3
  (`had_bangs` param on `finish_pipeline`; two call-site updates).
- `crates/huck-syntax/src/lexer.rs` — CF4 (one `Some('"')` arm in
  `emit_unquoted_dollar_atom`).
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` for CF3/CF4; a bespoke
  reused-Lexer test for CF2.
- `command.rs` diff-vs-main = EMPTY.
- Both `command_atoms` sites stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green (box is
  1 core/1.9GB — never `--workspace`, never multi-threaded).
- `cargo build -p huck-syntax` → 0 warnings.

## Task decomposition (SDD)

Three independent tasks (no ordering dependency; disjoint code):

- **T1 — CF2 (parser.rs, trivial):** entry-reset in `parse_sequence` + the bespoke
  reused-Lexer regression test.
- **T2 — CF3 (parser.rs, small):** add `had_bangs` to `finish_pipeline`, update the two
  call sites, widen the compound-wrap arm + the compound corpus + regressions.
- **T3 — CF4 (lexer.rs, small):** add the `Some('"')` arm to
  `emit_unquoted_dollar_atom` + the command/regex/dquote corpus.

## Live-flip carry-forwards

This iteration RESOLVES CF2, CF3, CF4 (removes them from the reconciliation list). No
new carry-forward is anticipated. After merge, mark them resolved in
`huck_carryforward_inventory.md` and record v259 in the iteration log. Remaining before
the finale: Iteration B (CF1 fill-walk) and Iteration C (CF6+CF7 arith quote sub-mode);
CF5/CF8/CF9/CF10 stay KEEP-INTENTIONAL. No `bash-divergences.md` change (dormant; the
oracle already handles all three).
