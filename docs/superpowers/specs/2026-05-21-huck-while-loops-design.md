# huck v18: `while` / `until` Loops

**Date:** 2026-05-21
**Status:** Design

## Goal

Add the `while` and `until` loop constructs, plus the `break` and
`continue` builtins. This is huck's second and third compound
command, built on the AST/parser/executor pattern v17 established for
`if`.

## Scope

**In scope:**
- `while LIST; do LIST; done` — run the body while the condition's
  exit status is 0
- `until LIST; do LIST; done` — run the body while the condition's
  exit status is non-zero
- `break` — exit the innermost loop
- `continue` — skip to the innermost loop's next iteration
- Ctrl-C interruptibility — an infinite loop (`while true; do …;
  done`) is escapable
- Single-line form only (parts separated by `;`)
- A `while`/`until` is a sequence-level compound command — composes
  with `;`/`&&`/`||`, nests, and can be followed by more commands

**Out of scope (deferred):**
- Multi-line compound commands (continuation prompt) — a separate
  iteration (v19) that adds REPL line-accumulation and incomplete-
  input detection, retrofitting `if` and covering `while`
- `break N` / `continue N` (numeric level argument)
- `for` / `case` / functions — later iterations
- `while` / `until` as a stage inside a `|` pipeline, and
  backgrounding a whole loop (`while …; done &`) — same limitations
  as `if` in v17
- Any lexer change

## Architecture

`while`/`until` are compound commands. They follow the v17 pattern
exactly: a new `Command` variant, a `parse_while` recursive-descent
function, and a `run_while` executor function. `break`/`continue` add
loop-control signalling: two new `ExecOutcome` variants that the
`break`/`continue` builtins produce, that propagate through the
executor like `Exit`, and that `run_while` catches. No lexer change —
`while`/`until`/`do`/`done` are recognized positionally by the parser.

### AST (`src/command.rs`)

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),       // new
}

