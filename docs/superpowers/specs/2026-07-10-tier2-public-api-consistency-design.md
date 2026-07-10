# Tier 2 Public-API Consistency Polish — Design

**Issue:** [#102 — Tier 2 public-API consistency polish for huck-syntax and huck-engine](https://github.com/jdstanhope/huck/issues/102)

**Date:** 2026-07-10

## Motivation

Tier 1 ([#99](https://github.com/jdstanhope/huck/issues/99), PR #100) hid huck's
internal module surface and surfaced the huck-syntax pipeline entry points. Tier 2
is the consistency-polish subset of the same API review: it makes the two crates'
public types read uniformly and closes small naming/redundancy gaps. There are
**no external consumers** of either crate, so nothing here is breaking.

One review item — consolidating the error-emitter surface — is **out of scope**,
split to [#101](https://github.com/jdstanhope/huck/issues/101): it is entangled
with `huck-cli`'s direct `emit_*` calls and sits on the redirect-aware routing
that regressed three times in v269, so it warrants a dedicated iteration.

## Non-goals

- Error-emitter consolidation (#101).
- Tier 3 doc-comment writing (undocumented modules/fields/variants).
- Any behavior change. Every item is `#[non_exhaustive]`, a re-export, a rename,
  a signature tweak, or a redundant-fn removal.

## Design

### A. `#[non_exhaustive]` on the engine return types

Add `#[non_exhaustive]` to `Output` (`engine.rs:62`), `Completion` (`engine.rs:75`),
`Candidate` (`completion.rs:24`), and `CandidateKind` (`completion.rs:8`). These are
values the engine **returns** — consumers read fields (`Output`/`Completion`/
`Candidate`) or match (`CandidateKind`), they don't construct them — so the attribute
is low-cost for consumers and makes adding a field/variant later non-breaking. This
aligns huck-engine with how huck-syntax already marks its AST enums (`Token`,
`WordPart`, `Command`, `ParseError`, …).

`#[non_exhaustive]` affects only *other* crates; in-crate construction (the engine
building these values, in-crate tests) is unaffected. Consumers that exhaustively
match `CandidateKind` will need a `_ =>` arm — the intended contract.

**Acceptance:** the four types carry `#[non_exhaustive]`; `huck-engine` and
`huck-cli` build (in-crate/sibling construction unaffected); the engine lib tests
that read these values still pass.

### B. Self-contained huck-syntax root re-exports

A consumer using only `huck_syntax::` root imports currently cannot name the type of
some public fields, nor destructure some public variants, of already-root-exported
types (`Command`, `ExecCommand`, `WordPart`, `ParamModifier`). Re-export the
transitive closure of types reachable through their public fields/variants. Add these
root re-exports (all are already `pub`, just module-only — purely additive):

- **Redirection family** (reachable from `ExecCommand.redirects` and the `slot_*`
  methods): `Redirection`, `RedirFd`, `RedirOp`, `FileMode`, `RedirectSlot` (the
  renamed slot enum — see C).
- **`Command` clause types**: `IfClause`, `WhileClause`, `ElifBranch`, `ForClause`,
  `SelectClause`, `ArithForClause`, `CaseClause`, `CaseItem`, `CaseTerminator`,
  `Connector`, `TestExpr`, `TestUnaryOp`, `TestBinaryOp`.
- **`WordPart` / `ParamModifier` payload types**: `TildeSpec`, `QuoteStyle`,
  `ProcDir`, `ArrayLiteralElement`, `SubstAnchor`, `CaseDirection`.

No name collisions exist with the current root exports (verified against the
existing `lib.rs` re-export list). `brace_expand::expand` remains intentionally
non-re-exported (module/function name collision — pre-existing decision, unchanged).

**Acceptance:** a scratch `use huck_syntax::{Command, IfClause, Redirection, RedirectSlot, TildeSpec, …};`
compiles; every public field type and variant payload of a root-exported type is
itself nameable from the root. `huck-syntax` builds and its lib tests pass.

### C. Rename the slot-view enum `Redirect` → `RedirectSlot`

`command.rs` has two confusably-named redirection types: `Redirection` (the full
parsed AST redirection stored in `ExecCommand.redirects`) and `Redirect` (a
simplified per-standard-stream *slot view* returned by
`ExecCommand::slot_stdin`/`slot_stdout`/`slot_stderr` and produced by
`slots_for_simple_path`). Rename the **slot-view** enum to `RedirectSlot` so the
AST type keeps the natural `Redirection` name.

Sites: `command.rs` (the `enum Redirect` def + variants + the `slot_*` methods'
return type `Option<Redirect>` + `slots_for_simple_path`), and its consumers in
`executor.rs` (~29 refs), `generate.rs` (~9 refs), `parser.rs` (1 ref). The token
`\bRedirect\b` matches only the standalone type — it does NOT match `Redirection`,
`RedirFd`, `RedirOp`, or the `Command::Redirected` variant — so a word-boundary
rename is safe, but each changed file must be compiled to confirm.

**Acceptance:** no `\bRedirect\b` type reference remains (only `Redirection` /
`RedirectSlot` / `RedirFd` / `RedirOp` / `Redirected`); `huck-syntax` + `huck-engine`
build and their lib tests pass; `RedirectSlot` is the type re-exported in B.

### D. Builder-knob consistency

1. **`ExecBuilder::restricted(on: bool)` → `restricted(self) -> Self`** (presence-only,
   like its sibling `merge_stderr()`). `exec_builder.rs:187`. Body sets the restricted
   flag to `true` unconditionally. Every current caller passes `true`, so each
   `.restricted(true)` becomes `.restricted()`. Call sites: `engine.rs` tests
   (~15) + module doctest (`engine.rs:30`), `crates/huck-engine/examples/engine_sandbox_diff.rs`
   (`.restricted(true)` at :24 and the conditional `b = b.restricted(true)` at :34,
   plus its `//!` doc comment lines :5/:7), and `site/app/library/page.tsx:45`
   (`.restricted(true)` in the `engineExecExample` snippet). The
   `shell_state.rs:694` reference is prose in a doc comment describing the mode —
   update the wording to `.restricted()`.

2. **`EngineBuilder::with_version(v)` → `version(v)`** — drop the `with_` prefix to
   match the bare sibling methods `env`/`arg0`/`args`. `engine.rs:279`. One call
   site: the test `builder_with_version_sets_huck_version` (`engine.rs:1380`) and its
   fn name should read naturally (rename the test to `builder_version_sets_huck_version`).

**Acceptance:** `restricted()` takes no argument; `version()` replaces `with_version`;
all builds (incl. `huck-engine` examples + site) pass; the renamed test passes.

### E. Remove the redundant `lex_error_message` / `parse_error_message`

These two `huck-syntax` free functions (`errors.rs:13`/`:17`) are thin wrappers that
delegate to the crate-private `lex_error_message_impl` / `parse_error_message_impl`
— exactly what the `Display` impls on `LexError` / `ParseError` already call. The
crate docs already call them "historical wrappers." Remove them:

- Delete `pub fn lex_error_message` and `pub fn parse_error_message` from
  `errors.rs`. **Keep** the crate-private `*_impl` functions (the `Display` impls at
  `lexer.rs:37` and `command.rs:769`, and `errors.rs:84`/`:89`, depend on them).
- Drop them from both root re-exports: `huck-syntax` `lib.rs:80`
  (`pub use errors::{lex_error_message, parse_error_message};` → remove the line),
  and `huck-engine` `lib.rs:66`
  (`pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};`
  → keep only `escape_double_quote_value`).
- Migrate the `huck-engine` callers to `Display`:
  - `shell.rs:459`: `crate::parse_error_message(&e)` → `e` used via `format_args!("syntax error: {e}")` (or `.to_string()`).
  - `builtins.rs:7322`/`:7454`: these construct `ParseError::Lex(...)` then message it — replace with `ParseError::Lex(...).to_string()`.
  - `builtins.rs:7482`: `crate::parse_error_message(&e)` → `e.to_string()`.
- Update `errors.rs` tests: the two that assert `format!("{err}") == lex_error_message(&err)` / `== parse_error_message(&err)` (`errors.rs:115`/`:121`) become tautologies once the wrapper is gone — delete them. The test at `errors.rs:127`-134 asserts real `ParseError::Lex` Display behavior (no leading `": "`) — keep it, but call `err.to_string()` / `format!("{err}")` instead of `parse_error_message(&err)`.
- `errors.rs:58` (`crate::lex_error_message(e)`) is inside `lex_error_message_impl`'s
  Substitution arm — replace with `e.to_string()` (or the `_impl` call) so the file
  no longer references the removed public fn.

`BraceError` already has only a `Display` impl (no `brace_error_message`), so this
removal moves all three error types to the consistent Display-only surface.

**Acceptance:** the two functions and their re-exports are gone; `grep -rn
'lex_error_message\b\|parse_error_message\b'` (excluding the `*_impl` names) returns
nothing in `crates/*/src`; `huck-syntax` + `huck-engine` + `huck-cli` build; error
messages are byte-identical to before (Display already produced them).

## Testing strategy

Per the repo's OOM constraint, per-crate + single-threaded:

- `cargo build -p huck-syntax -p huck-engine`, `cargo build -p huck-cli`,
  `cargo build -p huck`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `--doc`.
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` and `--doc`.
- `cargo build --examples -p huck-syntax -p huck-engine` and run the engine
  `engine_sandbox_diff` / syntax examples to confirm the `restricted()` change and
  re-exports hold.
- `cd site && npm run build` — proves the `restricted()` edit in the Library page.
- `cargo fmt --all --check`.
- A scratch compile (in a throwaway test or example) exercising the new root
  re-exports (B) — the syntax examples already `use huck_syntax::{…}` and can carry
  one added import to prove self-containment.

## Risks

Low. All changes are additive re-exports, mechanical renames, or removals of pure
duplication. The two cross-crate risks — the `Redirect` rename missing a consumer,
and a removed error fn leaving a dangling caller — are both caught by building
`huck-engine` + `huck-cli`. `#[non_exhaustive]` is caught at compile time if any
sibling crate constructs the types (none does; verified `huck-cli` only reads them).
