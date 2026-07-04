# v257 — `coproc [NAME] command` on the atom-command path

**Status:** design approved (2026-07-04; corrected 2026-07-04 after oracle probing)
**Arc:** Phase C, Stage 2 — porting deferred construct families onto the dormant
atom-command parser (`command_atoms`, default `false`), one family per iteration,
byte-identical to the `command.rs` oracle, marching toward the finale (flip
`command_atoms` live + delete the forward-scanning production scanners).

## Summary

Port the `coproc` coprocess compound onto the atom-command parser. Today the atom
compound-keyword dispatch (parser.rs:2085) has no `coproc` arm, so it falls through
to `UnsupportedCommand`. v257 adds `parse_coproc` + a restricted-pipeline body parser
+ a rest-stage guard so coproc parses byte-identically to the oracle.

**Dormant + differential.** The atom path builds the same
`Command::Coproc { name: String, body: Box<Command> }` AST as the oracle (name =
`"COPROC"` when anonymous). `command_atoms` stays `false`; no oracle LOGIC changes;
runtime coproc execution untouched.

**File scope (corrected):** primarily `parser.rs`, plus a purely-additive
`peek_nth_kind(n)` method on the lexer (a generalization of the existing
`peek_kind`/`peek2_kind`) and a `pub(crate)` visibility widening of
`valid_identifier_text` in `command.rs` (the v248/v253 pattern — no logic change).

**Scope (confirmed):** the full `coproc [NAME] command` — named + anonymous, simple +
compound bodies, the restricted-pipeline body semantics, trailing-redirect wrap, and
the pipeline-stage rejection. Last-but-one deferred family (only `$[ ]` remains).

## Background — the oracle grammar (probed)

`parse_coproc_command` (command.rs:1629), after `coproc` is consumed:
- **Named** = a valid-identifier `Word` NAME *immediately followed by a compound
  opener* (`is_compound_opener`, command.rs:1651: `{`, `(`/`((` (`Op(LParen)`),
  `if`/`while`/`until`/`for`/`case`/`select`, `[[`). Then consume NAME and parse the
  body via `parse_command_inner` → `Coproc { name: NAME, body }`.
- **Anonymous** = everything else → body = `parse_command_inner` →
  `Coproc { name: "COPROC", body }`.

The dispatch (command.rs:1055) wraps trailing redirects (`maybe_wrap_redirects`).

### The body is a RESTRICTED pipeline (`parse_command_inner`, command.rs:1016)

Probed oracle behavior — the body is NOT a plain command and NOT a plain pipeline:

| Input | Oracle `seq.first` |
|---|---|
| `coproc cat` | `Coproc{ Pipeline[cat] }` (bare simple → 1-stage Pipeline) |
| `coproc cat \| grep x` | `Coproc{ Pipeline[cat, grep x] }` (simple body **consumes** `\|`) |
| `coproc { a; } \| cat` | `Pipeline[ Coproc{BraceGroup}, cat ]` (compound body does **not** consume `\|`) |
| `coproc ! cat` | `Coproc{ Pipeline[Simple(program="!", args=[cat])] }` (**no** `!`-negation) |
| `coproc cat && echo y` | `Coproc{Pipeline[cat]}` + `&& echo y` (body stops at `&&`) |

So `parse_command_inner`: a **simple** command is extended into a pipeline (`|`
consumed, wrapped in `Pipeline`); a **compound** command is returned alone (`|` left
to the outer parser); a leading `!` is the program name, **not** negation.

### Top-level vs pipeline stage

The coproc ALLOW lives in `parse_command_inner` (command.rs:1055 — first stage /
top-level). Stages 2+ go through `parse_next_stage` (command.rs:2299), which REJECTS
coproc (command.rs:2344 → `UnexpectedKeyword("coproc")`). Because coproc is dispatched
as the outer pipeline's first stage and its compound body leaves a trailing `|`
unconsumed, `coproc { a; } | cat` naturally becomes `Pipeline[Coproc{BraceGroup},
cat]`; `echo x | coproc cat` errors.

## Architecture

**Files:**
- `crates/huck-syntax/src/lexer.rs` — add `peek_nth_kind(n)` (additive; wraps
  `fill_to(pos+n)` + `history.get(pos+n)`, exactly like `peek2_kind` with n=1).
- `crates/huck-syntax/src/command.rs` — widen `valid_identifier_text` to
  `pub(crate)` (visibility only; no logic change).
