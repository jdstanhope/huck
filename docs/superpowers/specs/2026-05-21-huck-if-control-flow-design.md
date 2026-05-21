# huck v17: `if` Control Flow

**Date:** 2026-05-21
**Status:** Design

## Goal

Add the `if` / `then` / `elif` / `else` / `fi` conditional construct.
This is huck's first compound command and the foundational
AST/parser/executor change that later control flow (`while`, `for`,
`case`, functions) will build on.

## Scope

**In scope:**
- `if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`
- Single-line form only — the whole `if` on one input line, parts
  separated by `;`
- `if` as a compound command at the **sequence level**: composable
  with `;`, `&&`, `||`, backgroundable with `&`, followed by further
  commands, and nestable inside branch bodies
- `&&` / `||` / `|` pipelines inside conditions and bodies
- Multi-command branch bodies (`then a; b; c; fi`)
- Positional keyword recognition by the parser (no lexer change)

**Out of scope (deferred):**
- Multi-line `if` with a continuation prompt — needs incomplete-input
  detection plus REPL line-accumulation; that machinery is a shared
  prerequisite for `while`/`for`/functions and is its own iteration
- `if` as a stage *inside* a `|` pipeline (`if ...; fi | grep`) —
  would require `Pipeline` to hold compound commands and the pipeline
  executor to run an `if` as a forked stage
- Backgrounding a whole `if` (`if ...; fi &`) — the background
  executor path is pipeline-specific; a trailing `&` after an `if`
  runs the `if` synchronously instead
- `while` / `until` / `for` / `case` / functions — later iterations
- Any lexer change

## Architecture

`if` is a compound command. The flat AST (`Sequence` of `Pipeline`)
becomes `Sequence` of `Command`, where a `Command` is either a
`Pipeline` or an `If`. The parser is rewritten as keyword-aware
recursive descent. The executor's sequence-runner dispatches each
`Command`, with a new `run_if` for the `If` case. No lexer change:
`if`/`then`/`elif`/`else`/`fi` are recognized positionally by the
parser.

### AST (`src/command.rs`)

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
}

pub struct IfClause {
    pub condition: Sequence,
    pub then_body: Sequence,
    pub elif_branches: Vec<ElifBranch>,
    pub else_body: Option<Sequence>,
}

pub struct ElifBranch {
    pub condition: Sequence,
    pub body: Sequence,
}

pub struct Sequence {
    pub first: Command,                    // was: Pipeline
    pub rest: Vec<(Connector, Command)>,   // was: (Connector, Pipeline)
    pub background: bool,
}
```

`Pipeline`, `SimpleCommand`, `ExecCommand`, `Redirect`, and
`Connector` are unchanged. `Command`, `IfClause`, `ElifBranch`, and
`Sequence` all derive `Debug, PartialEq, Eq, Clone`. The `Box` on
`Command::If` breaks the `Sequence` → `Command` → `IfClause` →
`Sequence` type cycle.

Nesting needs no extra types: an `IfClause`'s `condition` /
`then_body` / `else_body` are `Sequence`s, a `Sequence` holds
`Command`s, and a `Command` can be `If`.

### Parser (`src/command.rs`)

`pub fn parse(tokens) -> Result<Option<Sequence>, ParseError>`
remains the public entry. The parse layer becomes keyword-aware
recursive descent over the peekable token stream.

**Keyword recognition** is positional and quote-sensitive:

```rust
enum Keyword { If, Then, Elif, Else, Fi }

