# huck v21: `case` Statements — Design Spec

## Overview

huck has `if` (v17), `while`/`until` (v18), and `for` (v20) compound
commands. v21 adds the last POSIX control-flow construct: `case`.

```
case WORD in
    PATTERN1 | PATTERN2) LIST ;;
    (PATTERN3)            LIST ;&
    *)                    LIST ;;
esac
```

`case` expands the subject word, then walks the clauses in order; the first
clause with a matching pattern runs, and the clause terminator decides what
happens next. `case` is a compound command at the sequence level, exactly
like `if`/`while`/`for` — it composes with `;`/`&&`/`||`, nests, and (via
v19) can span multiple input lines.

This follows the established compound-command pattern — a new `Command` AST
variant, a recursive-descent `parse_case`, and an executor `run_case` — but
unlike v17/v18/v20 it also requires lexer work: `case` introduces five new
punctuation tokens.

## Scope

**In scope:**

- `case WORD in [(] PATTERN [| PATTERN]... ) LIST TERMINATOR ... esac`.
- All three terminators: `;;` (run the clause, then done), `;&` (fall
  through — run the next clause's body unconditionally), `;;&` (continue —
  resume pattern-testing at the next clause).
- The optional leading `(` before a pattern list.
- An omitted terminator on the final clause.
- Empty clause bodies (`pattern) ;;`) and an empty `case` (`case x in esac`).
- Glob pattern matching (`*`, `?`, `[…]`), `|`-alternation, quoted
  metacharacters matched literally.
- `break`/`continue` inside a `case` body (they target the enclosing loop —
  `case` is not a loop).
- Multi-line `case`, inherited from v19.

**Out of scope:**

- `select` loops.
- Redirections on the whole construct (`esac > file`) — not implemented for
  `if`/`while`/`for` either.
- `case` inside a `|` pipeline, or backgrounding a whole `case` — not
  supported for the other compound commands either.

## Architecture

| Unit | File | Responsibility |
| --- | --- | --- |
| Lexer tokens | `src/lexer.rs` | `(`/`)` and the three `;`-terminators |
| AST + keywords | `src/command.rs` | `Command::Case`, `CaseClause`/`CaseItem`/`CaseTerminator`, `Keyword::{Case,Esac}` |
| Parser | `src/command.rs` | `parse_case`; terminator-aware body termination; stray-paren rejection |
| Executor | `src/executor.rs` | `run_case` — subject expansion, pattern match, fall-through |
| Continuation | `src/continuation.rs` | classify `UnterminatedCase` as incomplete; history joiner |

## Section 1 — Lexer: new tokens

The `Operator` enum gains five variants:

| Variant | Lexeme | Role |
| --- | --- | --- |
| `LParen` | `(` | optional open before a pattern list |
| `RParen` | `)` | closes a pattern list |
| `DoubleSemi` | `;;` | normal clause terminator |
| `SemiAmp` | `;&` | fall-through terminator |
| `DoubleSemiAmp` | `;;&` | continue-testing terminator |

`(` and `)` get their own `match c` arms in `tokenize`, each flushing any
pending word then pushing the token — like the existing `;`/`|`/`&` arms.
Inside `'…'`/`"…"` they stay literal (the quote scanners never reach these
arms); inside `$(…)`/`$((…))`/backticks the closing `)` is consumed by the
substitution scanner and never reaches the tokenizer loop. Both unchanged.

The `;` arm becomes a four-way scan: read `;`, then look ahead — `;` then `;`
then `&` → `DoubleSemiAmp`; `;` then `;` → `DoubleSemi`; `;` then `&` →
`SemiAmp`; otherwise `Semi`.

### Consequence — the ripple

An unquoted `(` or `)` is now a shell metacharacter. `echo (x)` and
`echo a)b` become syntax errors — they were literal words before. Quoted
forms (`echo "(x)"`, `echo 'a)'`) are unaffected. This matches bash.
`;;`/`;&`/`;;&` outside a `case` were already errors and stay errors. The
parser arms that consume these tokens inside `case`, and reject them
elsewhere, are in Section 3.

## Section 2 — AST & keywords

`Keyword` gains `Case` and `Esac`; `keyword_of` maps `"case"`/`"esac"`,
`Keyword::name` gets the two arms. `in` is already a keyword (v20) and is
reused.

