# v78 — C-Style `for ((init; cond; step))` + Standalone `((expr))` Design Spec

**Date**: 2026-06-02
**Iteration**: v78
**Divergences closed**: M-23 (C-style arithmetic for-loop). Adds a new
fixed entry for the previously-unenumerated standalone `((expr))`
arithmetic command, since the lexer infrastructure to recognize `((`
is shared between both features.

## Goal

Add bash's two arithmetic-block command forms:

1. **`for ((init; cond; step)) do BODY done`** — C-style counter loop.
2. **`((expr))`** as a standalone command — evaluates `expr` and exits
   0 if the result is non-zero, 1 if zero. Common idiom in `if ((x >
   0)); then ...`.

Both use bash's `((` / `))` block syntax (no whitespace inside the
opener pair).

## Non-goals

These are deferred (M-23 follow-on entries in
`docs/bash-divergences.md` after merge):

- The `let EXPR ...` builtin variant. Bash supports `let "x = x + 1"`
  as an alternative to `((x = x + 1))`. huck already has a partial
  `let` (verify scope); this spec does NOT add or extend it.
- Floating-point arithmetic inside `((...))`. huck's arith module is
  integer-only; bash is too in `((...))`. No change here.

## Behavior change (one)

`((cmd))` (no whitespace inside the opener) currently parses in huck
as a nested subshell `( (cmd) )` per M-11 (v11). v78 changes this to
arith-block parsing — bash-aligning. `( (cmd) )` (with whitespace)
continues to parse as nested subshell.

Documented in the v78 change-log entry and called out in the M-11
entry as a parenthetical update.

## Architecture overview

Three files touched, each in a localized region:

| File | What changes |
|---|---|
| `src/lexer.rs` | Recognize `((` at command-position (no-space rule). Scan until matching `))` (tracking paren depth). Emit one new token `Token::ArithBlock(String)` containing the raw text between the parens. |
| `src/command.rs` | Two new AST variants: `Command::Arith(ArithExpr)` (standalone) and `Command::ArithFor(Box<ArithForClause>)` (loop). New `parse_arith_for_header(text)` helper that splits header text on top-level `;` and `arith::parse`s each section. New dispatch in `parse_command` for `Token::ArithBlock`. Extension to `parse_for` for the for-loop variant. Two new `ParseError` variants. |
| `src/executor.rs` | New `run_arith(expr, shell) -> ExecOutcome`. New `run_arith_for(clause, shell, sink) -> ExecOutcome` that mirrors `run_for`'s break/continue/return/exit/SIGINT handling. |

No changes to expand, builtins, shell_state, traps, or other modules.

## Lexer changes

### When `((` triggers arith-block mode

The lexer enters arith-block mode when **all** of these are true:

1. Current char is `(`.
2. Next char (no whitespace consumed between) is `(`.
3. The current position is "command-start" — that is, the lexer was
   about to read a new command word. Command-start positions are:
   - Beginning of input.
   - After a separator (`;`, `&`, `&&`, `||`, `|`, newline).
   - After a compound-command keyword (`if`, `then`, `else`, `elif`,
     `do`, `done`, `;`, `(`, `{`).

The "command-start" requirement prevents `echo (1+2))` from being
ambiguous (it's not — `(` in arg position is already a syntax error
per existing rules). Realistically the lexer can simply check the
no-space rule + the lookahead and emit `ArithBlock` whenever both
hold; the parser will reject `ArithBlock` in non-command positions.

### Scanning rules inside the block

After consuming the opening `((`:

```
depth = 0  (we're inside the outer pair; depth counts NESTED parens)
collected = String::new()
loop:
    char = next()?
    if char == '(':
        depth += 1
        collected.push('(')
    else if char == ')':
        if depth == 0 and peek() == ')':
            next()  // consume second `)`
            return Token::ArithBlock(collected)
        depth -= 1
        collected.push(')')
    else:
        collected.push(char)
```

- Backslash-newline inside the block: handled by the outer continuation
  classifier (same as for `$(( ... ))` — the line-joiner runs before
  the lexer sees the input).
- Unclosed block (EOF before matching `))`): emit `LexError::UnterminatedArithBlock`.
- Empty block `(())`: lexer emits `Token::ArithBlock("")`. Parser/arith
  module reject downstream (empty header in `for` is rejected at parse
  time; empty standalone `(())` is rejected by `arith::parse`).

The block text is captured raw. No interpolation or sub-tokenization
happens at this level — `arith::parse` already handles `$var`, quoting,
numeric literals, operators, etc.

### Disambiguation

`( (cmd) )` (with whitespace between the `(`s) continues to parse as
nested subshell. The lexer treats whitespace as significant for this
disambiguation: if any whitespace separates the two `(`s, the first
`(` tokenizes as `Op(LParen)` normally.

