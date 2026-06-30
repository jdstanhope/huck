# v242 — parser-driven command-list parser (flat subset) in `parser.rs` (design)

**Status: DESIGN (approved direction).** Date: 2026-06-30.

The first **command-level** piece of the Phase C parser-driven front-end: a flat
command-list parser built in `crates/huck-syntax/src/parser.rs` (the home v241
created), consuming the production `Command`-mode token stream and producing the
existing `Sequence`/`Command` AST — **dormant** and **differentially tested against
`command.rs`**. This begins the eventual Stage-2 command parser. Direction:
memory `huck-frontend-parser-driven-direction`; roadmap
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md`.

## Goal

Parse the **flat command grammar** — a `Sequence` of pipelines joined by
`;`/`&&`/`||`/`&`/newline, each pipeline of simple commands joined by `|`, each
simple command being assignments + program + args + redirects — in `parser.rs`,
producing the **same `Sequence` AST** `command.rs` produces, verified by a
differential corpus. `command.rs` is the oracle and stays untouched.

## Scope (in)

Reusing the existing AST types verbatim (no AST change — engine untouched):
- **`Sequence { first: Command, rest: Vec<(Connector, Command)>, background: bool }`**
  — and-or list: pipelines joined by `Connector::{Semi, And, Or, Amp}` and newlines;
  a trailing `&` sets `background`.
- **`Pipeline { negate: bool, commands: Vec<Command> }`** — optional leading `!`,
  simple-command stages joined by `|`. A single command is represented exactly as
  `command.rs` does (see §5 — the oracle decides `Command::Simple` vs
  `Command::Pipeline`).
- **`SimpleCommand::{ Exec(ExecCommand), Assign(Vec<Assignment>, u32) }`** —
  `ExecCommand { inline_assignments, program, args, redirects, line }`; a line of
  only `A=1 B=2` → `Assign`.
- **Redirects** (`Vec<Redirection>` in source order): the file/fd operators
  (`> >> < 2> >& <& &> &>> >| 2>| <>`), `Word` targets, and glued `RedirFd`
  prefixes (`3>`, `{fd}>`). Source order preserved.
- **Assignments**: leading `NAME=value` / `NAME+=value` / `NAME[i]=value` prefix
  words → `inline_assignments`; reuses the lexer-built `WordPart::AssignPrefix`
  and the `NAME=…` literal split exactly as `command.rs`'s `try_split_assignment`.

## Non-goals (deferred → `ParseError::UnsupportedCommand`)

The parser returns the NEW `ParseError::UnsupportedCommand` on encountering any of:
- **Subshell** `( … )` (`Op(LParen)` at command position).
- **Arith command** `(( … ))` (a `TokenKind::ArithBlock`).
- **Every compound command**: `if`/`while`/`until`/`for`/`case`/`select`/`{ … }`/
  `[[ … ]]`/function definition (`name()` or `function name`)/`coproc` — detected via
  the reserved-word / `( )`-after-name checks `command.rs` already uses.
- **Heredocs / here-strings** (`TokenKind::Heredoc`, `Op(HereString)`).
- **Command substitution inside words**: not parsed by this layer (the lexer
  pre-builds `WordPart::CommandSub`), but the differential **corpus avoids `$(…)`/
  backtick words** to keep the first slice focused; a word that happens to contain a
  pre-built `CommandSub` part still passes through opaquely (it is just a `WordPart`).

`UnsupportedCommand` lets the corpus assert the boundary cleanly (like v241's
`UnsupportedExpansion`). Also keeps `parser.rs` from silently mis-parsing a construct
it doesn't model.

## Global constraints

- **Byte-identical / dormant:** `parser.rs`'s command parser is reached ONLY by tests;
  `command.rs`'s parser, `Command` mode, and the engine are untouched. `cargo test
  --workspace` green, 0 warnings; release harness sweep byte-identical.
- **No lexer change at all** this iteration — `parser.rs` consumes the production
  `Command`-mode tokens via the existing pull API (`peek_kind`/`peek2_kind`/`next_kind`/
  `peek_span`/`next`). No new `Mode`, no new `TokenKind`.
- **`command.rs` is the oracle.** When the differential disagrees, fix `parser.rs` to
  match `command.rs` — never weaken the comparison or touch `command.rs`.
- New `ParseError::UnsupportedCommand` variant (+ `errors.rs` message arm) — additive.
- All new parsing code in `crates/huck-syntax/src/parser.rs`.

## 1. The parser functions (mirror `command.rs`'s structure)

New `pub(crate)` entry + internal fns in `parser.rs`. Names mirror `command.rs`'s
reference fns so the implementer can follow them row-for-row:

```rust
// Entry — mirrors `command::parse` (returns None on empty input).
pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>;