`Command` gains `Case(Box<CaseClause>)`:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseClause {
    /// The word being matched — unexpanded.
    pub subject: Word,
    /// The clauses, in source order. May be empty.
    pub items: Vec<CaseItem>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseItem {
    /// The `|`-separated patterns, unexpanded. Always non-empty.
    pub patterns: Vec<Word>,
    /// The clause body. `None` means an empty body.
    pub body: Option<Sequence>,
    pub terminator: CaseTerminator,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CaseTerminator {
    Break,         // ;;
    FallThrough,   // ;&
    ContinueMatch, // ;;&
}
```

Key points:

- **`body` is `Option<Sequence>`** — unlike `if`/`while`/`for`, a `case`
  clause body can be legitimately empty (`a) ;;`), and `Sequence` cannot
  represent emptiness (it requires a `first` command). `None` means "run
  nothing, exit status 0".
- **An omitted final terminator** (`*) echo hi esac`) is recorded as
  `CaseTerminator::Break`. The AST does not distinguish "wrote `;;`" from
  "wrote nothing"; both mean the same thing.
- `subject` and the pattern `Word`s are stored unexpanded; expansion happens
  at run time in `run_case`.
- `items` may be empty — `case x in esac` is valid.
- The `Newline` token never reaches the AST (v19), so a multi-line `case`
  produces the identical `CaseClause` as its single-line form.

Every exhaustive `match` on `Command` gains a `Case` arm — the compiler
enforces this (`run_command` in `executor.rs`, and the `first_pipeline` /
`first_if` test helpers in `command.rs`).

## Section 3 — Parser

`parse_command` gains a `Some(Keyword::Case) => …parse_case…` arm.

### `parse_case`

1. Consume `case`; `skip_newlines`; read the **subject** — the next token
   must be a `Word` (end-of-input → `UnterminatedCase`; a non-`Word` →
   `UnexpectedToken`).
2. `skip_newlines`; `expect` `in` (missing → `UnterminatedCase`).
3. `skip_newlines`, then parse clauses until `esac`. Each clause:
   - Optional leading `(` — consume an `LParen` if present.
   - **Pattern list** — one or more pattern `Word`s separated by `Pipe` (a
     linebreak after `|` is skipped); must be non-empty. After a pattern, the
     next token must be `Pipe` or `RParen`; anything else is
     `UnexpectedToken`.
   - `expect` `RParen`.
   - `skip_newlines`; then inspect the next token: end-of-input →
     `UnterminatedCase`; a terminator (`;;`/`;&`/`;;&`) or `esac` → the
     **body is empty** (`None`); anything else → parse the body as a
     `Sequence`.
   - **Terminator** — `;;` → `Break`, `;&` → `FallThrough`, `;;&` →
     `ContinueMatch` (consumed). Otherwise the clause had no written
     terminator → `Break`, and the token is left for step 4 (which `expect`s
     `esac`, so end-of-input there yields `UnterminatedCase`).
   - `skip_newlines`; loop.
4. `expect` `esac` (missing → `UnterminatedCase`).

### Body termination

A `case` clause body is a `Sequence` that must end at a terminator token or
`esac`. `parse_sequence` and `parse_pipeline` gain the three terminator
operators (`DoubleSemi`/`SemiAmp`/`DoubleSemiAmp`) to their peek-break sets —
a terminator ends the current sequence/pipeline cleanly, like `;`. The body
call passes `stop_at = &[Keyword::Esac]`.

Because `parse_sequence` now breaks on a terminator token, a stray terminator
at the top level would otherwise be silently left behind. So `parse` (the
entry point) gains a guard: after the top-level sequence, any leftover token
yields `ParseError::UnexpectedToken`.

### Stray parens

`parse_pipeline` rejects an `LParen`/`RParen` it encounters with
`UnexpectedToken`; `parse_sequence`'s existing `other` arm already does the
same — so a `(`/`)` outside a `case` pattern list is a clean syntax error.

### New error

`ParseError::UnterminatedCase` covers a truncated `case` (missing
subject/`in`/`)`/`esac`, or EOF mid-construct), with a message arm in
`parse_error_message` (`src/shell.rs`): `"unterminated 'case' (expected
'esac')"`. The v19 completeness classifier maps `UnterminatedCase` to
`Incomplete(Compound)` — a one-line addition to `classify` in
`src/continuation.rs` — so multi-line `case` continuation works with no other
classifier change. `continuation.rs`'s `ends_with_control_keyword` gains
`"case"` so a multi-line `case` collapses to a replayable single-line history
entry.

## Section 4 — Executor: `run_case`

`run_command` gains `Command::Case(clause) => run_case(clause, shell, sink)`.

### Subject

The subject `Word` is expanded to a single string — no field splitting, no
globbing — using the same single-value expansion the executor uses for
redirect targets. `case $x in` with `$x` unset yields the empty string.

### Pattern matching

For each pattern `Word` in a clause, expand it, build a `glob::Pattern`, and
match it against the subject string with `case_sensitive: true,
require_literal_separator: false, require_literal_leading_dot: false` —
`case` patterns are not pathname patterns, so `*` matches `/` and a leading
`.`. This reuses the `glob` crate exactly as `${var#pat}` (v12) does. A
pattern that fails to parse as a glob matches nothing (mirrors v12's
`remove_prefix`). A clause matches if **any** of its `|`-patterns matches.