## Parser changes (`src/command.rs`)

### New AST

```rust
pub enum Command {
    // ... existing variants ...
    Arith(crate::arith::ArithExpr),
    ArithFor(Box<ArithForClause>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArithForClause {
    pub init: Option<crate::arith::ArithExpr>,
    pub cond: Option<crate::arith::ArithExpr>,
    pub step: Option<crate::arith::ArithExpr>,
    pub body: Sequence,
}
```

### New `ParseError` variants

```rust
pub enum ParseError {
    // ... existing ...
    ArithBlock(String),       // arith::parse failed inside `((...))`
    ArithForHeader(String),   // `for ((header))` header is not 3 `;`-separated sections
}
```

The `String` payload carries the inner arith error message for diagnostics.

### Dispatch

`parse_command` gains a new arm BEFORE the `Token::Op(LParen)` subshell
arm:

```rust
match iter.peek() {
    Some(Token::ArithBlock(_)) => {
        let Some(Token::ArithBlock(text)) = iter.next() else { unreachable!() };
        let expr = crate::arith::parse(&text)
            .map_err(|e| ParseError::ArithBlock(e.to_string()))?;
        return Ok(Command::Arith(expr));
    }
    // ... existing arms ...
}
```

### `parse_for` extension

After consuming the `for` keyword, before reading the loop variable,
peek for `Token::ArithBlock`:

```rust
expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
    return Ok(parse_arith_for_clause(iter)?);  // returns ArithForClause
}

// ... existing POSIX-form path ...
```

The dispatch returns an `ArithForClause`, and the outer call site
constructs `Command::ArithFor(Box::new(clause))`. The exact return type
plumbing (e.g., changing `parse_for` to return an enum) is left to the
implementer; the constraint is that callers of `parse_for` in two
places (`parse_command` line ~659 and `parse_command_or_keyword_pipeline`
line ~1454) work for both POSIX and arith variants.

Simplest shape: factor a top-level `parse_for_command(iter) ->
Result<Command, ParseError>` that returns either `Command::For(...)`
or `Command::ArithFor(...)` and update both call sites to use it.

### `parse_arith_for_clause(iter)` helper

```rust
fn parse_arith_for_clause<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ArithForClause, ParseError> {
    // 1. Consume the ArithBlock token; parse the header.
    let Some(Token::ArithBlock(header_text)) = iter.next() else {
        unreachable!("caller verified peek");
    };
    let (init, cond, step) = parse_arith_for_header(&header_text)?;

    // 2. Skip separators; expect `do`.
    while matches!(iter.peek(), Some(Token::Op(Operator::Semi)) | Some(Token::Newline)) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    // 3. Body and `done`.
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;

    Ok(ArithForClause { init, cond, step, body })
}
```

### `parse_arith_for_header(text)` helper

