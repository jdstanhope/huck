# Parser error-recovery: a partial tree for incomplete input

Issue: [#246](https://github.com/jdstanhope/huck/issues/246)

## Problem

huck's parser is all-or-nothing on incomplete input. `parse()` returns
`Err(unterminated_*)` the moment it reaches EOF inside any unterminated
construct (`$(`, `${`, `"`, backtick, an `if` without its `then`, …), and the
`?`-propagation discards every node built up to that point.

That is fine for execution, but it blocks the parser from serving *completion*,
which by nature runs on a half-typed line. Today completion sidesteps the
parser entirely with a hand-rolled character scanner (`analyze_full` in
`completion.rs`) that re-derives shell structure — quote/backtick/paren
tracking, command-vs-argument position, opener disambiguation — and keeps
needing hand-patches as edge cases surface. It has real bash divergences a
daily user hits immediately (all currently produce **no completion** where bash
completes):

| input | bash completes | huck |
|---|---|---|
| `if whi` | commands | nothing |
| `echo "$(whi` | commands (inside the quoted comsub) | nothing |
| `echo $(( HO` | variables (arithmetic) | nothing |
| `for x in whi` | words / files | nothing |

This spec is **iteration 1** of a two-part effort. It gives the parser an
error-recovery capability: parse incomplete input and return a usable AST *up to
the cursor*. **Iteration 2** (separate issue) will delete `analyze_full` and
derive completion context by consuming this recovery output — fixing the
divergences above structurally. This spec does **not** touch completion.

## Ground truth this must eventually serve (iteration 2's targets)

Recorded here so iteration 1's output is shaped to make them trivial later
(verified against bash 5.2.21 in the earlier investigation):

- `if whi` / `while whi` / `until whi` → the word after the keyword is a
  **command**.
- `echo "$(whi` → inside a double-quoted command substitution, **command**.
- `echo $(( HO` → inside arithmetic, a **variable name**.
- `for x in whi` → after `in`, a **word/file**.
- `$(`, `` ` ``, `(`, `<(`, `>(` openers → **command**; `${…}` → **variable**;
  `NAME=(` array literal → not a subshell command.

## Design

### Principle: synthesize the minimal valid completion

Recovery is defined as: **at the cursor (real EOF of `line[..pos]`), synthesize
the minimal valid completion of every open construct, innermost-out**, producing
a *complete, well-formed* tree from the *existing* AST types. No AST changes;
the strict `parse()` path used for execution is untouched.

Incompleteness lives at two levels, handled with the same principle:

**Lexer half — open modes.** `$(`, `${`, `$((`, `"`, `'`, backtick, `<(`/`>(`,
`NAME=(`. These are frames on the lexer's `self.modes` stack. Under a recovery
option, when the lexer reaches real EOF with open modes, it emits the synthetic
*closing* atom for each open frame, innermost-out. This is a direct
generalization of the existing `LexerOptions::eof_closes_heredoc`
(`lexer.rs:4100`) — the same "EOF closes it" pattern, applied to every mode.
The parser then sees a well-formed token stream and assembles the nesting
through its normal paths.

The per-mode synthetic closer (enumerable, finite):

| open mode | synthetic close atom |
|---|---|
| `CommandSub` (`$(`) | `)` |
| subshell `(` | `)` |
| `ArrayLiteral` (`NAME=(`) | `)` |
| `Arith` (`$((`) | `))` |
| `BacktickRaw` | `` ` `` |
| `ParamExpansion` (`${`) | `}` |
| `DoubleQuote` | `"` |
| `SingleQuote` / `$'…'` | `'` |
| `Regex` / `Extglob` | their existing zero-width terminators |
| heredoc | already handled by `eof_closes_heredoc` |

**Parser half — open compound commands.** `if`/`while`/`until`/`for … in`/
`case`/`{ … }`/subshell body. These are parser *keywords*, not lexer modes, so
the lexer cannot close them. At each `Err(unterminated_*)` / EOF site in the
parser (the ~10-15 identified sites — `UnterminatedSubshell` at `parser.rs:845`,
`unterminated_cmdsub` at `:1654`/`:1698`/`:1775`/`:1802`, `unterminated_backtick`
at `:2013`, the `Unexpected{Eof}` sites at `:2785`/`:3074`/`:3832`, etc.), under
the recovery flag the parser synthesizes the minimal body and returns the node
instead of erroring. Minimal-body templates (enumerable):

| open construct at EOF | synthesized completion |
|---|---|
| `if COND` (no `then`) | `if COND; then :; fi` |
| `while/until COND` | `while COND; do :; done` |
| `for x in WORDS` (no `do`) | `for x in WORDS; do :; done` |
| `for x` (no `in`) | `for x in; do :; done` |
| `case W in` (no pattern/`esac`) | `case W in esac` |
| `{ …` | `{ …; }` |
| `( …` | `( … )` (also reachable via the lexer subshell closer) |

The synthetic `:` / empty bodies are inert placeholders; completion ignores the
synthetic tail (see cursor location).

### Why synthesize rather than a true "partial" AST

Making the AST itself representable-as-incomplete (optional fields, an
`Incomplete` variant) would ripple through every `match` on `Command`/`Sequence`
across the engine. Synthesizing the minimal completion needs **zero AST
changes**, keeps every existing tree consumer working unchanged, and is one
consistent principle at both levels. The cost is a small, enumerable set of
closer/body templates — contained entirely within the recovery paths.

### API and cursor location

The AST carries **no source spans** on its nodes (`Sequence`, `Command`,
`Pipeline`, `Word` have none), so the cursor node cannot be found by searching
the finished tree for an offset — and adding spans throughout is exactly the
ripple we are avoiding. Instead, recovery captures the cursor context **at the
synthesis boundary**: the instant real-EOF is reached and synthesis begins, the
lexer's mode stack and the parser's grammar frames already *are* the
enclosing-construct chain at the cursor. That is captured directly, for free.

Public entry point in huck-syntax:

```rust
pub fn parse_recover(src: &str) -> RecoveredParse;

pub struct RecoveredParse {
    /// The recovered (synthetically-completed) tree. `None` only for input
    /// that is empty or a hard syntax error unrelated to incompleteness.
    pub tree: Option<Sequence>,
    /// The cursor context captured at the synthesis boundary (real EOF).
    pub cursor: CursorContext,
}

pub struct CursorContext {
    /// Enclosing constructs, innermost LAST. e.g. `echo "$(whi` →
    /// `[DoubleQuote, CommandSub]`; `echo $(( HO` → `[Arith]`;
    /// `if whi` → `[IfCondition]`.
    pub enclosing: Vec<Frame>,
    /// What the cursor word is: Command | Argument | VariableName
    /// | RedirectTarget | AssignRhs | Unknown. A bare-identifier operand
    /// inside `$(( … ))` is `VariableName` with `enclosing` ending in `Arith`.
    pub position: WordPosition,
    /// The partial word at the cursor, unescaped. Empty if the cursor sits at a
    /// boundary (e.g. right after a space).
    pub word: String,
    /// Byte offset in `src` where `word` begins (the replacement anchor).
    pub word_start: usize,
}

pub enum Frame {
    CommandSub, Subshell, ArrayLiteral, Arith, Backtick,
    DoubleQuote, SingleQuote, ParamExpansion,
    IfCondition, WhileCondition, ForList, CaseSubject, BraceGroup,
}

pub enum WordPosition {
    Command, Argument, VariableName,
    RedirectTarget, AssignRhs, Unknown,
}
```

**The caller passes `src = line[..pos]`** — the cursor is EOF, matching how
completion already truncates. So "recover to EOF" *is* "recover to the cursor";
no cursor-offset needs threading through the parser.

`tree` is produced (per the chosen direction and for future value: richer
completion, tooling, better diagnostics), but iteration 2's completion consumer
reads `cursor`, the distilled thing it needs. `CursorContext` is deliberately a
superset of what today's `analyze_full` returns (`Command`/`Variable`/`File` +
`word_start` + prefix), so iteration 2 is a lowering from `CursorContext` to the
existing completion contexts plus the new cases.

`Frame`/`WordPosition`/`CursorContext` are `#[non_exhaustive]` so iteration 2 (or
later) can add cases without a breaking change.

### Isolation and boundaries

- **New, additive surface only.** `parse_recover` is a new entry point; the
  recovery behavior is gated behind a `LexerOptions` recovery flag and a parser
  recovery flag threaded from it. The strict `parse()` / `parse_sequence` path
  and its options default off, so execution parsing is byte-for-byte unchanged.
- **What each unit does:** the lexer recovery option owns synthetic mode-closing;
  the parser recovery flag owns synthetic compound-command bodies + capturing
  the `CursorContext`; `parse_recover` composes them and returns
  `RecoveredParse`. Each is independently testable.

## Testing

Entirely within huck-syntax — no completion code involved.

1. **Recovery-context tests**, one per construct, asserting `cursor.position`,
   the tail of `cursor.enclosing`, and `cursor.word`/`word_start`:
   - openers: `echo $(whi`, `echo \`whi`, `(whi`, `echo <(whi`, `cat >(whi` →
     `Command`; `echo ${whi` → `VariableName`; `x=(whi` → not `Command`
     (array literal); `x=$(whi` → `Command`.
   - quoted: `echo "$(whi` → `Command`, enclosing ends `[DoubleQuote,
     CommandSub]`; `echo "$whi"` → argument/word inside the quote.
   - arithmetic: `echo $(( HO` → `VariableName`, enclosing
     `[Arith]`.
   - compound: `if whi` / `while whi` / `until whi` → `Command`; `for x in whi`
     → `Argument`; `for x whi` (before `in`) → the `for`-name slot; `case whi`
     → `CaseSubject`; `{ whi` / `( whi` → `Command`.
   - plain: `whi` (command), `echo whi` (argument), `echo $HO` (variable),
     `echo ./whi` (file with a dir prefix), empty line (command).
2. **Recovered-tree shape tests** for a representative few (e.g. `echo $(whi`
   recovers to `echo` with a `CommandSub` argument whose body is the command
   `whi`) — proving the synthetic completion yields a well-formed, walkable tree.
3. **Strict-path-unchanged gate:** the full existing huck-syntax parser test
   suite stays green; `parse()` output is unaffected because recovery is a
   separate flag/entry point.
4. **Panic-robustness sweep:** for every input in the existing parser test
   corpus, call `parse_recover` on **every** byte-offset truncation and assert
   it returns without panicking. Completion feeds arbitrary partial input on
   every keystroke, so never-panic is a hard requirement.

Per the repo's constraints, run tests per-crate
(`cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`); the full
`tests/scripts/run_diff_checks.sh` sweep must stay green (unaffected — recovery
is a new surface, not a change to execution).

## Scope

**In scope (iteration 1).** The `parse_recover` entry point; the lexer recovery
option (synthetic mode closers); the parser recovery flag (synthetic
compound-command bodies + `CursorContext` capture); `RecoveredParse` /
`CursorContext` / `Frame` / `WordPosition`; the tests above.

**Out of scope.** Any change to `analyze_full`, `dispatch::resolve`, or
completion behavior (iteration 2). Adding source spans to the AST. Recovery of
constructs the current parser does not model.

**Deferred to iteration 2 (separate issue).** Deleting `analyze_full`; lowering
`CursorContext` to completion candidate sources; the bash-diff completion tests;
fixing the four divergences in the table.

## Documentation

`docs/architecture.md`'s front-end / parser section gains a note that a
recovery entry point (`parse_recover`) exists alongside strict `parse`, for
completion and tooling. No `bash-divergences.md` entry — this is internal
machinery, not a user-visible divergence (the divergences it enables fixing are
tracked on their own issues and close in iteration 2).
