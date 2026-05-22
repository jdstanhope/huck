# huck v20: `for` Loops — Design Spec

## Overview

huck has `if` (v17) and `while`/`until` (v18) compound commands. v20 adds the
third POSIX control-flow loop: `for`.

```
for NAME in WORD...; do
    LIST
done
```

The body runs once per word in the expanded list, with the loop variable
`NAME` set to each word in turn. `for` is a compound command at the sequence
level, exactly like `if` and `while` — it composes with `;`/`&&`/`||`, nests,
and (via v19) can be written across multiple input lines.

This is a clean instance of the established compound-command pattern: a new
`Command` AST variant, a recursive-descent `parse_for`, and an executor
`run_for` that mirrors `run_while`. The word list reuses the existing
argument-expansion machinery. The AST and executor are otherwise unchanged.

## Scope

**In scope:**

- `for NAME in WORD...; do LIST; done` — the standard form.
- The no-`in` form `for NAME; do LIST; done` (and `for NAME do LIST; done`):
  POSIX iterates the positional parameters; huck has none, so this runs the
  body zero times.
- The word list is expanded once, before the loop, through the same pipeline
  as command arguments: variable expansion, command substitution, arithmetic,
  tilde, parameter-expansion modifiers, word-splitting, and pathname
  (glob) expansion.
- `break` / `continue` (v18) inside a `for` body.
- SIGINT (Ctrl-C) interrupts a running `for` loop, status 130.
- Multi-line `for` (continuation lines), inherited from v19.

**Out of scope:**

- C-style `for ((init; cond; step))` — a bash extension, not POSIX.
- `select` loops.
- Positional parameters (`$@`/`$1`/...) — still unimplemented, which is why
  the no-`in` form is a zero-iteration no-op rather than an error.
- Redirections on the whole loop (`done > file`) — not implemented for `if`
  or `while` either; unchanged here.
- `for` inside a `|` pipeline, or backgrounding a whole `for` — not supported
  for `if`/`while` either; unchanged.

## Architecture

Three units, following v17/v18:

| Unit | File | Responsibility |
| --- | --- | --- |
| AST + keywords | `src/command.rs` | `Command::For`, `ForClause`, `Keyword::{For,In}` |
| Parser | `src/command.rs` | `parse_for` — recursive-descent parse of the construct |
| Executor | `src/executor.rs` | `run_for` — expand the list once, iterate the body |

## Section 1 — AST & keywords

### Keywords

`Keyword` gains two variants, `For` and `In`. `keyword_of` maps the bare,
unquoted, single-`Literal` words `"for"` and `"in"` to them; `Keyword::name`
gains the matching arms. This is identical to how `do`/`done`/`then` already
work.

Per the chosen approach, `in` becomes a reserved word: it is recognised as a
keyword wherever `keyword_of` is consulted (notably at command-start
position). It remains an ordinary word *inside* a `for` list — `for x in in
out; do …` iterates the two values `in` and `out`, because the word-list
reader (Section 2) stops only at `do`/`;`/newline, never at `in`. Running a
command literally named `in` is no longer possible; this matches bash, which
also fully reserves `in`.

### AST

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
}