- `crates/huck-syntax/src/parser.rs` — `parse_coproc`, `peek_coproc_named`,
  `parse_coproc_body`, extract `finish_pipeline` from `parse_pipeline`, the dispatch
  arm, the rest-stage coproc guard (inside `finish_pipeline`), import
  `valid_identifier_text`, and tests.

### `parse_coproc(iter)` (returns the bare `Command::Coproc`)

```
consume_command_word(iter)?              // `coproc`
skip_test_blanks(iter)?                  // blanks only, NOT newlines (a newline → anonymous)
if peek_coproc_named(iter)? {
    let name_word = consume_command_word(iter)?;                 // the NAME (single bare Lit)
    let name = valid_identifier_text(&name_word).expect("peek verified");
    skip_test_blanks(iter)?;                                     // blanks between NAME and body
    let body = parse_coproc_body(iter)?;
    Ok(Command::Coproc { name, body: Box::new(body) })
} else {
    let body = parse_coproc_body(iter)?;                         // untouched stream
    Ok(Command::Coproc { name: "COPROC".into(), body: Box::new(body) })
}
```

### `parse_coproc_body(iter)` — the atom `parse_command_inner`

```
skip_newlines(iter)?;                     // mirror parse_command_inner's leading skip
let first = parse_command(iter)?;
if matches!(first, Command::Simple(_)) {
    finish_pipeline(iter, first, false)   // simple → extend to a pipeline (|-loop), never `!`-negate
} else {
    Ok(first)                             // compound → return alone; a trailing `|` is the OUTER pipeline's
}
```

