# Tier 1 Public-API Surface Cleanup — Design

**Issue:** [#99 — Tier 1 public-API surface cleanup for huck-syntax and huck-engine](https://github.com/jdstanhope/huck/issues/99)

**Date:** 2026-07-09

## Motivation

An API consistency/understandability review of huck's two consumable crates
(`huck-syntax`, `huck-engine`) found the *core* designs sound — `Engine` /
`ExecBuilder` return exit codes uniformly, the syntax AST enums are
`#[non_exhaustive]` — but the **public surface** is inconsistent and hard to
navigate:

- `huck-engine` declares **24 `pub mod`s**; only `engine` + `exec_builder` are
  the intended embedder entry point. ~11 are accidentally public (referenced by
  no other crate), and ~10 are public *only* so `huck-cli` (the REPL) can reach
  them — all competing for attention with `Engine`.
- `huck-syntax` has the opposite problem: a curated root surface that omits the
  first two pipeline stages. `generate::{command_to_source, function_to_source}`
  are re-exported, but `Lexer` and `parser::parse_sequence` — how you actually
  *start* — are reachable only by full module path.
- Both crates carry stale documentation: `huck-syntax/src/lib.rs` points at
  `examples/` programs that do not exist, and `parser.rs` comments reference
  functions (`command::parse`, `parse_cursor`, `parse_one_unit`) that no longer
  exist.

There are **no external consumers** of either crate yet, so every change here is
non-breaking. This iteration is the high-value, low-risk subset of the review
(Tier 1): visibility changes, re-exports, one rename, and honest docs. Naming
polish, `#[non_exhaustive]` policy, and the error-emitter rework are deferred
(Tier 2/3).

## Non-goals (deferred to Tier 2/3)

- `#[non_exhaustive]` on engine data types (`Output`, `Completion`, `Candidate`,
  `CandidateKind`).
- Error-emitter surface consolidation (`emit_error` / `emit_error_to` /
  `emit_cli_error` / `emit_syntax_error` / `Diag` / the `sh_error!` macros).
- `Redirect` vs `Redirection` naming; making root re-exports fully self-contained
  (re-exporting AST clause types like `IfClause`, `ForClause`).
- Builder-knob consistency (`restricted(bool)` vs presence-only siblings;
  `EngineBuilder::with_version` vs bare `env`/`arg0`/`args`).
- The bulk of missing doc-comment writing on public items.

## Design

### A. huck-engine — demote accidentally-public modules to `pub(crate)`

These modules are referenced by no crate other than `huck-engine` itself
(verified: `huck-cli` imports only `continuation, traps, shell_state, shell,
readline_bind, prompt, jobs, history, completion, builtins`). They are internal
implementation detail and become `pub(crate) mod`:

`arith`, `completion_builtins`, `completion_spec`, `err_thread_local`, `expand`,
`glob_match`, `job_spec`, `param_expansion`, `procsub`, `test_builtin`,
`executor`.

`executor` is a special case: `huck-cli` does not import the module path, but the
crate root re-exports `StdoutSink`/`StderrSink` from it via
`pub use executor::{StderrSink, StdoutSink}`. Making `executor` `pub(crate)`
keeps that root re-export working (the *types* stay `pub`; only the module path
is hidden) — this is the standard "hide the module, expose the type" pattern.

**Acceptance:** `huck-engine`, `huck-cli`, and the `huck` binary all still build.
`huck_engine::arith::…` etc. no longer resolve from outside the crate;
`huck_engine::StdoutSink` still resolves.

### B. huck-engine — `#[doc(hidden)]` the cli-only modules

These stay `pub mod` (because `huck-cli` imports them by path) but gain
`#[doc(hidden)]` so they drop out of the documented/curated surface:

`builtins`, `continuation`, `history`, `jobs`, `prompt`, `readline_bind`,
`shell`, `shell_state`, `traps`, `completion`.

`completion` is `#[doc(hidden)]` at the *module* level, but its embedder-facing
types `Candidate` / `CandidateKind` remain surfaced through the existing crate
-root re-export `pub use completion::{Candidate, CandidateKind}` (which is not
hidden). `error_emit` is intentionally **left fully public** — its rework is
Tier 2.

**Acceptance:** `huck-cli` still builds (module paths still resolve). Rustdoc for
`huck-engine` no longer lists these modules at the top level; `Engine`,
`ExecBuilder`, `Output`, `Completion`, `Candidate`, `CandidateKind`, and
`error_emit` remain visible.

### C. huck-engine — `#[doc(hidden)]` the Shell-cell escape hatch

`Engine::from_shell_cell` and `Engine::shell_cell` traffic in
`Rc<RefCell<Shell>>` — the crate's deepest internal type and a `RefCell`
borrow-panic hazard. They stay `pub` (so `huck-cli` and advanced/dogfood callers
keep working) but gain `#[doc(hidden)]`.

**Acceptance:** both methods still callable; neither appears in rustdoc for
`Engine`.

### D. huck-engine — rename `Engine::exec` → `Engine::prepare`

`e.exec("script")` returns an `ExecBuilder` (configure, then `.run()`/`.capture()`),
but next to `run`/`capture` (which execute immediately) the name reads like a
third "run now" verb. Rename to `prepare`, which signals "set up, doesn't run yet."

Sites to update (all in-repo, no external users):
- The method definition in `engine.rs`.
- `Engine::capture`'s internal `self.exec(src).capture()` call.
- The `engine.rs` module-level doctest (`e.exec(...)` usages).
- Any internal unit/integration tests calling `.exec(`.
- `exec_builder.rs` doc comments that reference `Engine::exec`.
- The site's Library page (`site/app/library/page.tsx`) — the `engineExecExample`
  snippet uses `e.exec(...)`.

**Acceptance:** no `Engine::exec` / `.exec(` references to the *Engine* method
remain (the `ExecCommand` AST type and `ExecBuilder` are unrelated and unchanged);
doctests pass; the site builds.

### E. huck-syntax — surface the pipeline entry points

1. **Re-export `Lexer`** — add it to the existing
   `pub use lexer::{ … }` list in `lib.rs`.
2. **Re-export `parse_sequence`** — add `pub use parser::parse_sequence;` in
   `lib.rs`.
3. **Add a `parse` convenience function.** Define in `parser.rs` and re-export at
   the crate root:

   ```rust
   /// Parse shell source into a command AST using the default lexer
   /// configuration (no aliases, default `LexerOptions`). Returns `Ok(None)`
   /// for empty or comment-only input. For alias expansion or custom options,
   /// build a `Lexer` explicitly and call `parse_sequence`.
   pub fn parse(src: &str) -> Result<Option<Sequence>, ParseError> {
       let mut lx = Lexer::new(src, &Default::default(), LexerOptions::default());
       parse_sequence(&mut lx)
   }
   ```

   The `None`-for-empty shape matches `parse_sequence` exactly; no new
   error semantics are introduced.

**Acceptance:** `use huck_syntax::{Lexer, parse_sequence, parse};` resolves;
`parse("echo hi")` returns `Ok(Some(_))`, `parse("")` and `parse("# c")` return
`Ok(None)`, `parse("if")` returns `Err(_)`.

### F. huck-syntax — write the two referenced examples

`lib.rs` tells readers to run two examples that do not exist. Write them under
`crates/huck-syntax/examples/` (Cargo auto-discovers `examples/*.rs`):

- **`tokenize_dump.rs`** — take a shell string (a hardcoded sample, or `argv[1]`
  if provided), build a `Lexer`, pull tokens via the public token-cursor API
  (`next()` returning `Result<Option<Token>, LexError>`), and print each
  `Token`'s kind + span until exhaustion. Demonstrates the lexer entry point.
- **`list_assignments.rs`** — take a shell string, call the new `parse`, walk the
  resulting `Sequence`/`Command` AST, and print every assignment it finds (name +
  whether it is an append `+=`), using the public `try_split_assignment` /
  `Assignment` / `AssignTarget` surface. Demonstrates the parse entry point and
  AST walking.

Both must compile and run cleanly via
`cargo run --example <name> -p huck-syntax`.

**Acceptance:** both examples build and run and produce sensible output; the
`lib.rs` doc references now resolve to real programs.

### G. huck-syntax — fix stale doc cross-references

- `lib.rs` — keep the `examples/` references (now real after F); verify the
  invocation lines are exactly the file names.
- `parser.rs` — update comments that reference the nonexistent `command::parse`,
  `parse_cursor`, and `parse_one_unit` in `command.rs` to point at the real
  current functions (`parse_sequence` / `parse_one_unit` live in `parser.rs`), or
  remove the stale cross-reference if it no longer adds value.

**Acceptance:** no doc comment references a symbol that does not exist.

## Testing strategy

Per the repo's OOM constraint, run tests **per-crate**, single-threaded:

- `cargo build -p huck-syntax -p huck-engine` and `cargo build -p huck` (binary).
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (~437 tests).
- `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` (the `lib.rs`
  quick-start doctest, plus any new doctest on `parse`).
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1779 tests).
- `cargo run --example tokenize_dump -p huck-syntax` and
  `cargo run --example list_assignments -p huck-syntax` — must succeed.
- `cargo build -p huck-cli` — proves the `pub(crate)` demotions (A) did not sever
  a path `huck-cli` relies on, and the `#[doc(hidden)]` modules (B/C) stay
  reachable.
- `cd site && npm run build` — proves the `exec → prepare` rename (D) in the
  Library page compiles and the site prerenders.
- `cargo fmt --all --check`.

Add focused unit tests for the new `parse` function (the four acceptance cases in
E). The examples serve as compile-tested living documentation; they do not need
separate assertions beyond running successfully.

## Risks

- **Low.** Every change is visibility, re-export, a mechanical rename, or new
  additive code. The one cross-crate risk — a `pub(crate)` demotion breaking
  `huck-cli` — is caught by building `huck-cli`, and the module list was chosen
  precisely because `huck-cli` does not import those paths.
- The `exec → prepare` rename touches the site; the site build gates it.