// internal:
fn parse_and_or(iter: &mut Lexer) -> Result<Sequence, ParseError>;   // pipelines + connectors + background
fn parse_pipeline(iter: &mut Lexer) -> Result<Command, ParseError>;  // `!` + `|` stages -> Command::Simple/Pipeline
fn parse_command(iter: &mut Lexer) -> Result<Command, ParseError>;   // dispatch: simple, else UnsupportedCommand
fn parse_simple(iter: &mut Lexer) -> Result<Command, ParseError>;    // assignments + program + args + redirects
fn parse_redirects(...) -> Result<(), ParseError>;                   // interleaved redirect ops -> Vec<Redirection>
```

Reference functions in `command.rs` (the oracle to mirror): `parse` /
`parse_sequence` / `parse_command_then_pipeline` / `parse_simple_stage` /
`parse_trailing_redirects` / `try_split_assignment` / `keyword_of` /
`next_is_redirect`. `parser.rs` reimplements the FLAT subset of these; the deferred
constructs short-circuit to `UnsupportedCommand` at the dispatch point where
`command.rs` would branch into a compound.

## 2. Token consumption + dispatch

`parse_command` peeks (`peek_kind`, and `peek2_kind` for the `name()` function-def
check) and dispatches:
- a **reserved word** at command position (`keyword_of` recognizes `if`/`then`/`while`/
  `until`/`for`/`do`/`case`/`select`/`{`/`}`/`[[`/`function`/`!`-as-keyword-context/…)
  → `UnsupportedCommand` (except `!` which is the pipeline-negate prefix, handled in
  `parse_pipeline`).
- `Op(LParen)` → `UnsupportedCommand` (subshell).
- `TokenKind::ArithBlock(..)` → `UnsupportedCommand` (arith command).
- a `Word` that is `name` immediately followed by `Op(LParen)` then `Op(RParen)`
  (function def) → `UnsupportedCommand`.
- otherwise → `parse_simple`.

`parse_simple` consumes a run of `Word`/redirect tokens until a stage/list terminator
(`Op(Pipe)`, `Op(Semi)`, `Op(And)`, `Op(Or)`, `Op(Background)`, `Op(RParen)`,
`Newline`, EOF), building `ExecCommand`/`Assign`. On a `TokenKind::Heredoc`,
`Op(HereString)`, or a deferred construct mid-command → `UnsupportedCommand`.

## 3. Differential testing (the proof)

`Sequence`/`Command`/`Pipeline`/`ExecCommand`/`SimpleCommand`/`Assignment`/
`Redirection` all derive `PartialEq` — so AST equality is the check.

```rust
// for each input S:
let toks = tokenize_with_opts(S, opts)?;                 // production Command-mode tokens, ONCE
let want = command::parse(&mut Lexer::from_tokens(toks.clone()))?;   // oracle
let got  = parser::parse_sequence(&mut Lexer::from_tokens(toks))?;   // new path
assert_eq!(got, want, "command AST mismatch for {S:?}");
```

(`from_tokens` gives a replay lexer — both parsers consume identical token streams.
Use two clones since each consumes destructively.)

**In-scope corpus** (asserts `got == want`): `echo a`; `echo a b c`;
`A=1 B=2 cmd x`; `A=1`; `A=1 B=2`; `cmd >out`; `cmd >>out 2>&1`; `<in cat`;
`3>f cmd`; `{fd}>f cmd`; `cmd a >o b <i c`; `! a`; `a | b`; `a | b | c`;
`! a | b`; `x && y`; `x || y`; `x && y || z`; `a; b; c`; `a & b`; `p &`;
`a &&\n b`; `echo "$x" $y 'z'`; `name+=v cmd`; `arr[0]=v cmd`; empty input → `None`.

**Deferred corpus** (asserts `parser::parse_sequence` → `Err(UnsupportedCommand)`):
`( a )`; `(( 1+2 ))`; `if true; then x; fi`; `while x; do y; done`;
`for i in a; do x; done`; `case x in y) z;; esac`; `{ a; }`; `[[ -n x ]]`;
`f() { x; }`; `cat <<EOF\n…\nEOF`; `cat <<<word`; `coproc x`.

Plus unit tests for the assignment-split and redirect-order internals.

## 4. The `parse_sequence` contract (match `command::parse` exactly)

`parser::parse_sequence` must reproduce `command::parse`'s exact behavior for the
in-scope subset: the same `None`-on-empty, the same newline handling, the same
single-command representation (`Command::Simple` vs `Command::Pipeline`), the same
`background` handling, the same source `line` numbers on `ExecCommand`, and the same
inline-assignment splitting. The differential test enforces every one of these — any
divergence is a `got != want` failure to fix in `parser.rs`.

## 5. Open / edges (resolve in the plan)

- **Single command vs Pipeline wrapping:** confirm from `command.rs`
  (`parse_command_then_pipeline`) whether one command is `Command::Simple(..)` or
  `Command::Pipeline { commands: [one] }` — reproduce it. (The differential pins it.)
- **`line` numbers:** `ExecCommand.line` is the 1-based source line of the first
  token (`peek_span`); match `command.rs`'s exact assignment of it.
- **Assignment vs command word:** the `NAME=value` / `WordPart::AssignPrefix` split
  must match `try_split_assignment` exactly (incl. `+=`, `[i]=`, and a bare-assign-only
  line → `SimpleCommand::Assign`).
- **`!` negate:** `!` as a pipeline-negate prefix vs `!` reserved word — mirror
  `command.rs`. A lone `!` / `! ` toggles `Pipeline.negate`.
- **RedirFd + dup operators:** reproduce `command.rs`'s `Redirection` construction for
  `3>`, `2>&1`, `{fd}>`, `>&`, `<&`, `&>`, `>|`, `<>` exactly (the lexer pre-emits
  `RedirFd` + the redirect `Op`).
- **Connector vs background `&`:** `&` mid-list is `Connector::Amp`; a trailing `&`
  sets `Sequence.background` — match `command.rs`'s rule.
- **Where `UnsupportedCommand` is returned vs where `command.rs` succeeds:** every
  deferred construct must short-circuit BEFORE `parser.rs` partially consumes it in a
  way that diverges — return `UnsupportedCommand` at the dispatch peek.