pub struct WhileClause {
    pub condition: Sequence,
    pub body: Sequence,
    pub until: bool,               // false = `while`, true = `until`
}
```

`while` and `until` share one type; they differ only in the polarity
of the condition test, captured by `until`. `WhileClause` derives
`Debug, PartialEq, Eq, Clone`. The `Box` on `Command::While` keeps
`Command` finite. Nesting needs no extra types — a `WhileClause`'s
`condition`/`body` are `Sequence`s, which hold `Command`s, which can
be `While` or `If`.

### Parser (`src/command.rs`)

No lexer change. The `Keyword` enum gains four variants:

```rust
enum Keyword { If, Then, Elif, Else, Fi, While, Until, Do, Done }
```

`keyword_of` extends with `"while"`/`"until"`/`"do"`/`"done"`;
`Keyword::name()` gains their names.

**`parse_command`** dispatches on the peeked keyword:
- `if` → `parse_if`
- `while` / `until` → `parse_while`
- any other keyword (`then`/`elif`/`else`/`fi`/`do`/`done`) where a
  command must begin → `ParseError::UnexpectedKeyword`
- otherwise → `parse_pipeline`

**`parse_while`**:
1. Consume the leading keyword — `while` or `until` — recording which
   into `until`.
2. `condition = parse_sequence(&[Do])`.
3. Expect-and-consume `do`.
4. `body = parse_sequence(&[Done])`.
5. Expect-and-consume `done`.
6. Return `WhileClause { condition, body, until }`.

`do`/`done` terminate the inner sequences via the same `stop_at`
mechanism `parse_if` uses.

**`expect_keyword` gains an error parameter.** v17's `expect_keyword`
hardcodes `ParseError::UnterminatedIf` on a missing keyword. It
becomes `expect_keyword(iter, kw, on_missing: ParseError)`: `parse_if`
passes `UnterminatedIf`, `parse_while` passes a new
`ParseError::UnterminatedLoop`. `ParseError`'s variants are unit/cheap,
so each call constructs a fresh value — no `Clone` needed.

**New `ParseError` variant:** `UnterminatedLoop` — a `while`/`until`
ran out of tokens, or a required `do`/`done` was absent.

**The stray-keyword guard generalizes for free.** v17's review fix
made `parse_sequence`'s outer loop turn any unconsumed keyword into
`UnexpectedKeyword` rather than panicking. A misplaced `do`/`done`
(e.g. a bare `done`, or `do` outside a loop) flows through that same
guard — no new code.

As with `if`: `&` inside a `while` condition or body is rejected as
`UnexpectedBackground`; a `while`/`until` composes with `;`/`&&`/`||`,
nests, and is followed by other commands; single-line only.

### `break` / `continue` machinery (`src/executor.rs`, `src/builtins.rs`)

`ExecOutcome` gains two variants:

```rust
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak,        // new — `break`
    LoopContinue,     // new — `continue`
}
```

(`LoopBreak`/`LoopContinue`, not `Break`/`Continue`, because
`Continue` already names the shell-keeps-running variant — these are
distinct concepts.)

**Builtins.** `break` and `continue` join `BUILTIN_NAMES` and the
`run_builtin` dispatch. `builtin_break` returns
`ExecOutcome::LoopBreak`; `builtin_continue` returns
`ExecOutcome::LoopContinue`. Builtins already return `ExecOutcome`.
Extra arguments are ignored (no `break N` in v18).

**Propagation — three touch points:**

1. `execute_sequence_body` short-circuits on `LoopBreak`/`LoopContinue`
   exactly as it does on `Exit`: when a command in a sequence returns
   one, the rest of the sequence is abandoned and the signal is
   returned. So `echo a; break; echo b` runs `echo a`, then `break`
   stops the sequence — `echo b` does not run.
2. `run_if` propagates `LoopBreak`/`LoopContinue` exactly as it
   propagates `Exit` — a `break` inside an `if` inside a `while` must
   reach the `while`.
3. `run_while` catches them (see below).

**Stray `break`/`continue` outside any loop.** If no `run_while`
catches the signal it reaches the top level — `execute` /
`process_line` receives a `LoopBreak`/`LoopContinue` outcome and
neutralizes it to `Continue(0)` (a stray `break` is harmless, matching
bash's lenient behavior). No crash, no error.

**Match-site ripple.** Adding two `ExecOutcome` variants makes
exhaustive `match`es non-exhaustive — the compiler flags each.
Non-exhaustive `matches!(x, ExecOutcome::Continue(0))`-style guards
are unaffected. Genuine exhaustive matches (in `execute_sequence_body`,
`execute_capturing`'s status extraction, and where `process_line`
maps an outcome to `$?`) get explicit arms — `LoopBreak`/
`LoopContinue` map to status 0 wherever a bare status is needed.

### Executor — `run_while` (`src/executor.rs`)

`run_command` gains a `Command::While(c) => run_while(c, shell, sink)`
arm.

```
run_while(clause, shell, sink) -> ExecOutcome:
    last = Continue(0)
    loop:
        if shell.sigint_flag is set:
            clear it; return Continue(130)        # Ctrl-C escape
        cond = execute_sequence_body(&clause.condition, ...)
        match cond:
            Exit(_) | LoopBreak | LoopContinue -> return cond   # propagate
            Continue(c):
                keep_going = if clause.until { c != 0 } else { c == 0 }
                if not keep_going: break
        body = execute_sequence_body(&clause.body, ...)
        match body:
            Exit(_)      -> return body          # `exit` ends the shell
            LoopBreak    -> last = Continue(0); break
            LoopContinue -> last = Continue(0); continue
            Continue(c)  -> last = Continue(c)
    return last
