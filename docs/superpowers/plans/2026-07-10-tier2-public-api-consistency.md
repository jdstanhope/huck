# Tier 2 Public-API Consistency Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck-syntax and huck-engine's public API read uniformly — self-contained re-exports, a clearer slot-type name, `#[non_exhaustive]` on engine return types, consistent builder knobs, and removal of redundant error-message functions.

**Architecture:** Five independent tasks. Task 1 renames the slot enum (`Redirect`→`RedirectSlot`) across both crates; Task 2 depends on it to add self-contained huck-syntax root re-exports. Tasks 3 (non_exhaustive), 4 (builder knobs), and 5 (remove error fns) are independent of everything else.

**Tech Stack:** Rust (edition 2024), Cargo workspace (`huck-syntax`, `huck-engine`, `huck-cli`, `huck` binary), Next.js site under `site/`.

**Reference:** Spec at `docs/superpowers/specs/2026-07-10-tier2-public-api-consistency-design.md` (issue [#102](https://github.com/jdstanhope/huck/issues/102)).

## Global Constraints

- **No external consumers exist** — every change is non-breaking; do not add deprecated aliases or shims.
- **No behavior change** — messages/outputs stay byte-identical; these are surface changes only.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Never run `cargo test --workspace`** (OOM). Test per-crate: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **Format before every commit:** `cargo fmt --all` (CI enforces `--check`).
- **Out of scope:** the error-emitter consolidation (issue #101) — do not touch `emit_error`/`emit_error_to`/`emit_cli_error`/`emit_syntax_error`/`Diag`/`sh_error!`.

---

### Task 1: Rename `Redirect` → `RedirectSlot` (spec C)

`command.rs` has two confusable types: `Redirection` (full AST redirection) and `Redirect` (a per-standard-stream slot view returned by `ExecCommand::slot_*`). Rename the slot enum to `RedirectSlot`.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (~12 refs incl. the `enum Redirect` def)
- Modify: `crates/huck-syntax/src/generate.rs` (~9 refs)
- Modify: `crates/huck-syntax/src/parser.rs` (1 ref)
- Modify: `crates/huck-engine/src/executor.rs` (~29 refs)

**Interfaces:**
- Produces: the enum is now `RedirectSlot` (same variants: `Read`/`Truncate`/`Append`/`Clobber`/`Dup`/`Heredoc`/`HereString`); `ExecCommand::slot_stdin`/`slot_stdout`/`slot_stderr` now return `Option<RedirectSlot>`. Task 2 re-exports `RedirectSlot`.

- [ ] **Step 1: Rename in all four files via word-boundary sed.**

`\bRedirect\b` matches only the standalone type — NOT `Redirection`, `RedirFd`, `RedirOp`, or the `Command::Redirected` variant. Run:
```bash
cd /home/john/projects/huck
sed -i 's/\bRedirect\b/RedirectSlot/g' \
  crates/huck-syntax/src/command.rs \
  crates/huck-syntax/src/generate.rs \
  crates/huck-syntax/src/parser.rs \
  crates/huck-engine/src/executor.rs
```

- [ ] **Step 2: Confirm no unintended matches and the enum def renamed.**

```bash
grep -rn '\bRedirect\b' crates/ || echo "no bare Redirect remains"
grep -n 'pub enum RedirectSlot' crates/huck-syntax/src/command.rs
grep -n 'Redirection\|Redirected\|RedirFd\|RedirOp' crates/huck-syntax/src/command.rs | head -3
```
Expected: `no bare Redirect remains`; the `pub enum RedirectSlot` line prints; `Redirection`/`Redirected`/`RedirFd`/`RedirOp` are still present (untouched).

- [ ] **Step 3: Build both crates.**

Run: `cargo build -p huck-syntax -p huck-engine 2>&1 | tail -5`
Expected: `Finished`, no errors.

- [ ] **Step 4: Run the lib tests for both crates.**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
```
Expected: both `test result: ok.` (huck-syntax ~441, huck-engine ~1773).

- [ ] **Step 5: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/generate.rs crates/huck-syntax/src/parser.rs crates/huck-engine/src/executor.rs
git commit -m "$(printf 'refactor(#102): rename slot-view enum Redirect -> RedirectSlot\n\ncommand.rs had two confusably-named redirection types: Redirection (the full\nparsed AST redirection stored in ExecCommand.redirects) and Redirect (a\nsimplified per-standard-stream slot view returned by ExecCommand::slot_*).\nRename the slot view to RedirectSlot so the AST type keeps the Redirection name.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Self-contained huck-syntax root re-exports (spec B)

A consumer using only `huck_syntax::` root imports cannot currently name some public field types or destructure some public variants of root-exported types. Add the transitive closure of reachable types to the root re-exports. Depends on Task 1 (`RedirectSlot`).

**Files:**
- Modify: `crates/huck-syntax/src/lib.rs` (the `pub use command::{…}` and `pub use lexer::{…}` blocks)
- Modify: `crates/huck-syntax/examples/list_assignments.rs` (add one import to prove self-containment)

**Interfaces:**
- Consumes: `RedirectSlot` (Task 1).
- Produces: the following are now root-reachable: `Redirection`, `RedirFd`, `RedirOp`, `FileMode`, `RedirectSlot`, `IfClause`, `WhileClause`, `ElifBranch`, `ForClause`, `SelectClause`, `ArithForClause`, `CaseClause`, `CaseItem`, `CaseTerminator`, `Connector`, `TestExpr`, `TestUnaryOp`, `TestBinaryOp`, `TildeSpec`, `QuoteStyle`, `ProcDir`, `ArrayLiteralElement`, `SubstAnchor`, `CaseDirection`.

- [ ] **Step 1: Extend the `command::{…}` root re-export in `lib.rs`.**

Replace the existing block (currently):
```rust
pub use command::{
    AssignTarget, Assignment, Command, ExecCommand, ParseError, Pipeline, Sequence, SimpleCommand,
    try_split_assignment,
};
```
with:
```rust
pub use command::{
    ArithForClause, AssignTarget, Assignment, CaseClause, CaseItem, CaseTerminator, Command,
    Connector, ElifBranch, ExecCommand, FileMode, ForClause, IfClause, ParseError, Pipeline,
    RedirFd, RedirOp, RedirectSlot, Redirection, SelectClause, Sequence, SimpleCommand, TestBinaryOp,
    TestExpr, TestUnaryOp, WhileClause, try_split_assignment,
};
```
If rustfmt reorders these, that's fine — the set is what matters.

- [ ] **Step 2: Extend the `lexer::{…}` root re-export in `lib.rs`.**

Replace the existing block (currently):
```rust
pub use lexer::{
    LexError, Lexer, LexerOptions, ParamModifier, Span, SubscriptKind, Token, TokenKind, TransformOp,
    Word, WordPart,
};
```
with:
```rust
pub use lexer::{
    ArrayLiteralElement, CaseDirection, LexError, Lexer, LexerOptions, ParamModifier, ProcDir,
    QuoteStyle, Span, SubstAnchor, SubscriptKind, TildeSpec, Token, TokenKind, TransformOp, Word,
    WordPart,
};
```

- [ ] **Step 2b: If the compiler reports any of these names is not `pub` in its module, make it `pub`.**

All listed types are expected to already be `pub` (just module-only). If `cargo build` errors with "no `X` in the root" because a type is `pub(crate)`, promote that one type to `pub` in its module (`command.rs`/`lexer.rs`) — do NOT invent a new type. Note any such promotion in the report.

- [ ] **Step 3: Prove self-containment from the examples' import.**

In `crates/huck-syntax/examples/list_assignments.rs`, change the import line
`use huck_syntax::{parse, Assignment};` to also pull a redirection type and a clause type purely from the root:
```rust
use huck_syntax::{parse, Assignment, Command, IfClause, Redirection, RedirectSlot};
```
Then add, at the end of `main` (after the existing output), a compile-only reference so the imports are used (this documents the self-contained surface without changing behavior):
```rust
    // Self-contained-surface check: these types are all nameable from the crate
    // root (see the Tier 2 re-exports). Sizes are compile-time only.
    let _sizes = (
        std::mem::size_of::<Command>(),
        std::mem::size_of::<IfClause>(),
        std::mem::size_of::<Redirection>(),
        std::mem::size_of::<RedirectSlot>(),
    );
    let _ = _sizes;
```

- [ ] **Step 4: Build the crate and the examples.**

```bash
cargo build -p huck-syntax 2>&1 | tail -3
cargo build --examples -p huck-syntax 2>&1 | tail -3
```
Expected: both `Finished`. If Step 2b promotions were needed, they compile now.

- [ ] **Step 5: Run the syntax lib tests + the example.**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo run --example list_assignments -p huck-syntax -- 'a=1 b+=2 echo hi' 2>&1 | tail -4
```
Expected: lib tests `ok`; the example still prints `a=…` / `b+=…` and exits 0.

- [ ] **Step 6: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-syntax/src/lib.rs crates/huck-syntax/examples/list_assignments.rs
git commit -m "$(printf 'feat(#102): make the huck-syntax root re-exports self-contained\n\nRe-export the transitive closure of types reachable through already-root-exported\ntypes public fields/variants (redirection family + Command clause types +\nWordPart/ParamModifier payload types), so a root-imported Command/ExecCommand can\nbe fully destructured without hunting through module paths.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: `#[non_exhaustive]` on the engine return types (spec A)

Add `#[non_exhaustive]` to the four engine values consumers receive, aligning with huck-syntax's AST-enum policy.

**Files:**
- Modify: `crates/huck-engine/src/engine.rs` (`Output` at :62, `Completion` at :75)
- Modify: `crates/huck-engine/src/completion.rs` (`CandidateKind` at :8, `Candidate` at :24)

**Interfaces:**
- Produces: `Output`, `Completion`, `Candidate`, `CandidateKind` become `#[non_exhaustive]`. No field/variant changes.

- [ ] **Step 1: Add `#[non_exhaustive]` to `Output` and `Completion` in `engine.rs`.**

Insert `#[non_exhaustive]` on its own line immediately above each `pub struct`, after the existing `#[derive(...)]`:
```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Output {
```
and
```rust
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Completion {
```

- [ ] **Step 2: Add `#[non_exhaustive]` to `CandidateKind` and `Candidate` in `completion.rs`.**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CandidateKind {
```
and
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidate {
```

- [ ] **Step 3: Build huck-engine, huck-cli, and the binary.**

```bash
cargo build -p huck-engine 2>&1 | tail -3
cargo build -p huck-cli 2>&1 | tail -3
cargo build -p huck 2>&1 | tail -3
```
Expected: all `Finished`. `#[non_exhaustive]` only restricts OTHER crates; if `huck-cli` (a sibling crate) constructs any of these with a struct literal or matches `CandidateKind` exhaustively, the build fails here — if so, fix the huck-cli site to use `..` (struct) or a `_ =>` arm (match), and note it in the report. (Expected: no such site — huck-cli only reads these values.)

- [ ] **Step 4: Run the engine lib tests.**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3`
Expected: `test result: ok.` (~1773). In-crate construction is unaffected by `#[non_exhaustive]`.

- [ ] **Step 5: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/engine.rs crates/huck-engine/src/completion.rs
git commit -m "$(printf 'refactor(#102): #[non_exhaustive] on the engine return types\n\nOutput, Completion, Candidate, CandidateKind are values the engine RETURNS\n(consumers read/match, never construct), so marking them #[non_exhaustive] makes\nadding a field/variant non-breaking and aligns with huck-syntax AST-enum policy.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Builder-knob consistency (spec D)

`ExecBuilder::restricted(bool)` → `restricted()` (presence-only), and `EngineBuilder::with_version(v)` → `version(v)`.

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs` (`restricted` def at :187)
- Modify: `crates/huck-engine/src/engine.rs` (`with_version` def at :279; `.restricted(true)` test call sites; the `with_version` test at :1379-1380; the module doctest at :30)
- Modify: `crates/huck-engine/examples/engine_sandbox_diff.rs` (`.restricted(true)` at :24 and `b = b.restricted(true)` at :34; `//!` doc lines :5/:7)
- Modify: `crates/huck-engine/src/shell_state.rs` (doc-comment wording at :694)
- Modify: `site/app/library/page.tsx` (`.restricted(true)` at :45)

**Interfaces:**
- Produces: `ExecBuilder::restricted(self) -> Self` (no bool arg; enables restricted mode); `EngineBuilder::version(self, version: &str) -> Self` (replaces `with_version`).

- [ ] **Step 1: Change the `restricted` signature in `exec_builder.rs`.**

Current (`:187`):
```rust
    pub fn restricted(mut self, on: bool) -> Self {
        self.restricted = on;
        self
    }
```
Replace the signature + body so it is presence-only (keep the surrounding doc comment, but update any `on`-referencing wording):
```rust
    pub fn restricted(mut self) -> Self {
        self.restricted = true;
        self
    }
```
(If the field assignment uses a different field name than `self.restricted`, keep that field name and just set it to `true` unconditionally.)

- [ ] **Step 2: Update every `.restricted(true)` call site to `.restricted()`.**

```bash
cd /home/john/projects/huck
sed -i 's/\.restricted(true)/.restricted()/g' \
  crates/huck-engine/src/engine.rs \
  crates/huck-engine/examples/engine_sandbox_diff.rs \
  site/app/library/page.tsx
```
Then handle the two non-`(true)` forms by hand:
- `engine_sandbox_diff.rs:34` `b = b.restricted(true);` → `b = b.restricted();` (covered by the sed above — verify).
- `engine.rs` module doctest (`:30`) `.restricted(true)` → `.restricted()` (covered by the sed — verify it is inside the `//!` block).
- `engine_sandbox_diff.rs` `//!` doc lines (`:5`,`:7`) mention `.restricted(true)` in prose — update to `.restricted()`.
- `shell_state.rs:694` prose `under \`.restricted(true)\`` → `under \`.restricted()\``.

Verify none remain:
```bash
grep -rn 'restricted(true)' crates/ site/app/ || echo "no restricted(true) remains"
```
Expected: `no restricted(true) remains`.

- [ ] **Step 3: Rename `with_version` → `version` in `engine.rs`.**

Current (`:279`):
```rust
    pub fn with_version(mut self, version: &str) -> Self {
```
→
```rust
    pub fn version(mut self, version: &str) -> Self {
```
Update the one caller + test name (`:1379-1380`):
```rust
    fn builder_version_sets_huck_version() {
        let mut e = Engine::builder().version("9.9.9").build();
```
Verify no `with_version` remains: `grep -rn 'with_version' crates/ || echo "clean"` → `clean`.

- [ ] **Step 4: Build engine + examples + binary, run engine lib + doctests.**

```bash
cargo build -p huck-engine 2>&1 | tail -3
cargo build --examples -p huck-engine 2>&1 | tail -3
cargo build -p huck 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --lib builder_version -- --test-threads 1 2>&1 | tail -4
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --doc -- --test-threads 1 2>&1 | tail -3
```
Expected: builds `Finished`; `builder_version_sets_huck_version` passes; full lib `ok`; doctest `ok` (the module doctest now uses `restricted()`).

- [ ] **Step 5: Build the site.**

```bash
cd site && npm run build 2>&1 | tail -4 && cd ..
```
Expected: build succeeds, `/library` prerenders. (If `site/node_modules` is missing, `npm install` in `site/` first.)

- [ ] **Step 6: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/exec_builder.rs crates/huck-engine/src/engine.rs crates/huck-engine/examples/engine_sandbox_diff.rs crates/huck-engine/src/shell_state.rs site/app/library/page.tsx
git commit -m "$(printf 'refactor(#102): consistent builder knobs (restricted(), version())\n\nExecBuilder::restricted(bool) -> restricted() (presence-only, like merge_stderr());\nEngineBuilder::with_version(v) -> version(v) (drop with_ prefix, matches env/arg0/args).\nUpdated call sites in tests, the engine_sandbox_diff example, the module doctest,\nand the site Library page.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Remove redundant `lex_error_message` / `parse_error_message` (spec E)

These two huck-syntax free functions duplicate the `Display` impls (docs call them "historical wrappers"). Remove them; migrate callers to `Display`. **Keep** the crate-private `*_impl` functions.

**Files:**
- Modify: `crates/huck-syntax/src/errors.rs` (delete the two `pub fn`s; fix the internal caller at :58; update tests at :115/:121/:127)
- Modify: `crates/huck-syntax/src/lib.rs:80` (drop the `errors::{…}` re-export line)
- Modify: `crates/huck-engine/src/lib.rs:66` (drop the two names from the `pub use huck_syntax::{…}` line)
- Modify: `crates/huck-engine/src/shell.rs:459` (migrate to Display)
- Modify: `crates/huck-engine/src/builtins.rs:7322,7454,7482` (migrate to Display)

**Interfaces:**
- Produces: `huck_syntax::lex_error_message` / `parse_error_message` no longer exist; use `LexError`/`ParseError`'s `Display` (`format!("{err}")` / `err.to_string()`). The crate-private `lex_error_message_impl` / `parse_error_message_impl` are unchanged.

- [ ] **Step 1: Delete the two public wrapper functions in `errors.rs`.**

Remove:
```rust
pub fn lex_error_message(error: &LexError) -> String {
    lex_error_message_impl(error)
}

pub fn parse_error_message(error: &ParseError) -> String {
    parse_error_message_impl(error)
}
```
Leave `lex_error_message_impl` and `parse_error_message_impl` (crate-private) in place — they back the `Display` impls.

- [ ] **Step 2: Fix the internal caller at `errors.rs:58`.**

`errors.rs:58` calls `crate::lex_error_message(e)` (the fn being deleted). Change it to call the impl directly: `lex_error_message_impl(e)` (it is in the same module). Verify the surrounding function still compiles.

- [ ] **Step 3: Update the `errors.rs` tests.**

- The two tests asserting `format!("{err}") == lex_error_message(&err)` (`:115`) and `== parse_error_message(&err)` (`:121`) become tautologies once the wrapper is gone — DELETE both test functions.
- The test at `:127`-134 (asserts a `ParseError::Lex`'s message has no leading `": "`) — KEEP it, but replace `parse_error_message(&err)` with `err.to_string()`:
```rust
        let msg = err.to_string();
```

- [ ] **Step 4: Drop the huck-syntax root re-export.**

In `crates/huck-syntax/src/lib.rs`, delete the line:
```rust
pub use errors::{lex_error_message, parse_error_message};
```

- [ ] **Step 5: Drop the huck-engine re-export names.**

In `crates/huck-engine/src/lib.rs:66`, change:
```rust
pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};
```
to:
```rust
pub use huck_syntax::escape_double_quote_value;
```

- [ ] **Step 6: Migrate the huck-engine callers to Display.**

- `shell.rs:459`: `format_args!("syntax error: {}", crate::parse_error_message(&e))` → `format_args!("syntax error: {e}")`.
- `builtins.rs:7322` and `:7454`: these are `crate::parse_error_message(&crate::command::ParseError::Lex( … ))` — replace with `crate::command::ParseError::Lex( … ).to_string()` (keep the exact `ParseError::Lex(...)` argument unchanged; just wrap in `.to_string()` instead of the free fn).
- `builtins.rs:7482`: `crate::parse_error_message(&e)` → `e.to_string()`.

- [ ] **Step 7: Confirm no references to the removed functions remain.**

```bash
grep -rn 'lex_error_message\b\|parse_error_message\b' crates/*/src | grep -v '_impl'
```
Expected: no output (only the `*_impl` names remain, which this grep excludes).

- [ ] **Step 8: Build all crates + run the syntax and engine lib tests.**

```bash
cargo build -p huck-syntax -p huck-engine 2>&1 | tail -3
cargo build -p huck-cli 2>&1 | tail -3
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
```
Expected: all build `Finished`; both test suites `ok` (huck-syntax loses 2 tests → ~439).

- [ ] **Step 9: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-syntax/src/errors.rs crates/huck-syntax/src/lib.rs crates/huck-engine/src/lib.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/builtins.rs
git commit -m "$(printf 'refactor(#102): remove redundant lex_error_message/parse_error_message\n\nThe two huck-syntax free functions were thin wrappers over the same *_impl\nfunctions the Display impls already call (docs called them historical wrappers).\nDelete them, drop both root re-exports, and migrate the huck-engine callers to\nDisplay (.to_string() / format!). BraceError already had Display-only, so all\nthree error types now share the consistent surface.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Final verification (after all tasks)

- `cargo build -p huck-syntax -p huck-engine && cargo build -p huck-cli && cargo build -p huck` — all `Finished`, 0 warnings.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `--doc` — `ok`.
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` and `--doc` — `ok`.
- `cargo build --examples -p huck-syntax -p huck-engine` and run `list_assignments` + `engine_sandbox_diff` — exit 0.
- `cd site && npm run build` — succeeds.
- `cargo fmt --all --check` — clean.
- `grep -rn '\bRedirect\b\|with_version\|restricted(true)\|lex_error_message\b\|parse_error_message\b' crates/ site/app/ | grep -v '_impl'` — no output.
