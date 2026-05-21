# huck v19: Multi-line Input — Design Spec

## Overview

huck currently reads exactly one physical line per command. A compound
command (`if`, `while`, `until`) must be written on a single line with its
parts separated by `;`. v19 lets a command span multiple input lines: the
REPL keeps reading until the typed text forms a complete command, showing a
`> ` continuation prompt for each extra line.

This delivers two things at once:

1. **Multi-line input** — open quotes, pending operators, and backslash
   continuations all carry over onto the next line.
2. **Retrofit of `if`/`while`/`until`** — the v17/v18 compound commands can
   now be written across several lines, the way they appear in real scripts.

The AST and executor are **unchanged**. Multi-line support is entirely a
matter of the lexer, the parser, a new completeness classifier, and the REPL
read loop.

## Scope

**In scope:**

- A `Newline` token; the parser treats it as a skippable soft separator.
- Continuation when input ends with: an open quote or expansion (`'`, `"`,
  `` ` ``, `$(`, `${`, `$((`); a pending operator (`|`, `&&`, `||`); an
  unterminated compound command (`if`/`while`/`until`); or a backslash.
- A pure completeness classifier driving the continuation decision.
- A REPL continuation loop with a `> ` prompt, Ctrl-C abort, and EOF
  handling.
- Multi-line `if`/`while`/`until` (the retrofit).
- Multi-line commands stored as a single, `;`-joined history entry.

**Out of scope:**

- A script-file argument (`huck script.sh`) or `-c string`. huck still has
  only two input sources — interactive TTY and piped stdin — and both flow
  through the same REPL read loop, so no separate "parse a whole file" path
  is needed or built.
- Here-documents (`<<EOF`) — a separate future iteration.
- A user-configurable continuation prompt (`PS2` as a variable).
- Comments (`#`) — huck has none today; unaffected.

## Architecture

Four units, each independently testable:

| Unit | File | Responsibility |
| --- | --- | --- |
| `Newline` token | `src/lexer.rs` | Emit a token for a newline outside quotes |
| Parser retrofit | `src/command.rs` | Treat `Newline` as a skippable soft separator |
| Completeness classifier | `src/continuation.rs` (new) | Decide Complete / Incomplete / Error for a buffer |
| REPL continuation loop | `src/shell.rs` | Accumulate lines until complete; prompts; abort; EOF; history |

Data flow per command: `run()` calls `read_logical_command`, which reads
physical lines and calls `continuation::classify` after each; once the buffer
is `Complete` (or `Error`) it is handed to the existing `process_line`, which
lexes, parses, and executes exactly as today.

## Section 1 — Tokenizer & parser: newlines as separators

### Lexer

Add `Token::Newline`. Outside quotes, a `\n` character — currently swallowed
by the `c.is_whitespace()` branch — instead flushes any pending word (the
same flush other whitespace performs) and pushes a `Token::Newline`. `\r`
remains ordinary ignored whitespace. Inside `'…'` and `"…"`, a `\n` stays
literal content; the quote-scanning loops already push any character
verbatim, so no change is needed there.

Consecutive blank lines produce consecutive `Newline` tokens; the parser
collapses them. The lexer never encounters a backslash-newline: the REPL
strips a trailing unescaped `\` before assembling the buffer (Section 3), so
`\` is never adjacent to a `\n` the lexer sees.

### Parser

`Newline` is a **soft separator**: it ends a command like `;`, but unlike
`;` it is *also* skipped wherever a command is expected but not yet present.
`Semi` keeps its current strict behavior.

A `skip_newlines` helper consumes a run of `Newline` tokens. It is called:

- At the start of `parse()` — leading newlines skipped; if nothing remains,
  `parse()` returns `Ok(None)` (a blank or all-newline buffer is a no-op).
- At the start of `parse_sequence`, and after each `;`/`&&`/`||`/`Newline`
  connector before the next `parse_command`.
- After the `if`/`while`/`until` keyword, and after `then`/`do`/`else`/
  `elif` — so `if`⏎`cond`, `then`⏎`body`, etc. parse.
- After each `|` inside `parse_pipeline` — so `cmd |`⏎`cmd` continues the
  pipeline.

In `parse_sequence`'s connector loop, a `Newline` token is handled like
`Semi`: it joins the next command with `Connector::Semi`. A `Newline` (or run
of them) immediately followed by a `stop_at` keyword, or by end-of-input,
simply terminates the sequence. The `Token` match in `parse_sequence` must
handle `Newline` explicitly — the compiler enforces this once the variant is
added, which guarantees no `Newline` reaches the `unreachable!` arm.

### Behavior

- The `Newline` token never reaches the AST. `if`/`while`/`until` produce the
  identical `IfClause`/`WhileClause` whether written on one line with `;` or
  across many lines.
- The executor and AST are untouched.
- A newline after `then`/`do` is valid; a `;` there remains a syntax error
  (`then; cmd` is invalid — matching bash). Only newlines are soft.

### Backward compatibility

Existing single-line `;`-separated token streams contain no `Newline`
tokens, so they parse exactly as before. The v17/v18 `if`/`while` test
suites are the regression proof.

## Section 2 — Completeness classifier

A new module `src/continuation.rs` exposes a pure function:

```
classify(buffer: &str) -> Completeness

enum Completeness {
    Complete,
    Incomplete(ContinuationReason),
    Error,
}

enum ContinuationReason { Backslash, OpenQuote, Operator, Compound }
```

### Algorithm (in order)

1. **Trailing backslash** — if `buffer` ends with an odd-length run of `\`,
   return `Incomplete(Backslash)`. Checked before lexing.
2. **Tokenize** with `lexer::tokenize`:
   - `UnterminatedQuote`, `UnterminatedBrace`, `UnterminatedSubstitution`,
     `UnterminatedArith` → `Incomplete(OpenQuote)`.
   - Any other `LexError` → `Error`.
3. **Trailing operator** — if the last token is `Op(Pipe | And | Or)`,
   return `Incomplete(Operator)`.
4. **Parse** with `command::parse`:
   - `UnterminatedIf`, `UnterminatedLoop` → `Incomplete(Compound)`.
   - Any other `ParseError` → `Error`.
   - `Ok(_)` → `Complete`.

### Joiner helper

Also in `continuation.rs`, a pure helper used by the REPL for the history
form:

```
joiner_for(reason: ContinuationReason, last_line: &str) -> &'static str
```

- `Backslash` → `""`
- `Operator` → `" "`
- `OpenQuote` → `"; "`
- `Compound` → `" "` when `last_line`, trimmed, ends with a bare control
  keyword (`if`/`while`/`until`/`then`/`do`/`else`/`elif`); otherwise `"; "`.

### Notes

- The classifier carries no error message and no tokens. On `Complete` *or*
  `Error`, the REPL hands the finished buffer to the existing `process_line`,
  which re-lexes and re-parses and either executes or prints the syntax error
  using the messages it already produces. The buffer is therefore processed
  twice — once to classify, once to run. This is intentional: it keeps the
  classifier tiny and avoids duplicating error-formatting, at a cost that is
  negligible at a prompt.
- `classify` runs on every input line and **must never panic**. During
  implementation, verify that a stray token after a closed compound command
  (e.g. `if x; then y; fi extra`) yields a `ParseError` rather than reaching
  the parser's `unreachable!` arm. If that path is reachable, fix it as part
  of v19 — the classifier exercises it far more than before.
- Treating `UnterminatedIf`/`UnterminatedLoop` as `Incomplete` is always
  safe: any such buffer can be completed by appending the missing keyword, so
  the user is never trapped, and Ctrl-C aborts regardless.

## Section 3 — REPL continuation loop

A new function `read_logical_command` in `src/shell.rs` wraps line reading;
`run()` calls it in place of the bare `editor.readline`.

### The loop

Read a physical line (`huck> ` for the first line of a command, `> ` for
each continuation line). Apply history expansion to that physical line, as
`run()` does today (an expanded line is echoed). Append the line to two
parallel accumulators, then call `classify(buffer)`:

- `Incomplete(reason)` → read another line; `reason` and the just-read line
  select the joiner via `joiner_for`.
- `Complete` or `Error` → stop reading; hand `buffer` to `process_line`,
  which executes it or prints the syntax error.

### Two accumulators

- **`buffer`** (fed to the lexer/executor) joins physical lines with a real
  `\n`, so the lexer emits `Newline` tokens and quoted content keeps its
  newlines. The exception is a backslash continuation: the trailing `\` is
  stripped from the line and the next line is joined with nothing.
- **`history_entry`** (stored in history) joins physical lines with
  `joiner_for(reason, last_line)`. The result is a single physical line.

For a single-line command the two accumulators are equal and history is
unchanged from today.

### Prompts

`PS1` stays `huck> `. `PS2` is the constant `> `. rustyline already omits
the prompt when stdin is not a TTY, so piped scripts display neither prompt
— no extra code is required for the interactive-vs-piped distinction.

### History

Once the read loop finishes — whether the buffer classified as `Complete`
or as a genuine `Error` — the single-line `history_entry` is added to huck's
history and to rustyline's editor history, gated by the existing "non-blank"
check (a syntax-error command is still recorded, as today). A multi-line
command therefore appears in `history` and
`~/.huck_history` as one `;`-joined line. This is lossy in two known cases,
accepted by design: a newline inside a quoted string becomes a literal `;`,
and an unusual layout may misjudge the `then`/`do` keyword check. The
*executed* command is always correct; only the stored form is approximate.

### Ctrl-C (Interrupted)

Mid-buffer, Ctrl-C discards the partial command and returns to the `huck> `
prompt — nothing is executed, nothing is stored. At an empty first-line
prompt, behavior is unchanged from today (redraw the prompt).

### Ctrl-D (EOF)

At an empty first-line prompt, EOF exits the shell as today. Mid-buffer, EOF
means the input stream ended on an incomplete command: huck prints
`huck: syntax error: unexpected end of input`, sets `$?` to 2, and exits.
This makes a truncated piped script fail cleanly. It is slightly stricter
than bash for an interactive Ctrl-D mid-entry (bash abandons the line and
returns to the prompt); this simplification is deliberate for v19.

## Error handling

| Situation | Behavior |
| --- | --- |
| Genuine syntax error (classifier returns `Error`) | `process_line` prints `huck: syntax error: …`, `$?` = 2 (unchanged) |
| EOF mid-buffer | `huck: syntax error: unexpected end of input`, `$?` = 2, exit |
| Ctrl-C mid-buffer | Discard buffer, return to `huck> `, `$?` unchanged |
| Blank / all-newline buffer | `parse()` → `Ok(None)` → no-op, `$?` = 0 (unchanged) |
| Open quote/operator/compound at EOF of a piped script | Same as EOF mid-buffer: syntax error, exit 2 |

## Testing

**Lexer unit tests** — `\n` outside quotes emits `Token::Newline`; `\n`
inside `'…'`/`"…"` stays literal; consecutive blank lines emit consecutive
`Newline` tokens.

**Parser unit tests** — a multi-line `if`/`while`/`until` token stream parses
to the identical AST as its single-line `;` form; `Newline` skipped after
`then`/`do`/`else`/`elif` and after `|`/`&&`/`||`; leading newlines skipped;
an all-`Newline` buffer → `Ok(None)`; `then` followed by `Semi` still errors.

**Classifier unit tests** (`continuation.rs`) — a table of `buffer →
Completeness`: complete commands; one case per `Incomplete` reason (open
quote, open `$(`/`${`/`$((`, trailing `|`/`&&`/`||`, open `if`/`while`/
`until`, trailing `\`); genuine `Error` cases. A separate table for
`joiner_for`, including the `then`/`do`-ends-the-line space case.

**Integration tests** (new `tests/multiline_integration.rs`) — pipe real
multi-line scripts to huck and check output: multi-line `if`, `while`,
`until`, a nested loop inside an `if`, a quoted string spanning lines, a
trailing-`|` pipeline, a trailing-`&&` sequence, a backslash-newline join,
and a script that ends inside an unterminated `if` → `unexpected end of
input`, exit status 2. One test confirms a multi-line command appears in
`history` as its single-line `;`-joined form.

**PTY tests** (extend `tests/pty_interactive.rs`) — an incomplete first line
produces the `> ` continuation prompt; a multi-line `if` typed interactively
runs and prints its output; Ctrl-C at the `> ` prompt discards the buffer,
returns to `huck> `, and the shell survives to run a following command.

**Regression** — all 679 existing tests stay green; the v17/v18 single-line
`if`/`while` suites prove the `Newline` retrofit is backward-compatible.