Splits `text` on top-level `;` (respecting paren depth so `(a;b)`
inside the header isn't split), trims whitespace from each section,
and parses each (or returns `None` for empty) via `arith::parse`.

```rust
fn parse_arith_for_header(
    text: &str,
) -> Result<(Option<ArithExpr>, Option<ArithExpr>, Option<ArithExpr>), ParseError> {
    let sections = split_top_level_semi(text);  // Vec<String>
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}", sections.len()
        )));
    }
    let parse_section = |s: &str| -> Result<Option<ArithExpr>, ParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            crate::arith::parse(trimmed)
                .map(Some)
                .map_err(|e| ParseError::ArithBlock(e.to_string()))
        }
    };
    Ok((
        parse_section(&sections[0])?,
        parse_section(&sections[1])?,
        parse_section(&sections[2])?,
    ))
}

fn split_top_level_semi(text: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut depth = 0i32;
    for c in text.chars() {
        match c {
            '(' => { depth += 1; out.last_mut().unwrap().push(c); }
            ')' => { depth -= 1; out.last_mut().unwrap().push(c); }
            ';' if depth == 0 => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}
```

Note: bash's actual parser is more permissive about whitespace inside
the header (newlines, etc.). v78 accepts the standard form
`init; cond; step` with each section being an optional arith expression.
Edge cases like `for ((;;))` (all empty) work because `split_top_level_semi`
emits 3 empty strings.

## Executor changes (`src/executor.rs`)

### `run_arith(expr, shell)`

```rust
fn run_arith(expr: &ArithExpr, shell: &mut Shell) -> ExecOutcome {
    match crate::arith::eval(expr, shell) {
        Ok(0) => ExecOutcome::Continue(1),
        Ok(_) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("huck: ((: {e}");
            ExecOutcome::Continue(1)
        }
    }
}
```

Bash's semantics: exit 0 if the result is non-zero, 1 if zero. Arith
error → exit 1 with diagnostic.

### `run_arith_for(clause, shell, sink)`

Mirrors `run_for`'s control-flow handling:

```rust
fn run_arith_for(clause: &ArithForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // 1. Eval init once (if present).
    if let Some(init) = &clause.init {
        if let Err(e) = crate::arith::eval(init, shell) {
            eprintln!("huck: ((: {e}");
            return ExecOutcome::Continue(1);
        }
    }

    let mut last = ExecOutcome::Continue(0);
    loop {
        // SIGINT check.
        if shell.sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }

        // 2. Eval cond. Empty cond = always true.
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::arith::eval(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("huck: ((: {e}");
                    return ExecOutcome::Continue(1);
                }
            },
        };
        if cond_value == 0 {
            break;
        }

        // 3. Execute body.
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through to step
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }

        // 4. Eval step (if present).
        if let Some(step) = &clause.step {
            if let Err(e) = crate::arith::eval(step, shell) {
                eprintln!("huck: ((: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    }
    last
}
```

Wire `run_arith` and `run_arith_for` into the existing `Command` match
in `run_command` (or wherever `Command::For` is dispatched today, around
src/executor.rs:158).

## Errors

The arith error stream from `arith::eval` uses the existing
`ArithError` type, which already produces user-quality messages
(`division by zero`, `syntax error`, etc.). The executor's `eprintln!`
prefix `"huck: (( :"` matches bash's `bash: ((: error_text` style
closely enough.

Parse-time errors carry the inner message:
- `ParseError::ArithBlock("syntax error near '+'")` — when `arith::parse`
  fails on a header section or a standalone block.
- `ParseError::ArithForHeader("expected 3 sections separated by `;`, got 2")`
  — when the for-loop header doesn't split into exactly 3 `;`-separated parts.

## Testing

### Lexer unit tests (`src/lexer.rs::tests`, ~8 tests)

- `arith_block_simple` — `"((1+2))"` → one `ArithBlock("1+2")` token.
- `arith_block_with_semicolons` — `"((a;b;c))"` → `ArithBlock("a;b;c")`.
- `arith_block_nested_parens` — `"(((a+b)*c))"` → `ArithBlock("((a+b)*c)")` (the outer `((`/`))` is the delimiter).
- `arith_block_with_whitespace_inside` — `"((  1 + 2  ))"` → `ArithBlock("  1 + 2  ")`.
- `arith_block_empty` — `"(())"` → `ArithBlock("")`.
- `arith_block_unclosed_errors` — `"((1+2"` → `LexError::UnterminatedArithBlock`.
- `space_between_parens_is_not_arith` — `"( (cmd) )"` → tokens for `LParen LParen Word("cmd") RParen RParen` (two separate ops; existing nested-subshell path).
- `arith_block_in_arg_position` — `"echo ((1+2))"` — the lexer can either emit ArithBlock (parser rejects) or treat as args; document the chosen behavior. Likely simplest: lexer always recognizes contiguous `((`, parser rejects at arg position with a specific error.

### Parser unit tests (`src/command.rs::tests`, ~12 tests)

- `parse_standalone_arith` — `((1+2))` → `Command::Arith(...)`.
- `parse_arith_for_basic` — `for ((i=0;i<10;i++)) do :; done` → `Command::ArithFor { init: Some(...), cond: Some(...), step: Some(...), body: ... }`.
- `parse_arith_for_all_empty` — `for ((;;)) do :; done` → all three are `None`.
- `parse_arith_for_only_init` — `for ((i=0;;)) do :; done`.
- `parse_arith_for_only_cond` — `for ((;i<10;)) do :; done`.
- `parse_arith_for_only_step` — `for ((;;i++)) do :; done`.
- `parse_arith_for_newline_before_do` — `for ((;;))\ndo :; done`.
- `parse_arith_for_semicolon_before_do` — `for ((;;)); do :; done`.
- `parse_arith_for_bad_header_two_sections_errors` — `for ((i=0;i<10)) do :; done` → `ArithForHeader`.
- `parse_arith_for_arith_parse_error_in_section_errors` — `for ((i=+;;)) do :; done` → `ArithBlock`.
- `parse_arith_for_missing_do_errors` — `for ((;;)) :; done` → `UnterminatedLoop`.
- `parse_arith_for_missing_done_errors` — `for ((;;)) do :` → `UnterminatedLoop`.

### Executor unit tests (`src/executor.rs::tests`, ~6 tests)

- `arith_command_nonzero_exits_0`.
- `arith_command_zero_exits_1`.
- `arith_command_division_by_zero_exits_1_with_diag`.
- `arith_for_counter` — `for((i=0;i<3;i++)) do echo $i; done` produces `0\n1\n2\n`.
- `arith_for_break_at_value` — `for((i=0;i<10;i++)) do if [ $i -eq 5 ]; then break; fi; done` ends with `$i` == 5.
- `arith_for_continue_evaluates_step` — `for((i=0;i<5;i++)) do continue; done` ends with `$i` == 5 (step runs after `continue`).

### Integration tests (`tests/arith_for_integration.rs`, ~8 tests)

Binary-driven (stdin-piped since huck has no `-c`):

- Standalone `((x=5))` followed by `echo $x` prints `5`.
- `for((i=0;i<5;i++)) do printf "%d " $i; done` prints `0 1 2 3 4`.
- `for((;;)) do break; done; echo ok` prints `ok`.
- `if ((x > 0)); then echo positive; fi` works.
- Nested `for((i=0;i<2;i++)) do for((j=0;j<2;j++)) do printf "%d%d " $i $j; done; done` prints `00 01 10 11`.
- `set -e` + arith error inside the loop exits the shell.
- `((cmd))` (no space) NO LONGER parses as nested-subshell — document
  this via a test that asserts it parses as arith (e.g., `((5+5))` exits 0).
- `( (cmd) )` (with space) STILL parses as nested subshell (regression).

### Bash-diff harness (`tests/scripts/arith_for_diff_check.sh`, ~10 fragments)

Byte-identical to bash 5.2:

- `((1+2)); echo $?` → `0`
- `((0)); echo $?` → `1`
- `for ((i=0;i<3;i++)); do echo $i; done`
- `for ((;;)); do break; done; echo ok`
- `for ((i=0;i<5;i++)); do if [ $i -eq 2 ]; then continue; fi; echo $i; done`
- `if ((5 > 3)); then echo yes; fi`
- `x=10; ((x++)); echo $x`
- `for ((i=0;i<2;i++)); do for ((j=0;j<2;j++)); do printf "%d%d " $i $j; done; done; echo`
- `((0/1)); echo $?` (zero result)
- `(( 1 / 0 )) 2>&1 | head -1` (division-by-zero diag — may need to strip the binary name prefix difference)

## Scope estimate

| Section | LOC |
|---|---|
| Lexer (scan + token + tests) | ~80 + 50 tests |
| Parser (AST + dispatch + helpers + tests) | ~100 + 80 tests |
| Executor (run_arith + run_arith_for + tests) | ~80 + 50 tests |
| Integration tests | ~120 |
| Bash-diff harness | ~60 |
| Docs | ~30 |
| **Total** | **~260 LOC code + ~410 LOC tests** |

Three tasks:

1. **Lexer + parser + AST** — emit `Token::ArithBlock`; parse standalone + for-header. ~15 unit tests (8 lexer + 12 parser).
2. **Executor** — `run_arith` + `run_arith_for`; ~6 unit tests + ~8 integration tests.
3. **Bash-diff harness + docs** — flip M-23 to `[fixed v78]`; add new fixed entry for standalone arith (or wrap into M-23); change-log entry; M-11 update for the `((cmd))` no-space change; README row.

## Deferrals

After v78 ships, the following remain `[deferred]`:

- `let EXPR EXPR ...` builtin form (separate Tier-2 entry — verify if catalogued).
- Floating-point arith inside `((...))`. Not in any current entry; not adding.
- Comma operator `((i++, j++))` — already supported by huck's arith (verify in tests).

## Change-log entry (for `docs/bash-divergences.md`)

To add after merge:

> **2026-06-XX** (implementer updates to merge date): v78 ships M-23 —
> bash's C-style `for ((init; cond; step)) do BODY done` arithmetic
> for-loop. Also adds bash's standalone `((expr))` command form
> (previously parsed as a nested subshell per M-11; now parses as
> arith). New `Token::ArithBlock(String)` in `src/lexer.rs` capturing
> raw text between `((` and matching `))` (depth-tracked). New
> `Command::Arith(ArithExpr)` and `Command::ArithFor(Box<ArithForClause>)`
> variants in `src/command.rs`. New `run_arith` and `run_arith_for`
> in `src/executor.rs`; the latter mirrors `run_for`'s
> break/continue/return/exit/SIGINT handling. Empty cond in
> `for ((;;))` = always true. Empty header sections evaluate to no-op.
> Lexer requires contiguous `((` (no whitespace) — `( (cmd) )` with
> whitespace continues to parse as nested subshell per M-11. New
> `ParseError::ArithBlock(String)` and `ParseError::ArithForHeader(String)`
> variants carry arith-inner-error diagnostics. ~26 unit tests + 8
> integration tests + 10 bash-diff fragments byte-identical to bash 5.2.
> M-11 entry updated to note the `((cmd))` no-space behavior change.

## Open questions

None. All architectural decisions resolved during brainstorm.
