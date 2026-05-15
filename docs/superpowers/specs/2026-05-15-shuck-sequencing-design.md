# shuck — command sequencing (`&&`, `||`, `;`)

**Date:** 2026-05-15
**Status:** Approved
**Builds on:** `2026-05-14-shuck-pipes-redirection-design.md` (shuck v2: pipes
and redirection)

## Overview

This adds command-sequencing operators to `shuck`. A command line becomes a
sequence of pipelines joined by:

- `;`  — always run the next pipeline
- `&&` — run the next pipeline only if the previous succeeded (status 0)
- `||` — run the next pipeline only if the previous failed (non-zero status)

Each side of a sequencing operator is itself a full pipeline, so
`ls | grep foo && find . -name bar | wc -l` parses as two multi-stage
pipelines joined by `&&`.

## Goals

- Recognize `&&`, `||`, `;` outside quotes/escapes.
- Allow trailing `;` (bash-compatible); reject trailing `&&` and `||`.
- Reject leading sequencing operators and consecutive sequencing operators
  (`; ls`, `ls && && b`).
- The sequence's exit status is the status of the last pipeline that ran.
- `exit` short-circuits the rest of the sequence.
- No new dependencies.

## Non-goals (this version)

- `&` (background jobs). A lone `&` is rejected as a syntax error — this
  reserves the character for future use without committing to any semantics.
- `(...)` subshells, `{...}` group commands.
- Multi-line input / continuation lines.
- `$?` surfaced into user-visible state (still groundwork only).

## Architecture

The v2 data flow was `&str -> Vec<Token> -> Pipeline -> ExecOutcome`. This
version inserts one outer layer:

```
&str
  -> lexer::tokenize     -> Vec<Token>     (Word | Op)
  -> command::parse      -> Option<Sequence>   (or ParseError)
  -> executor::execute   -> ExecOutcome
```

Module boundaries are unchanged. Inside `command.rs` the existing parser
body is extracted as a private `parse_pipeline` helper; the public `parse`
becomes a thin outer loop. Inside `executor.rs` the previous public
`execute(&Pipeline)` becomes a private `run_pipeline`, and a new public
`execute(&Sequence)` drives the loop.

## Components

### lexer.rs

`Operator` gains three variants:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe, RedirOut, RedirAppend, RedirIn, RedirErr, RedirErrAppend,
    And,  // &&
    Or,   // ||
    Semi, // ;
}
```

State-machine additions, all outside quotes and not escaped:

- `|` — peek next: another `|` consumes it and emits `Op(Or)`; otherwise
  emit `Op(Pipe)`. (Symmetric with the existing `>` / `>>` lookahead.)
- `&` — new arm: peek next: another `&` consumes it and emits `Op(And)`.
  Otherwise return `Err(LexError::BareAmpersand)`.
- `;` — new arm: always emit `Op(Semi)`. No lookahead.

`LexError` gains `BareAmpersand`. Quoted (`"&&"`, `';'`) or escaped (`\&`,
`\;`, `\|`) operator characters stay literal words via the existing
quote/escape arms — no special handling needed.

### command.rs

Two new types are added alongside `Pipeline`/`Command`/`Redirect`/`ParseError`:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi, // ;
    And,  // &&
    Or,   // ||
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}
```

`parse` changes its return type from `Result<Option<Pipeline>, ParseError>`
to `Result<Option<Sequence>, ParseError>`. The existing `ParseError`
variants (`MissingCommand`, `MissingRedirectTarget`,
`RedirectTargetIsOperator`) cover every new error case — no new variants.

### Parser behavior

The existing parser body is extracted as a private helper:

```rust
fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError>;
```

`parse_pipeline` walks the iterator and returns one `Pipeline`. It treats
`Op(Pipe)` as an internal stage separator (current Task-3 v2 behavior). It
stops — **without consuming** — when it peeks `Op(Semi | And | Or)` or runs
out of input. If at end it has no `program`, it returns
`Err(ParseError::MissingCommand)`.

The public `parse` becomes:

- Empty token list → `Ok(None)`.
- Otherwise call `parse_pipeline` for the first pipeline.
- Loop while the iterator has more:
  - Consume one token: it must be `Op(Semi | And | Or)` (anything else
    means `parse_pipeline` returned early without good reason — but in
    practice it can only stop on these or end). Map to a `Connector`.
  - Trailing-`;` allowance: if the connector is `Semi` and `iter.peek()` is
    `None`, break out of the loop.
  - Otherwise call `parse_pipeline` and push `(connector, pipeline)`.
- Return `Ok(Some(Sequence { first, rest }))`.

Error cases fall out naturally:

| Input | Why it errors | Variant |
|-------|---------------|---------|
| `; ls` | `parse_pipeline` peeks `Semi` immediately, breaks, has no program | `MissingCommand` |
| `ls &&` | outer loop consumes `&&`, calls `parse_pipeline`, which finds end of input with no program | `MissingCommand` |
| `ls && && b` | second `parse_pipeline` peeks `&&` first, breaks, no program | `MissingCommand` |
| `ls > ;` | redirect operator's next-token check sees `Op(Semi)` | `RedirectTargetIsOperator` |
| `ls ; ` (trailing semicolon) | accepted; outer loop breaks at the trailing-`;` allowance | (no error) |

### executor.rs

Renames and one new function:

| Function | Visibility | Notes |
|---------|------------|-------|
| `execute(&Sequence) -> ExecOutcome` | `pub` | New top-level driver. |
| `run_pipeline(&Pipeline) -> ExecOutcome` | private | The body of the v2 `execute(&Pipeline)`, renamed. Dispatches single vs. multi-stage. |
| `run_single(&Command) -> ExecOutcome` | private | Unchanged. |
| `run_multi_stage(&[Command]) -> ExecOutcome` | private | Renamed from the v2 private `run_pipeline` (the multi-stage helper). |

`execute(&Sequence)`:

```text
status = run_pipeline(&seq.first)
if status is Exit(_): return status
for (conn, pipeline) in &seq.rest:
    should_run = match conn:
        Semi => true
        And  => status is Continue(0)
        Or   => status is Continue(c) where c != 0
    if should_run:
        status = run_pipeline(pipeline)
        if status is Exit(_): return status
return status
```

The loop is left-to-right and consults only the most recent status. For
`&&`/`||`/`;` this is equivalent to bash's actual precedence: `a ; b && c`
runs `a`, then evaluates `b && c`; `a && b ; c` evaluates `a && b`, then
runs `c`. No explicit precedence machinery is needed.

`Exit` from any pipeline short-circuits the entire sequence
(`exit 0 ; echo hi` exits before `echo` runs). `exit` appearing inside a
multi-stage pipeline remains a no-op per v2.

### shell.rs

Two small edits in `process_line`:

- The `Ok(Some(_))` arm now receives a `Sequence`; pass it to
  `executor::execute(&sequence)`.
- The `Err(LexError::...)` arm becomes a `match` over both variants,
  routed through a tiny `lex_error_message` helper parallel to the existing
  `parse_error_message`:

  ```rust
  fn lex_error_message(error: LexError) -> &'static str {
      match error {
          LexError::UnterminatedQuote => "unterminated quote",
          LexError::BareAmpersand => "unexpected '&'",
      }
  }
  ```

## Error handling

| Situation | Behavior |
|-----------|----------|
| Lone `&` | `shuck: syntax error: unexpected '&'`, `Continue(2)` |
| Leading `;`/`&&`/`||` | `shuck: syntax error: expected a command`, `Continue(2)` |
| Trailing `&&` or `||` | `shuck: syntax error: expected a command`, `Continue(2)` |
| Trailing `;` | accepted, no error |
| Double sequencing op (`a && && b`) | `shuck: syntax error: expected a command`, `Continue(2)` |
| Redirect target is sequencing op (`ls > ;`) | `shuck: syntax error: expected a filename after redirection`, `Continue(2)` (already covered) |
| `exit` in a sequence | rest of sequence is skipped |

Every error path returns to the prompt — the shell never crashes on a bad
line.

## Testing

- **Lexer unit tests:**
  - `&&` → `Op(And)`; `||` → `Op(Or)`; `;` → `Op(Semi)`.
  - Single `|` still emits `Op(Pipe)` (regression).
  - Bare `&` (e.g. `a & b`) → `Err(LexError::BareAmpersand)`.
  - Quoted `"&&"`, `"||"`, `";"` stay `Word` tokens.
  - Escaped `\&`, `\;`, `\|` stay `Word` tokens.
  - Combined `a && b || c ; d` tokenizes correctly.

- **Parser unit tests:**
  - `a ; b` → `Sequence { first: a, rest: [(Semi, b)] }`.
  - `a && b` → `Sequence { first: a, rest: [(And, b)] }`.
  - `a || b` → `Sequence { first: a, rest: [(Or, b)] }`.
  - Mixed `a && b || c ; d` → three connectors in `rest`.
  - **`ls | grep foo && find . -name bar | wc -l`** — a sequence whose
    `first` and second pipeline are both multi-stage. Verifies
    `parse_pipeline` keeps `|` internal and only breaks on sequencing ops.
  - Pipeline-with-redirect inside a sequence (`echo hi > f ; cat f`).
  - Trailing `;` accepted: `a ;` parses as `Sequence { first: a, rest: [] }`.
  - Trailing `&&` → `Err(MissingCommand)`.
  - Trailing `||` → `Err(MissingCommand)`.
  - Leading `;` (`; a`) → `Err(MissingCommand)`.
  - Double connector (`a && && b`) → `Err(MissingCommand)`.
  - Redirect target is sequencing op (`ls > ;`) → `Err(RedirectTargetIsOperator)`.

- **Executor:** manual smoke testing (consistent with v1/v2):
  - `true && echo a` prints `a`.
  - `false && echo a` prints nothing.
  - `false || echo b` prints `b`.
  - `true || echo c` prints nothing.
  - `echo a ; echo b` prints `a` then `b`.
  - `false || echo x | tr a-z A-Z` prints `X` (left-to-right: false fails,
    `||` runs the next pipeline `echo x | tr a-z A-Z`).
  - `ls | grep nonexistent && echo found || echo nope` prints `nope`.
  - `exit 0 ; echo unreached` exits without printing `unreached`.
  - Pipeline exit status of a sequence is the last pipeline's status
    (verify via EOF exit code, as in v2).

## Future extensions (still not in scope)

- `&` (background jobs) and job control.
- `( ... )` subshells and `{ ... }` group commands.
- `$?` and other parameter expansion.
- Multi-line / continuation input.