The `Command::Simple(_)` discriminator reproduces `parse_command_inner`'s split (its
simple fall-through goes through `parse_pipeline_with_first`; every compound returns a
single command). `parse_command` does not strip `!` (that is `parse_pipeline`'s job),
so `coproc ! cat` yields `program="!"`.

### `finish_pipeline(iter, first, negate)` — extracted from `parse_pipeline`

`parse_pipeline` currently is: count leading `!` → `negate`; `first =
parse_command`; then post-first logic (skip trailing blanks; no `|` → wrap-or-return;
`|` → build `Pipeline` via the stage loop). Extract the post-first logic verbatim into
`finish_pipeline(iter, first, negate)`; `parse_pipeline` becomes `{ negate = count
bangs; first = parse_command; finish_pipeline(iter, first, negate) }`. Behavior
byte-unchanged (the existing pipeline suite is the regression net). The rest-stage
coproc guard is added inside `finish_pipeline`'s stage loop (below).

### `peek_coproc_named(iter)` — non-consuming (peek 0..3)

The caller has consumed `coproc` and skipped blanks, so `peek_kind()` is the first
significant token. Mirrors the oracle's `peek(Word valid_ident) &&
is_compound_opener(peek2)` without consuming:

```
let Some(TokenKind::Lit { text, quoted: false }) = iter.peek_kind()? else { return Ok(false) };
if valid_identifier_text(&single_lit_word(text)).is_none() { return Ok(false) }
match iter.peek2_kind()? {
    Some(TokenKind::Op(Operator::LParen)) => Ok(true),          // glued: `MYP(...)`
    Some(TokenKind::Blank) => match iter.peek_nth_kind(2)? {    // space then compound opener
        Some(TokenKind::Op(Operator::LParen)) => Ok(true),
        Some(TokenKind::Lit { text, quoted: false }) => match keyword_from_str(text) {
            Some(k) if is_compound_kw(k) =>                     // { if while until for case select [[
                Ok(is_word_boundary(iter.peek_nth_kind(3)?)),   // boundary after keyword (mirrors peek_leading_keyword)
            _ => Ok(false),
        },
        _ => Ok(false),
    },
    _ => Ok(false),                                             // continuation → multi-part word; other boundary → no opener
}
```

- `single_lit_word(text)` = `Word(vec![WordPart::Literal { text: text.clone(), quoted: false }])`.
- `is_compound_kw(k)` = `k ∈ {LBrace, If, While, Until, For, Case, Select,
  DoubleBracketOpen}` (NOT `Function`/`Coproc`, matching `is_compound_opener`).
- `is_word_boundary(t)` = `matches!(t, None | Some(Blank) | Some(Newline) | Some(Op(_))
  | Some(RedirFd(_)) | Some(Heredoc{..}))` (the `peek_leading_keyword` boundary set).

### Dispatch (parser.rs:2085)

Add, replacing the fall-through to `UnsupportedCommand`:
```rust
Some(Keyword::Coproc) => { let cmd = parse_coproc(iter)?; return maybe_wrap_redirects(cmd, iter); }
```

### Rest-stage guard (inside `finish_pipeline`'s stage loop)

Before each rest-stage `parse_command(iter)` (stages after the first `|`):
```rust
if peek_leading_keyword(iter)? == Some(Keyword::Coproc) {
    return Err(ParseError::UnexpectedKeyword("coproc".to_string()));
}
```
Mirrors `parse_next_stage`. Because `finish_pipeline` is shared, this covers both
top-level pipelines and coproc simple bodies. The first stage keeps allowing coproc
(the dispatch handles it).

### Progress / OOM safety

`peek_coproc_named` is bounded non-consuming lookahead (no loop). `parse_coproc`
consumes `coproc` then delegates. No mark/rewind, no speculation. Trivially
terminating.

## Differential corpus

All `diff_cmd` unless marked `diff_err`. Every value probed against the oracle.

**Anonymous simple → `Coproc{COPROC, Pipeline[…]}`:** `coproc awk prog`, `coproc foo
bar`, `coproc cat`, `coproc\ncat` (newline → anonymous; body skips the newline),
`coproc ! cat` (`program="!"`).
**Named compound → `Coproc{NAME, …}`:** `coproc MYP { read l; }` (BraceGroup),
`coproc M (echo hi)` (Subshell, spaced), `coproc M(echo hi)` (Subshell, glued),
`coproc M if x; then y; fi` (If).
**Anonymous compound → `Coproc{COPROC, …}`:** `coproc { read l; }`, `coproc (echo
hi)`, `coproc if x; then y; fi`.
**Body pipeline semantics:** `coproc cat | grep x` → `Coproc{Pipeline[cat, grep x]}`
(simple body consumes `|`); `coproc { a; } | cat` → `Pipeline[Coproc{BraceGroup},
cat]` (compound body leaves `|` to the outer pipeline); `coproc M { :; } | cat` →
`Pipeline[Coproc{M,BraceGroup}, cat]`; `coproc cat && echo y` → `Coproc{Pipeline[cat]}`
then `&& echo y`.
**Redirects:** `coproc cat >out` → `Coproc{Pipeline[Simple(cat, [>out])]}`; `coproc M
{ :; } >out` → `Coproc{M, Redirected{BraceGroup, [>out]}}`.
**Errors (`diff_err`):** `coproc 123 { :; }` → `UnexpectedKeyword("}")`; `echo x |
coproc cat` → `UnexpectedKeyword("coproc")`; `coproc` alone → `MissingCommand`;
`coproc |cat` → `MissingCommand`.

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` / `diff_err`.
- `command.rs` diff = ONLY the `valid_identifier_text` `pub(crate)` widening.
- `lexer.rs` diff = ONLY the additive `peek_nth_kind` method.
- The existing pipeline test suite is the regression net for the `finish_pipeline`
  extraction (byte-unchanged `parse_pipeline` behavior).
- Both `command_atoms` sites (lexer.rs:811, lexer.rs:4167) stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green.
- `cargo build -p huck-syntax` → 0 warnings.

## Task decomposition

- **T1 — enablers + core:** `peek_nth_kind` (lexer, additive); `valid_identifier_text`
  `pub(crate)` + import; extract `finish_pipeline` from `parse_pipeline` (behavior
  byte-unchanged — existing pipeline suite green). No new behavior; the pipeline suite
  is the gate.
- **T2 — parse_coproc + body + dispatch:** `peek_coproc_named`, `parse_coproc_body`,
  `parse_coproc`, the dispatch arm; corpus = named/anonymous × simple/compound, the
  body-pipeline semantics (`|` inside vs outside), `!`-body, newline-body, redirects;
  flip the pre-existing `coproc` deferral test sites (parser.rs:3914, 4305, 4852).
- **T3 — pipeline guard + errors + adversarial:** the rest-stage coproc guard in
  `finish_pipeline` + `echo x | coproc cat` / `coproc a | coproc b`; error parity
  (`coproc 123 { :; }`, `coproc`, `coproc |cat`); adversarial corpus (coproc inside
  compounds, nested, `&&`/`||`/`;` boundaries).

## Live-flip carry-forwards

None anticipated. Any divergence discovered during implementation is pinned with a
test and recorded in the ledger, per the v248–v256 convention. After v257 only `$[ ]`
remains deferred before the finale.
