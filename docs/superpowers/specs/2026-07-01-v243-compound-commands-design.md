# v243 — compound commands in the parser-driven command parser (design)

**Status: DESIGN (approved direction).** Date: 2026-07-01.

Extends v242's dormant flat command-list parser in `crates/huck-syntax/src/parser.rs`
to the **command-list-body compound commands**, differential-tested against
`command.rs`. This continues the Stage-2 command parser incrementally. Direction:
memory `huck-frontend-parser-driven-direction`; prior: v242 (flat command parser).

## Goal

Replace v242's compound-deferral seams (`parse_command` returning
`UnsupportedCommand` for keyword/`(`/`ArithBlock` openers) with real parsers for the
control-flow compounds, producing the SAME `Command` AST `command.rs` produces —
verified by a differential corpus. `command.rs` is the oracle and stays untouched.

## Scope (in)

The recursion enabler + these compounds (each mirrors a `command.rs` function):

- **Recursion enabler:** add `stop_at: &[Keyword]` to `parse_and_or` (mirroring
  `command.rs`'s `parse_sequence_opts`): break — WITHOUT consuming — when a peeked
  token is a keyword in the stop set. Add a `parser`-local `Keyword` enum + a
  `keyword_kind(&TokenKind) -> Option<Keyword>` (mirror `command.rs`'s `keyword_of`).
  A `parse_compound_section(iter, stop_at, unterminated_err)` helper (maps
  `MissingCommand`-at-EOF → the compound's `unterminated` error, mirroring
  `command.rs`).
- **Compound dispatch:** in `parse_command`, replace the deferral seams with dispatch
  to the compound parsers below (and mirror the same dispatch in a `parse_next_stage`
  equivalent so a compound can be a pipeline stage). Wrap trailing redirects via a
  `maybe_wrap_redirects` helper → `Command::Redirected { inner, redirects }` (mirror
  `command.rs`). Reuse the v242 redirect helpers.
- **Subshell** `( … )` → `Command::Subshell { body }` — mirror `parse_subshell` /
  `parse_subshell_sequence` (a sequence loop terminating on `Op(RParen)`; empty `()` →
  `EmptySubshell`).
- **Brace group** `{ … }` → `Command::BraceGroup(body)` — `parse_and_or(&[RBrace])`.
- **`if … then … [elif …] [else …] fi`** → `Command::If(IfClause{condition, then_body,
  elif_branches, else_body})` — the section stop sets are `[Then]`, `[Elif,Else,Fi]`, etc.
- **`while`/`until` … `do` … `done`** → `Command::While(WhileClause{condition, body,
  until})` — `[Do]` then `[Done]`.
- **`for NAME [in WORDS]; do … done`** (POSIX) → `Command::For(ForClause{var, words,
  has_in, body})` — bespoke `in`-word-list loop (Words until newline/`;`/`do`), body
  `[Done]`.
- **`select NAME [in WORDS]; do … done`** → `Command::Select(SelectClause{var, words,
  body})` — like `for` but `words: Option<Vec<Word>>`.
- **`case WORD in … esac`** → `Command::Case(CaseClause{subject, items})` — bespoke
  pattern-list sub-grammar (optional `(`, `pat (| pat)*`, `)`, optional body via
  `parse_and_or(&[Esac])`, terminator `;;`/`;&`/`;;&` → `CaseTerminator`).
- **Arith command `(( … ))`** → `Command::Arith(Word)` — consume the `ArithBlock(text,
  opts)` token and call **`crate::lexer::arith_string_to_word(&text, opts)`** (the SAME
  function `command.rs` uses). The body is a `Word` of literal runs + `Var`/
  `ParamExpansion`/`CommandSub`/`Arith` parts — a double-quote-like expandable string
  (`(( x + $y ))`, `(( $(cmd) + 1 ))`), expanded at runtime then evaluated by `arith.rs`.
  Reusing `arith_string_to_word` makes the body `Word` match the oracle by construction.

All reuse the existing AST verbatim (`IfClause`/`WhileClause`/`ForClause`/`CaseClause`/
`SelectClause`/`Command::*`) — NO AST change, engine untouched.

## Non-goals (deferred → `ParseError::UnsupportedCommand`)

- **`[[ … ]]`** double-bracket test — a whole 4-level Pratt test-expression grammar
  (36 operators, `TestExpr` AST). Its own iteration (v244).
- **Function definition** `NAME() compound` / `function NAME` — cheap follow-on once
  the compounds exist (body reuses `parse_command`), but deferred to keep v243 focused.
- **`coproc`** — same (body is a `parse_command`); deferred. (Also note `command.rs`
  rejects `coproc` as a pipeline stage.)
- **C-style `for (( … )); do … done`** (ArithFor) — the `((init;cond;step))` header
  needs the arith-string split; deferred.
- **Heredocs / here-strings** — still deferred (v242 boundary).
- **Command substitution in words** — pre-built by the lexer; corpus keeps words simple.

## Global constraints

- **Byte-identical / dormant:** `parser.rs`'s parser is reached ONLY by tests;
  `command.rs`'s parser, `Command` mode, and the engine are untouched. `cargo test
  --workspace` green, 0 warnings; release harness sweep byte-identical.
- **No lexer change** — consume the production `Command`-mode tokens.
- **`command.rs` is the ORACLE** — fix `parser.rs` to match on any `diff_cmd`
  mismatch; never weaken the comparison or edit `command.rs`.
- Reuse `command.rs`/`lexer.rs` helpers via `pub(crate)` where it avoids duplication
  (e.g. `arith_string_to_word` is already `pub(crate)`; a visibility bump on a subtle
  helper is acceptable — behavior-neutral, protects only against copy-drift).
- No new `ParseError` variant needed beyond v242's `UnsupportedCommand` (the deferred
  compounds keep using it); reuse the existing compound `ParseError`s
  (`UnterminatedIf`/`UnterminatedLoop`/`UnterminatedBrace`/`EmptySubshell`/etc.) to
  match the oracle's errors.

## Testing (the proof)

Same differential harness as v242 (`old_seq` = `command::parse` = ORACLE, `new_seq` =
`parser::parse_sequence`, `diff_cmd` asserts full `Sequence` AST equality;
`diff_unsupported` asserts the deferred boundary).

**In-scope corpus** (each `diff_cmd`): `( a; b )`, `( a | b )`, `( (nested) )`, `()`
(EmptySubshell — via `assert_eq!` on the `Err`), `{ a; b; }`, `if x; then y; fi`,
`if x; then y; else z; fi`, `if a; then b; elif c; then d; else e; fi`,
`while x; do y; done`, `until x; do y; done`, `for i in a b c; do echo $i; done`,
`for i; do x; done` (no-`in`), `select x in a b; do y; done`,
`case $x in a) 1;; b|c) 2;; *) 3;; esac`, `case x in a) ;; esac` (empty body),
nested/recursive (`if x; then for i in a; do y; done; fi`, `{ ( a ); }`,
`while x; do case $y in z) w;; esac; done`), pipelines with compound stages
(`if x; then y; fi | cat`, `a | { b; }`), trailing redirects
(`{ a; } >f`, `while x; do y; done <f`, `for i in a; do x; done 2>&1`),
and **arith commands** (`(( 1+2 ))`, `(( x + $y ))`, `(( ${n} * 2 ))`, `(( $(cmd)+1 ))`,
`(( a )) && echo`, arith as a pipeline-stage/redirected form).

**Deferred corpus** (`diff_unsupported`): `[[ -n x ]]`, `f() { x; }`, `function f { x; }`,
`coproc x`, `for ((i=0;i<3;i++)); do x; done`, `cat <<<w`.

## Open / edges (resolve in the plan)

- **`parse_next_stage` mirror:** a compound as a pipeline stage AFTER `|` — reproduce
  `command.rs`'s `parse_next_stage` dispatch exactly (incl. `coproc` rejected as a stage).
- **`maybe_wrap_redirects`** placement — after EVERY compound parser return, in both
  the command-position and pipeline-stage dispatch, matching `command.rs`.
- **`for`/`select` `in`-word-list** termination (newline/`;`/`do`) — match `command.rs`.
- **`case` pattern grammar** — optional leading `(`, `|`-separated patterns, `)`, the
  three terminators, and the implicit `Break` at `esac`; match `parse_case`/`parse_case_item`.
- **Subshell stop mechanism** — `parse_subshell_sequence` stops on `Op(RParen)` (not a
  keyword); reproduce its connector loop exactly (incl. `EmptySubshell`).
- **`stop_at` threading** — a body's `parse_and_or(stop_at)` recurses into nested
  compounds which push their OWN stop sets; confirm the nesting matches `command.rs`
  (e.g. `if … then while … do … done … fi`).
- **Error parity** — each compound's `Unterminated*`/`Empty*` error must match the
  oracle for the truncated/invalid corpus cases (use `assert_eq!(new_seq, old_seq)` on
  the `Err`, like v242's `cmd_invalid_double_background`).