/// Returns the keyword a token represents, or None. A token is a
/// keyword only when it is `Token::Word(Word(parts))` with exactly
/// one part, an unquoted `Literal`, whose text equals the keyword.
fn keyword_of(token: &Token) -> Option<Keyword>;
```

So `'if'`, `"if"`, `i"f"`, and `if` used as a command argument all
remain ordinary words. Only a bare unquoted `if` starting a command
is the keyword.

**`parse_sequence(iter, stop_at: &[Keyword])`** parses `Command`s
joined by `;` / `&&` / `||` (and an optional trailing `&`), exactly
as the current top-level loop does, with one addition: before
parsing each command it peeks; if the next token is a keyword in
`stop_at`, it returns **without consuming** that keyword (it belongs
to the enclosing `if`). End-of-input also ends the sequence. A
sequence that parses zero commands yields `ParseError::MissingCommand`.

**`parse_command(iter)`** peeks the next token:
- a bare `if` keyword → `parse_if` → `Command::If`
- a `then` / `elif` / `else` / `fi` keyword (a keyword where a
  command must begin) → `ParseError::UnexpectedKeyword`
- otherwise → `parse_pipeline` → `Command::Pipeline`

**`parse_if(iter)`**:
1. Consume `if`.
2. `condition = parse_sequence(&[Then])`.
3. Expect and consume `then`; if the next token is not `then`
   (including end-of-input), return `ParseError::UnterminatedIf`.
4. `then_body = parse_sequence(&[Elif, Else, Fi])`.
5. While the next token is `elif`: consume it, parse a condition with
   `parse_sequence(&[Then])`, consume `then`, parse a body with
   `parse_sequence(&[Elif, Else, Fi])`, push an `ElifBranch`.
6. If the next token is `else`: consume it,
   `else_body = Some(parse_sequence(&[Fi]))`.
7. Expect and consume `fi`; if absent, return
   `ParseError::UnterminatedIf`.

**`parse_pipeline(iter)`** is essentially unchanged. It needs no
keyword awareness — keywords act as keywords only in command
position, which is exactly where `parse_sequence`'s pre-peek catches
them. A missing `;` (`if cond then ...`) makes `then` an ordinary
argument to the last condition command; the condition then runs to
end-of-input and `parse_if`'s "expect `then`" fails with
`UnterminatedIf` — the correct rejection.

**New `ParseError` variants:**

```rust
UnterminatedIf,            // ran out of tokens inside an `if`, or a
                           // required `then`/`fi` was absent
UnexpectedKeyword(String), // a then/elif/else/fi where a command
                           // was expected
```

`MissingCommand` is reused for an empty condition or body
(`if ; then ...`, `if x; then; fi`).

#### Parser walkthrough

`if test -f x; then echo yes; fi` (tokens: `if` `test` `-f` `x` `;`
`then` `echo` `yes` `;` `fi`):

1. `parse_if`: consume `if`. `parse_sequence(&[Then])` →
   `parse_pipeline` reads `test -f x`, stops at `;`; the sequence
   consumes `;`, peeks `then` ∈ stop → returns `Sequence(test -f x)`.
2. Consume `then`. `parse_sequence(&[Elif, Else, Fi])` → reads
   `echo yes`, stops at `;`; consumes `;`, peeks `fi` ∈ stop →
   returns `Sequence(echo yes)`.
3. No `elif`, no `else`; consume `fi`. Result: `Command::If` with an
   empty `elif_branches` and `else_body: None`.

### Executor (`src/executor.rs`)

The sequence-runner currently iterates `Pipeline`s connected by
`Connector`s, applying the connector logic (`&&` runs the next
command only when `$?` == 0, `||` only when `$?` != 0, `;`
unconditionally) and the background path. Two changes:

- The element type goes from `Pipeline` to `Command`. At each element
  it dispatches: `Command::Pipeline(p)` → the existing
  pipeline-execution code, unchanged; `Command::If(c)` → `run_if`.
- The connector logic is untouched — it reads `$?` after each
  `Command` regardless of kind, so `cmd && if ...; fi`,
  `if ...; fi || echo`, and `if ...; fi; echo` all work.

**`run_if(if_clause, shell) -> i32`** returns the exit status the
`if` leaves in `$?`:

```
status = run if_clause.condition          (a Sequence)
if status == 0:
    return run if_clause.then_body
for elif in if_clause.elif_branches:
    if (run elif.condition) == 0:
        return run elif.body
if let Some(else_body) = if_clause.else_body:
    return run else_body