pub struct ForClause {
    pub var: String,        // loop variable name — a validated identifier
    pub words: Vec<Word>,   // the unexpanded `in` list
    pub body: Sequence,     // the do…done body
}
```

- `words` holds the *unexpanded* `Word`s. Expansion happens once at run time
  in `run_for`, because it depends on shell state.
- The no-`in` form and an empty `in` list both produce `words: vec![]`. They
  are behaviourally identical (zero iterations), so the AST does not record
  which was written.
- `var` is a plain `String`, validated at parse time.
- The `Newline` token (v19) never reaches the AST, so a multi-line `for`
  produces the identical `ForClause` as its single-line form.

Every exhaustive `match` on `Command` gains a `For` arm; the compiler
enforces this.

## Section 2 — Parser

`parse_command` gains an arm: peeking `Keyword::For` dispatches to
`parse_for`, which returns `Command::For(Box::new(...))`.

### `parse_for`

1. Consume the `for` keyword.
2. **Read the loop variable.** Take the next token:
   - End of input → `ParseError::UnterminatedLoop` (incomplete — the v19
     classifier maps this to `Incomplete(Compound)`, so the REPL reads more).
   - A single, unquoted-`Literal` `Word` whose text is a valid identifier
     (`[A-Za-z_][A-Za-z0-9_]*`) and is *not* a reserved keyword → use its text
     as `var`.
   - Anything else (`for 2x`, `for $x`, `for "x"`, `for in`, `for ;`) →
     `ParseError::ForVariable`.
3. `skip_newlines` — POSIX permits a linebreak between the variable and `in`.
4. **Optional `in` and word list.** If the next token is `Keyword::In`:
   consume it, then collect the word list — repeatedly take `Word` tokens,
   stopping at `Keyword::Do`, `Token::Op(Semi)`, or `Token::Newline`. Any
   operator token (`|`, `&&`, `||`, `&`, redirections) encountered in the list
   → `ParseError::UnexpectedToken`. Keyword *words* such as `then` are
   collected as ordinary values; only `do` terminates the list. If the next
   token is not `in` (it is `do`/`;`/newline/end), this is the no-`in` form
   and `words` stays empty.
5. Consume any run of `Token::Op(Semi)` / `Token::Newline` separators, then
   `expect` `Keyword::Do` (missing → `ParseError::UnterminatedLoop`).
6. Parse the body with `parse_compound_section(iter, &[Keyword::Done],
   ParseError::UnterminatedLoop)` — the v19 helper, exactly as `parse_while`
   parses its body.
7. `expect` `Keyword::Done` (missing → `ParseError::UnterminatedLoop`).

### Errors

`for` is a loop, so a truncated construct or a missing `do`/`done` reuses
`ParseError::UnterminatedLoop`. Its v19 classifier mapping
(`UnterminatedLoop` → `Incomplete(Compound)`) means multi-line `for`
continuation works with no new classifier code.

One new variant is needed: `ParseError::ForVariable`, for an invalid or
missing loop variable name. Its message arm in `parse_error_message`
(`src/shell.rs`) reads `"invalid variable name in 'for' loop"`. An operator
inside the word list reuses the existing `ParseError::UnexpectedToken`.

### Backward compatibility

The only change to existing keyword handling is adding `For`/`In` to
`keyword_of` and `Keyword::name`. Single-line and multi-line `for` parse to
the identical `ForClause`. The v17/v18/v19 parser suites must remain green.

## Section 3 — Executor: `run_for`

`run_command` gains a `Command::For(clause) => run_for(clause, shell, sink)`
arm. `run_for` mirrors `run_while` and reuses the v18 `ExecOutcome` loop
machinery.

1. **Expand the word list once**, before iterating, via the same path used
   for command arguments:

   ```rust
   let mut values: Vec<String> = Vec::new();
   for word in &clause.words {
       values.extend(glob_expand_fields(expand(word, shell)));
   }
   ```

   So `for f in *.txt` globs, `for x in $list` word-splits, and `for n in
   $(seq 3)` substitutes — each exactly as the same words would behave as
   command arguments. An empty `words`, or words that all expand to nothing,
   give an empty `values` and thus zero iterations.

2. **Iterate.** `last` starts at `ExecOutcome::Continue(0)`. For each value:
   - Poll SIGINT with `sigint_flag.compare_exchange(true, false, Relaxed,
     Relaxed)`; if it was set, return `ExecOutcome::Continue(130)`.
   - Assign the loop variable: set `$var` to the current value in the same
     variable store a plain `NAME=value` assignment writes to (unexported
     unless the variable is already exported).
   - Run the body with `execute_sequence_body(&clause.body, shell, sink)`:
     - `ExecOutcome::Exit(code)` → return `ExecOutcome::Exit(code)`.
     - `ExecOutcome::LoopBreak` → set `last = Continue(0)`, stop iterating.
     - `ExecOutcome::LoopContinue` → set `last = Continue(0)`, go to the next
       value.
     - `ExecOutcome::Continue(c)` → set `last = Continue(c)`.
3. Return `last`.

### Behaviour

- `break` / `continue` propagate out of any nested `if`, are caught by the
  innermost loop, and a `for` nested in a `while` (or vice versa) breaks only
  the innermost loop — all inherited from the v18 `ExecOutcome` machinery.
- After the loop, `$var` retains the last value assigned (POSIX). A
  zero-iteration loop leaves `$var` untouched.
- Exit status: the last body command's status, or 0 for zero iterations or
  after a `break` — the same convention as `run_while`.
- An `exit` anywhere inside the body propagates out of the shell.
- The word list is expanded exactly once; values are not re-expanded per
  iteration.

The AST and every other executor function are unchanged.

## Error handling

| Situation | Behaviour |
| --- | --- |
| Invalid/missing loop variable (`for 2x`, `for in`, `for ;`) | `ParseError::ForVariable`, `huck: syntax error: …`, `$?` = 2 |
| Operator in the word list (`for x in a \| b`) | `ParseError::UnexpectedToken`, syntax error, `$?` = 2 |
| Missing `do` or `done`, or a truncated `for` | `ParseError::UnterminatedLoop`; at a REPL this is treated as incomplete and a continuation line is read |
| Empty / all-empty word list | Zero iterations, `$?` = 0, `$var` untouched |
| `break` / `continue` with no enclosing loop | Already handled (v18) — neutralised at the top level |
| Ctrl-C during a `for` loop | Loop stops, `$?` = 130 |

## Testing

**Parser unit tests** (`src/command.rs`) — `for x in a b c; do …; done` parses
to the expected `ForClause`; a multi-line `for` parses to the identical
`ForClause` as the single-line form; the no-`in` form and an empty `in` list
both yield empty `words`; `do` directly terminating the list (`for x in a b
do …`); an invalid variable name (`for 2x`, `for in`, `for $x`) →
`ParseError::ForVariable`; a truncated `for` (`for`, `for x`, `for x in a`) →
`ParseError::UnterminatedLoop`; an operator in the word list →
`ParseError::UnexpectedToken`.

**Executor unit tests** (`src/executor.rs`) — a `for` over a literal list runs
the body once per value in order; an empty list runs the body zero times with
exit status 0; `break` stops iteration early; `continue` skips to the next
value; the loop variable holds the last value after the loop; a nested loop
works. These reuse the v17/v18 executor test helpers plus a small new
`for`-clause constructor.

**Integration tests** (new `tests/for_integration.rs`) — piped end-to-end
scripts: `for` over a literal list; over a glob (`for f in *.txt`); over a
command substitution (`for n in $(…)`); word-splitting an unquoted `$var`; a
multi-line `for`; a `for` nested inside an `if` and inside a `while`;
`break`/`continue`; `$var`'s value observable after the loop; a
zero-iteration loop.

**No new PTY tests.** Multi-line `for` rides entirely on v19's continuation
mechanism, which already has PTY coverage for unterminated compound commands
generically. Piped multi-line `for` scripts in the integration suite exercise
the same path. This is consistent with v18 (`while`), which also added no PTY
tests.

**Regression** — all 733 existing tests stay green; the v17/v18/v19 suites
prove that adding `For`/`In` to `keyword_of` and a `Command::For` variant is
backward-compatible.