```

- The **`until` flag** inverts only the condition test: `while` loops
  while the condition exits 0; `until` loops while it exits non-zero.
- **Ctrl-C interruptibility:** each iteration checks
  `shell.sigint_flag` — the `Arc<AtomicBool>` the SIGINT handler sets,
  the same flag `wait` has polled since v6. An infinite
  `while true; do …; done` is escapable; the loop exits with status
  130 (the conventional "terminated by SIGINT" status). The flag is
  cleared so the shell continues normally afterward.
- **`break`/`continue` in the body** are caught here: `break` ends the
  loop, `continue` jumps to the next condition test.
- **Loop exit status** is the body's last command's status, or 0 if
  the body never ran or the loop ended via `break`/`continue` —
  matching bash.
- A loop signal arriving from the *condition* is exotic; `run_while`
  propagates it (returns it) so an outer loop can catch it or the top
  level can neutralize it.

A plain `while`/`until` runs in the shell's own process — no fork.
Builtins in the condition or body run in-process; external commands
fork as usual.

### `shell.rs`

- Add a parse-error message for `ParseError::UnterminatedLoop`:
  `": unterminated loop (expected 'do'/'done')"` (matching the style
  of the `UnterminatedIf` arm).
- Where `process_line` maps the final `ExecOutcome` to a status, a
  top-level `LoopBreak`/`LoopContinue` is treated as `Continue(0)`.

## Data flow examples

`i=0; while test $i -lt 3; do echo $i; i=$((i+1)); done`:
1. Parsed as two top-level commands: the assignment, then a
   `Command::While` with `until: false`.
2. `run_while`: each iteration runs `test $i -lt 3`; while it exits 0,
   the body runs `echo $i` and `i=$((i+1))`. After three iterations
   `$i` is 3, the condition exits 1, the loop ends. Output: `0 1 2`.

`until test -f /tmp/ready; do sleep 1; done`:
1. `WhileClause { until: true }`. The body runs while `test -f` exits
   non-zero — i.e. until the file exists.

`while true; do echo x; if test 1 -eq 1; then break; fi; done`:
1. `run_while` loops; the body prints `x`, then the inner `if` runs
   `break`. `break` returns `ExecOutcome::LoopBreak`; `run_if`
   propagates it; `execute_sequence_body` (the body) returns it;
   `run_while` catches `LoopBreak` and ends the loop after one
   iteration.

`while true; do echo x; done` then Ctrl-C:
1. The body prints `x` repeatedly. Ctrl-C sets `shell.sigint_flag`;
   `run_while`'s next per-iteration check sees it, clears it, and
   returns `Continue(130)`. The shell returns to its prompt.

## Error handling summary

| Input | Result |
|-------|--------|
| `while c; do b` (no `done`) | `ParseError::UnterminatedLoop` |
| `while c; done` (no `do`) | `do` expected, `done` found → `UnexpectedKeyword` |
| bare `do` / `done` | `ParseError::UnexpectedKeyword` |
| `while ; do b; done` (empty condition) | `ParseError::MissingCommand` |
| `&` inside a `while` condition/body | `ParseError::UnexpectedBackground` |
| `break` / `continue` outside any loop | no-op, status 0 |
| infinite loop + Ctrl-C | loop ends, status 130 |
| any parse error | the line does not run; `$?` set non-zero |

## Testing

**`command.rs` parser unit tests:**
- `while c; do b; done` → `WhileClause { until: false, .. }`
- `until c; do b; done` → `until: true`
- `&&`/`||` in a condition; a multi-command body
- a `while` followed by another command, and joined with `&&`
- a nested loop; a loop inside an `if` body and an `if` inside a loop
  body
- errors: missing `do` → `UnexpectedKeyword`; missing `done` →
  `UnterminatedLoop`; bare `do`/`done` → `UnexpectedKeyword`;
  `while ; do b; done` → `MissingCommand`; `&` inside a loop →
  `UnexpectedBackground`
- regression: `echo while` keeps `while` as an argument

**`executor.rs` unit tests:** `run_while` — a self-terminating
`while` runs the body the right number of times; `until` polarity;
`break` ends the loop; `continue` skips an iteration; loop exit
status reflects the body's last command; `break` propagating out of a
nested `if`.

**`break`/`continue` builtin tests:** `run_builtin("break", …)`
returns `ExecOutcome::LoopBreak`; `run_builtin("continue", …)`
returns `ExecOutcome::LoopContinue`.

**Integration tests (`tests/while_integration.rs`)** — end-to-end via
the shell binary:
- a counting `while` loop printing `0 1 2`
- an `until` loop
- `break` exits early; `continue` skips
- a nested loop
- `break` from inside an `if` inside a `while`
- a stray top-level `break` is a no-op (the shell stays alive)
- a `while`-loop syntax error writes a message and runs nothing

## File layout impact

- **Modify:** `src/command.rs` — `Command::While`, `WhileClause`,
  four `Keyword` variants, `parse_while`, `expect_keyword` error
  parameter, `ParseError::UnterminatedLoop`
- **Modify:** `src/executor.rs` — `ExecOutcome::LoopBreak`/
  `LoopContinue`, `run_while`, `run_command` dispatch, loop-signal
  short-circuiting in `execute_sequence_body`, propagation in
  `run_if`, exhaustive-match fixes
- **Modify:** `src/builtins.rs` — `break`/`continue` in
  `BUILTIN_NAMES`, dispatch arms, `builtin_break`/`builtin_continue`
- **Modify:** `src/shell.rs` — `UnterminatedLoop` display; neutralize
  a top-level loop signal
- **New:** `tests/while_integration.rs`
- **Modify:** `README.md` — v18 row, features note, builtins list,
  test count
- **No lexer change.**

## Open questions

None at design time.

## References

- POSIX 2008 Shell Command Language §2.9.4 — the `while` and `until`
  loops
- bash(1) — Compound Commands; the `break` and `continue` builtins