return 0      // no branch matched and no else — bash leaves $? = 0
```

"run a Sequence" is the same sequence-runner, used recursively (a
branch body is a `Sequence` that may itself contain `Command::If`).
Running a `Sequence` yields the `$?` of its last command; `run_if`
returns that.

**Same-process execution.** A plain `if` is not forked — the
condition and the chosen branch run in the shell's own process, like
a top-level sequence. Builtins in the condition (notably `test` from
v16) run in-process; external commands fork as usual.

**Backgrounding an `if` is out of scope for v17.** The background
executor path (`run_background_sequence`) is pipeline-specific. The
`execute` entry point keeps calling it only when the sequence's
`first` is a `Command::Pipeline`; when `background` is set but
`first` is a `Command::If`, the executor falls through to synchronous
execution — the `&` has no effect. This is a documented limitation
for the rare `if ...; fi &` case; `run_background_sequence` itself is
unchanged.

**`$?` semantics.** When a branch body runs, its first command sees
`$?` set to the condition's exit status (bash behavior); nothing
resets `$?` between the condition and the branch, so this falls out
naturally. An `if` with no matching branch and no `else` leaves
`$?` = 0.

**Command substitution.** `$(...)` already runs a `Sequence` through
the executor, so an `if` inside `$(...)` works once the
sequence-runner handles `Command::If`.

## Data flow examples

`if test -f /etc/hostname; then echo present; else echo absent; fi`:
1. Parsed to a single `Command::If` with `condition` = the sequence
   `test -f /etc/hostname`, `then_body` = `echo present`,
   `else_body` = `Some(echo absent)`.
2. `run_if`: run the condition → `test` returns 0 → run `then_body`
   → prints `present`.

`if false; then echo a; elif test 1 -eq 1; then echo b; else echo c; fi`:
1. Condition `false` → non-zero → skip `then_body`.
2. `elif` condition `test 1 -eq 1` → 0 → run its body → prints `b`.

`if test -d /tmp; then echo yes; fi && echo done`:
1. The line is one `Sequence`: `first` = `Command::If(...)`, `rest` =
   `[(And, Command::Pipeline(echo done))]`.
2. `run_if` runs, prints `yes`, returns 0. The `&&` sees `$?` == 0 →
   runs `echo done`.

`echo if`:
1. `if` is the second word — an argument, not command position — so
   `keyword_of` is irrelevant; it stays an ordinary `Literal`. Prints
   `if`. (Regression case.)

## Error handling summary

| Input | Result |
|-------|--------|
| `if c; then b` (no `fi`) | `ParseError::UnterminatedIf` |
| `if c; then b; elif c2` (ends early) | `ParseError::UnterminatedIf` |
| `if c; fi` (no `then`) | error — `then` expected, `fi` found |
| bare top-level `then` / `elif` / `else` / `fi` | `ParseError::UnexpectedKeyword` |
| `if ; then b; fi` (empty condition) | `ParseError::MissingCommand` |
| `if c; then; fi` (empty body) | `ParseError::MissingCommand` |
| `if` condition fails, no `elif` match, no `else` | runs nothing; `$?` = 0 |
| any parse error | the line does not run; `$?` set non-zero |

## Testing

**`command.rs` parser unit tests:**
- `if c; then b; fi` → the expected `IfClause` (empty `elif_branches`,
  `else_body: None`)
- `if c; then b; else e; fi`; `if c; then b; elif c2; then b2; fi`; a
  two-`elif` chain
- `&&` / `||` and a `|` pipeline inside a condition; a multi-command
  body (`then a; b; fi`)
- `if` followed by another command (`if c; then b; fi; echo next`)
  and joined (`if c; then b; fi && echo`)
- a nested `if` inside a `then` body
- errors: `if c; then b` → `UnterminatedIf`; `if c; fi` → its error;
  bare top-level `then`/`fi` → `UnexpectedKeyword`; `if ; then b; fi`
  → `MissingCommand`
- regressions: a plain pipeline still parses (now wrapped in
  `Command::Pipeline`); `echo if` keeps `if` as an argument

**`executor.rs` unit tests:** `run_if` — a true condition runs
`then_body`; a false condition runs `else_body`; an `elif` chain
selects the correct body; no match and no `else` leaves `$?` = 0;
`$?` reflects the branch's last command.

**Integration tests (`tests/if_integration.rs`)** — end-to-end via
the shell binary:
- `if test -f <file>; then echo yes; else echo no; fi` for an
  existing file and a missing file
- an `elif` chain that selects the middle branch
- a multi-command body
- `if ...; fi && echo done` (composability)
- a nested `if`
- `$?` after an `if`; a syntax error (`if x; then y`) writes a
  message and runs nothing

## File layout impact

- **Modify:** `src/command.rs` — `Command` / `IfClause` /
  `ElifBranch`; `Sequence` restructure; the recursive-descent parser
  (`keyword_of`, `parse_sequence`, `parse_command`, `parse_if`,
  `parse_pipeline`); new `ParseError` variants; updated parser tests
- **Modify:** `src/executor.rs` — sequence-runner dispatches
  `Command`; new `run_if`
- **Modify:** `src/shell.rs` — `ParseError` display for
  `UnterminatedIf` and `UnexpectedKeyword`
- **New:** `tests/if_integration.rs`
- **Modify:** `README.md` — v17 row, features note, test count
- **No lexer change**; `expand.rs` only passes `Sequence` through and
  needs no logic change.

## Open questions

None at design time.

## References

- POSIX 2008 Shell Command Language §2.9.4 — the `if` conditional
  construct, reserved words, and command-position recognition
- bash(1) — Compound Commands; reserved words