**Quoting.** An unquoted `*`/`?`/`[` in a pattern is a metacharacter; a
quoted one is matched literally (`case $x in "?")` matches a literal `?`).
The glob-pattern string is assembled from the expanded pattern by escaping
quoted runs via `glob::Pattern::escape` while leaving unquoted runs verbatim.
This is more correct than v12's `${var#pat}`, which does not honour quoting;
v21 does it right because the `Field` type produced by expansion already
carries the per-run quoted information. (Implementation note: the plan must
confirm the expansion `Field` API exposes quoted runs; if it does not, a
small accessor is added.)

### Fall-through state machine

`run_case` walks `items` by index, tracking whether it is mid-fall-through:

1. Expand the subject. `last` starts at `ExecOutcome::Continue(0)`.
2. For index `i` from 0: a clause runs if we are mid-fall-through *or* one of
   its patterns matches the subject.
3. When a clause runs:
   - Execute its body. An empty body (`None`) leaves `last = Continue(0)`. A
     present body runs via `execute_sequence_body`; `ExecOutcome::Exit`
     propagates out (return it); `LoopBreak`/`LoopContinue` propagate out
     unchanged (return them — `case` is not a loop, so `break`/`continue`
     target the enclosing loop, exactly as in `run_if`); `Continue(c)` sets
     `last`.
   - Honour the terminator: `Break` → return `last`; `FallThrough` (`;&`) →
     advance to the next clause and run it unconditionally; `ContinueMatch`
     (`;;&`) → advance to the next clause and resume pattern-testing.
4. A non-matching clause is skipped. Running off the end of `items` (whether
   by fall-through or exhausting the list) returns `last`.

### Exit status

The exit status is the last command of the last body that ran, or 0 if no
clause matched or the matched body was empty — the same convention as
`run_if`. `run_case` does no SIGINT polling: it is not a loop and runs a
bounded number of bodies, exactly like `run_if`.

## Error handling

| Situation | Behaviour |
| --- | --- |
| Truncated `case` (missing subject/`in`/`)`/`esac`, EOF mid-construct) | `ParseError::UnterminatedCase`; at a REPL this is incomplete and a continuation line is read |
| Empty pattern list, or a bad token where a pattern/`)`/terminator is expected | `ParseError::UnexpectedToken`, `huck: syntax error: …`, `$?` = 2 |
| Stray `(`/`)` or `;;`/`;&`/`;;&` outside a `case` | `ParseError::UnexpectedToken`, syntax error, `$?` = 2 |
| No clause matches | Run nothing, `$?` = 0 |
| Matched clause with an empty body | Run nothing, `$?` = 0 |
| Unparseable glob pattern | That pattern matches nothing |
| `break`/`continue` inside a `case` body | Propagate to the enclosing loop (v18 machinery), unchanged |

## Testing

**Lexer unit tests** — `(`/`)` tokenize to `LParen`/`RParen`; `;;`/`;&`/`;;&`
to `DoubleSemi`/`SemiAmp`/`DoubleSemiAmp`; a lone `;` stays `Semi`; quoted
parens stay literal word content.

**Parser unit tests** — `case x in a) echo hi ;; esac` parses to the expected
`CaseClause`; a multi-line `case` parses to the identical AST as its
single-line form; multiple `|`-separated patterns; the optional leading `(`;
an empty clause body (`a) ;;` → `body: None`); each terminator recorded as
the right `CaseTerminator`; an omitted final terminator → `Break`; an empty
`case` (`case x in esac` → no items); a truncated `case` →
`UnterminatedCase`; a stray `)` outside a `case` → `UnexpectedToken`; a
malformed pattern list (two words, no `|`) → error.

**Executor unit tests** — the first matching clause's body runs; glob
patterns (`*`, `?`, `[…]`) and `|`-alternation match; a `*` catch-all; no
match → status 0; an empty body → status 0; a quoted metacharacter matches
literally; `;&` runs the next clause's body unconditionally; `;;&` resumes
testing; `break` inside a `case` body propagates out (a `case` in a `while` —
`break` exits the `while`).

**Integration tests** (new `tests/case_integration.rs`) — piped scripts: a
basic `case`, glob patterns, alternation, the `*` catch-all, a multi-line
`case`, `;&` fall-through, `;;&` continue-testing, a `case` nested inside
`if`/`for`/`while`, `break` from a `case` inside a loop, a quoted-metacharacter
literal match, the no-match exit status, and the `(pattern)` leading-paren
form.

**No new PTY tests** — multi-line `case` rides on v19's continuation
mechanism, already PTY-covered for unterminated compound commands; consistent
with v18/v20.

**Regression** — all 764 existing tests stay green. The `(`/`)`-become-tokens
change is the riskiest part; the v1–v20 suites passing is the proof that no
real script regressed.
