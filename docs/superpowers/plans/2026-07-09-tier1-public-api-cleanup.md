# Tier 1 Public-API Surface Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sharpen the public API of huck's two consumable crates by hiding internal/cli-only surface, renaming a mis-signalling method, and surfacing + documenting the huck-syntax pipeline entry points — with no external consumers, so nothing is breaking.

**Architecture:** Four independent tasks. Tasks 1–2 tighten `huck-engine`'s surface (module visibility + one method rename). Tasks 3–4 expand and document `huck-syntax`'s entry points (re-exports + a convenience fn, then two examples + doc fixes). Task 4 depends on Task 3; the engine and syntax halves are independent.

**Tech Stack:** Rust (edition 2024), Cargo workspace (`huck-syntax`, `huck-engine`, `huck-cli`, `huck` binary), Next.js site under `site/`.

**Reference:** Spec at `docs/superpowers/specs/2026-07-09-tier1-public-api-cleanup-design.md` (issue [#99](https://github.com/jdstanhope/huck/issues/99)).

## Global Constraints

- **No external consumers exist** — every change here is non-breaking; do not add back-compat shims or deprecated aliases.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Never run `cargo test --workspace`** (OOM on this box). Test per-crate: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **Format before every commit:** `cargo fmt --all` (CI enforces `cargo fmt --all --check`).
- **`error_emit` stays fully public** (not `pub(crate)`, not `#[doc(hidden)]`) — its rework is Tier 2, out of scope.
- **Do NOT touch `process.exec()` in `executor.rs:5546`** — that is `std::os::unix::process::CommandExt::exec` (the exec syscall), unrelated to `Engine::exec`.

---

### Task 1: huck-engine — hide the internal + cli-only module surface

Demote accidentally-public modules to `pub(crate)`, `#[doc(hidden)]` the cli-only modules, and `#[doc(hidden)]` the `Shell`-cell escape hatch. This is spec sections A, B, C.

**Files:**
- Modify: `crates/huck-engine/src/lib.rs:11-42` (the `pub mod` block)
- Modify: `crates/huck-engine/src/engine.rs` (two method attributes)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: no new symbols. `huck_engine::StdoutSink` / `StderrSink` / `Candidate` / `CandidateKind` remain re-exported at the crate root (unchanged re-export lines); the module *paths* for the demoted/hidden modules change visibility only.

- [ ] **Step 1: Edit the module-visibility block in `lib.rs`.**

In `crates/huck-engine/src/lib.rs`, change exactly these lines (leave every other `mod` line, all `pub use` lines, and `engine`/`error_emit`/`exec_builder` untouched):

Demote to `pub(crate)` (drop `pub`, add `pub(crate)`):
```rust
pub(crate) mod arith;
pub(crate) mod completion_builtins;
pub(crate) mod completion_spec;
pub(crate) mod err_thread_local;
pub(crate) mod executor;
pub(crate) mod expand;
pub(crate) mod glob_match;
pub(crate) mod job_spec;
pub(crate) mod param_expansion;
pub(crate) mod procsub;
pub(crate) mod test_builtin;
```

Add `#[doc(hidden)]` (keep `pub`):
```rust
#[doc(hidden)]
pub mod builtins;
#[doc(hidden)]
pub mod completion;
#[doc(hidden)]
pub mod continuation;
#[doc(hidden)]
pub mod history;
#[doc(hidden)]
pub mod jobs;
#[doc(hidden)]
pub mod prompt;
#[doc(hidden)]
pub mod readline_bind;
#[doc(hidden)]
pub mod shell;
#[doc(hidden)]
pub mod shell_state;
#[doc(hidden)]
pub mod traps;
```

- [ ] **Step 2: `#[doc(hidden)]` the Shell-cell escape hatch in `engine.rs`.**

Add `#[doc(hidden)]` immediately above `pub fn from_shell_cell` (currently `engine.rs:105`) and above `pub fn shell_cell` (currently `engine.rs:204`). Example:
```rust
    /// Wrap a caller-owned (possibly pre-configured) shell cell. The caller keeps
    /// ownership of any process-global concerns (e.g. signal handlers).
    #[doc(hidden)]
    pub fn from_shell_cell(cell: Rc<RefCell<Shell>>) -> Self {
```
and
```rust
    #[doc(hidden)]
    pub fn shell_cell(&self) -> &Rc<RefCell<Shell>> {
```
(Keep the existing doc comments; just insert the attribute line above the `pub fn`.)

- [ ] **Step 3: Build the engine crate.**

Run: `cargo build -p huck-engine 2>&1 | tail -20`
Expected: `Finished` with no errors. (Internal `crate::arith::…` paths are unaffected by `pub`→`pub(crate)`; only external resolution changes.)

- [ ] **Step 4: Build huck-cli and the binary — this proves no over-demotion.**

Run: `cargo build -p huck-cli && cargo build -p huck 2>&1 | tail -15`
Expected: both `Finished` with no errors. `huck-cli` imports `continuation, traps, shell_state, shell, readline_bind, prompt, jobs, history, completion, builtins` — all kept `pub` (only `#[doc(hidden)]`), so they still resolve. If a `pub(crate)` demotion breaks a `huck-cli` path, the compile error names the module — if so, that module was mis-classified: revert it to `#[doc(hidden)] pub mod` instead and note it in the task report.

- [ ] **Step 5: Confirm the demoted modules are gone from rustdoc and the sink types survive.**

Run: `cargo doc -p huck-engine --no-deps 2>&1 | tail -5 && grep -rl 'struct.Engine' target/doc/huck_engine/ | head`
Expected: docs build cleanly. Spot-check that `target/doc/huck_engine/arith/` does **not** exist (demoted) while `target/doc/huck_engine/struct.Engine.html` and `target/doc/huck_engine/enum.StdoutSink.html` (root re-export) **do** exist:
```bash
test ! -d target/doc/huck_engine/arith && echo "arith hidden OK"
test -f target/doc/huck_engine/enum.StdoutSink.html && echo "StdoutSink surfaced OK"
test -f target/doc/huck_engine/struct.Engine.html && echo "Engine surfaced OK"
```
Expected: all three echo lines print.

- [ ] **Step 6: Run the engine lib tests (no behavioral change expected).**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `test result: ok.` with the full count passing (~1779).

- [ ] **Step 7: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/lib.rs crates/huck-engine/src/engine.rs
git commit -m "$(printf 'refactor(#99): hide internal + cli-only huck-engine module surface\n\nDemote 11 accidentally-public modules to pub(crate), #[doc(hidden)] the 10\ncli-only modules (kept pub for huck-cli) and the Rc<RefCell<Shell>> escape\nhatch (from_shell_cell/shell_cell). Engine/ExecBuilder/error_emit and the\ncurated root re-exports are unchanged.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: huck-engine — rename `Engine::exec` → `Engine::prepare`

`e.exec("script")` returns an `ExecBuilder`; next to `run`/`capture` (which execute now) the name mis-signals. Rename to `prepare`. This is spec section D. Purely mechanical; no behavior change.

**Files:**
- Modify: `crates/huck-engine/src/engine.rs` (method def line 126, internal call line 119, module doctest lines 20/28/36, comment line 222, ~50 in-crate test call sites)
- Modify: `crates/huck-engine/src/exec_builder.rs:1,7` (two doc references to `Engine::exec`)
- Modify: `crates/huck-engine/tests/streaming_fd_serial.rs` (4 sites)
- Modify: `crates/huck-engine/tests/tee_inherit.rs` (1 site, line 48)
- Modify: `site/app/library/page.tsx` (2 sites, lines 37 and 43)

**Interfaces:**
- Consumes: nothing from earlier tasks (independent of Task 1).
- Produces: `Engine::prepare(&mut self, src: &str) -> ExecBuilder<'_>` replaces `Engine::exec`. No other engine method changes.

- [ ] **Step 1: Rename the method definition and its internal caller in `engine.rs`.**

Change `engine.rs:126` from:
```rust
    pub fn exec(&mut self, src: &str) -> crate::exec_builder::ExecBuilder<'_> {
```
to:
```rust
    pub fn prepare(&mut self, src: &str) -> crate::exec_builder::ExecBuilder<'_> {
```
Change `engine.rs:119` from `self.exec(src).capture()` to `self.prepare(src).capture()`.
Update the doc comment above `prepare` if it says "Start an advanced execution chain" — keep it, it still reads correctly. Update the comment at `engine.rs:222` `the public `Engine::exec`` → `the public `Engine::prepare``.

- [ ] **Step 2: Rename every remaining `.exec(` call in `engine.rs` (doctests + unit tests).**

These are ALL `Engine::prepare` calls (`e.exec(…)` / `.exec(…)` builder chains + the `//!` doctest lines 20/28/36). None are `std::process::Command::exec`. Apply:
```bash
sed -i 's/\.exec(/.prepare(/g' crates/huck-engine/src/engine.rs
```
Then verify no unintended matches remain:
```bash
grep -n '\.exec(\|fn exec\|Engine::exec' crates/huck-engine/src/engine.rs || echo "clean"
```
Expected: `clean` (the sed already converted `pub fn prepare` in Step 1; `fn exec` should not appear).

- [ ] **Step 3: Update the two doc references in `exec_builder.rs`.**

Change `exec_builder.rs:1` `` per-call builder for [`Engine::exec`]. `` → `` per-call builder for [`Engine::prepare`]. `` and `exec_builder.rs:7` `` [`Engine::exec`]: crate::engine::Engine::exec `` → `` [`Engine::prepare`]: crate::engine::Engine::prepare ``.

- [ ] **Step 4: Update the integration tests.**

```bash
sed -i 's/\.exec(/.prepare(/g' crates/huck-engine/tests/streaming_fd_serial.rs crates/huck-engine/tests/tee_inherit.rs
grep -rn '\.exec(' crates/huck-engine/tests/ || echo "tests clean"
```
Expected: `tests clean`.

- [ ] **Step 5: Update the site's Library page.**

In `site/app/library/page.tsx`, change the two occurrences inside the `engineExecExample` template string (lines ~37 and ~43): `e.exec("for i in 1 2 3; do echo $i; done")` → `e.prepare("for i in 1 2 3; do echo $i; done")` and `e.exec(untrusted_script)` → `e.prepare(untrusted_script)`.

- [ ] **Step 6: Confirm no stray `Engine::exec` remains anywhere.**

Run: `grep -rn 'Engine::exec\|e\.exec(\|self\.exec(' crates/ site/ || echo "fully renamed"`
Expected: `fully renamed`. (The `process.exec()` in `executor.rs:5546` is NOT matched by these patterns — verify it is still present and untouched: `grep -n 'process.exec()' crates/huck-engine/src/executor.rs`.)

- [ ] **Step 7: Build + run engine lib tests + doctests.**

```bash
cargo build -p huck-engine
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -4
cargo test -p huck-engine --jobs 1 --doc -- --test-threads 1 2>&1 | tail -6
```
Expected: build `Finished`; lib tests `ok`; doctests `ok` (the `engine.rs` module doctest now uses `prepare`).

- [ ] **Step 8: Build the isolated integration tests + the site.**

```bash
cargo test -p huck-engine --jobs 1 --test streaming_fd_serial --test tee_inherit -- --test-threads 1 2>&1 | tail -6
cd site && npm run build 2>&1 | tail -4 && cd ..
```
Expected: integration tests pass; site build succeeds (the `/library` route prerenders).

- [ ] **Step 9: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/engine.rs crates/huck-engine/src/exec_builder.rs crates/huck-engine/tests/streaming_fd_serial.rs crates/huck-engine/tests/tee_inherit.rs site/app/library/page.tsx
git commit -m "$(printf 'refactor(#99): rename Engine::exec -> Engine::prepare\n\nexec() returns an ExecBuilder (configure, then run/capture); next to run/capture\nthe name read like a third run-now verb. prepare signals set-up-doesn'"'"'t-run.\nUpdated the method, its internal caller, doctests, in-crate + integration tests,\nexec_builder doc links, and the site Library page.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: huck-syntax — surface the pipeline entry points

Re-export `Lexer` + `parse_sequence` at the crate root and add a `parse(&str)` convenience. This is spec section E.

**Files:**
- Modify: `crates/huck-syntax/src/lib.rs` (re-export lines)
- Modify: `crates/huck-syntax/src/parser.rs` (add `parse` fn + its unit tests)

**Interfaces:**
- Consumes: existing `Lexer::new(input, aliases, opts)`, `parse_sequence(&mut Lexer) -> Result<Option<Sequence>, ParseError>`, `LexerOptions::default()`.
- Produces: `huck_syntax::parse(src: &str) -> Result<Option<Sequence>, ParseError>`; root re-exports `huck_syntax::Lexer` and `huck_syntax::parse_sequence`. Task 4 consumes all three.

- [ ] **Step 1: Write failing unit tests for `parse` in `parser.rs`.**

Add to the `#[cfg(test)] mod tests` block in `crates/huck-syntax/src/parser.rs`:
```rust
    #[test]
    fn parse_convenience_returns_ast_for_command() {
        let seq = super::parse("echo hi").expect("no parse error").expect("non-empty");
        assert!(!seq.background);
    }

    #[test]
    fn parse_convenience_none_for_empty() {
        assert!(super::parse("").expect("no parse error").is_none());
    }

    #[test]
    fn parse_convenience_none_for_comment_only() {
        assert!(super::parse("# just a comment").expect("no parse error").is_none());
    }

    #[test]
    fn parse_convenience_errors_on_incomplete() {
        assert!(super::parse("if").is_err());
    }
```

- [ ] **Step 2: Run the tests to confirm they fail to compile (fn missing).**

Run: `cargo test -p huck-syntax --jobs 1 --lib parse_convenience -- --test-threads 1 2>&1 | tail -8`
Expected: compile error `cannot find function `parse` in module `super``.

- [ ] **Step 3: Add the `parse` function to `parser.rs`.**

Add near the top of `parser.rs` (module scope, after the imports; `Lexer`, `LexerOptions`, `Sequence`, `ParseError` are already in scope in this module — confirm and add `use` only if the compiler complains):
```rust
/// Parse shell source into a command AST using the default lexer configuration
/// (no aliases, default `LexerOptions`). Returns `Ok(None)` for empty or
/// comment-only input. For alias expansion or custom options, build a
/// [`Lexer`](crate::lexer::Lexer) explicitly and call [`parse_sequence`].
pub fn parse(src: &str) -> Result<Option<Sequence>, ParseError> {
    let mut lx = crate::lexer::Lexer::new(src, &Default::default(), crate::lexer::LexerOptions::default());
    parse_sequence(&mut lx)
}
```

- [ ] **Step 4: Run the `parse` tests to confirm they pass.**

Run: `cargo test -p huck-syntax --jobs 1 --lib parse_convenience -- --test-threads 1 2>&1 | tail -8`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 5: Add the root re-exports in `lib.rs`.**

In `crates/huck-syntax/src/lib.rs`:
- Add `Lexer` to the existing `pub use lexer::{ … };` list (insert `Lexer,` — keep alphabetical-ish ordering with the others: `LexError, Lexer, LexerOptions, …`).
- Add a new line after the `pub use command::{…};` block:
```rust
pub use parser::{parse, parse_sequence};
```

- [ ] **Step 6: Confirm the root surface resolves.**

Add a temporary check (or a doctest) — run this one-off to confirm the three names resolve from the crate root:
```bash
cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1 2>&1 | tail -5
```
Then verify with a throwaway compile:
```bash
cat > /tmp/hs_check.rs <<'RS'
fn main() { let _ : fn(&str) -> _ = huck_syntax::parse; let _ = huck_syntax::parse_sequence; }
RS
echo "manual: use huck_syntax::{Lexer, parse, parse_sequence}; must compile in Task 4 examples"
```
Expected: doctests pass. (The definitive proof of the re-exports is Task 4's examples, which `use huck_syntax::{Lexer, parse, ...}`.)

- [ ] **Step 7: Run the full huck-syntax lib + doc tests.**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -4
cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1 2>&1 | tail -4
```
Expected: both `ok` (lib ~441 with the 4 new tests).

- [ ] **Step 8: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-syntax/src/lib.rs crates/huck-syntax/src/parser.rs
git commit -m "$(printf 'feat(#99): surface the huck-syntax pipeline entry points\n\nRe-export Lexer + parse_sequence at the crate root and add a\nparse(src: &str) -> Result<Option<Sequence>, ParseError> convenience over the\ndefault lexer path (Ok(None) for empty/comment-only input, matching\nparse_sequence). Stages 1-2 of the lex->parse->generate pipeline are now\nreachable from the root, like stage 3 already was.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: huck-syntax — write the referenced examples + fix stale docs

Write the two examples `lib.rs` already points at, and fix stale doc cross-references. This is spec sections F + G. Depends on Task 3 (`parse`, `Lexer` re-export).

**Files:**
- Create: `crates/huck-syntax/examples/tokenize_dump.rs`
- Create: `crates/huck-syntax/examples/list_assignments.rs`
- Modify: `crates/huck-syntax/src/lib.rs` (verify the `examples/` refs match file names)
- Modify: `crates/huck-syntax/src/parser.rs` (fix stale `command::parse`/`parse_cursor`/`parse_one_unit` cross-references)

**Interfaces:**
- Consumes: `huck_syntax::{Lexer, LexerOptions, parse, try_split_assignment, Assignment, AssignTarget, Command, Sequence, Token}` (from Task 3 + existing exports).
- Produces: two runnable example binaries; no library symbols.

- [ ] **Step 1: Write `examples/tokenize_dump.rs`.**

```rust
//! Lex a shell string and print the token stream.
//!
//! Run: `cargo run --example tokenize_dump -p huck-syntax -- 'echo hi | wc -l'`
//! (falls back to a built-in sample if no argument is given).

use huck_syntax::lexer::{Lexer, LexerOptions};

fn main() {
    let src = std::env::args().nth(1).unwrap_or_else(|| "echo hello | wc -l".to_string());
    println!("source: {src:?}\n");

    let mut lx = Lexer::new(&src, &Default::default(), LexerOptions::default());
    loop {
        match lx.next() {
            Ok(Some(tok)) => println!("{:>4}:{:<3} {:?}", tok.span.line, tok.span.column, tok.kind),
            Ok(None) => break,
            Err(e) => {
                eprintln!("lex error: {e}");
                std::process::exit(1);
            }
        }
    }
}
```

- [ ] **Step 2: Run `tokenize_dump` and confirm it prints tokens.**

Run: `cargo run --example tokenize_dump -p huck-syntax -- 'echo hi | wc -l' 2>&1 | tail -20`
Expected: builds, prints the `source:` line then one line per token (Word/Operator/etc.), exits 0.

- [ ] **Step 3: Write `examples/list_assignments.rs`.**

```rust
//! Parse a shell string and print every assignment in the first command.
//!
//! Run: `cargo run --example list_assignments -p huck-syntax -- 'a=1 b+=2 echo hi'`
//! (falls back to a built-in sample if no argument is given).

use huck_syntax::command::{Command, SimpleCommand};
use huck_syntax::{parse, Assignment};

fn main() {
    let src = std::env::args().nth(1).unwrap_or_else(|| "a=1 b+=2 echo hi".to_string());
    println!("source: {src:?}\n");

    let seq = match parse(&src) {
        Ok(Some(seq)) => seq,
        Ok(None) => {
            println!("(no command — empty or comment-only input)");
            return;
        }
        Err(e) => {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
    };

    let assigns = collect_assignments(&seq.first);
    if assigns.is_empty() {
        println!("(no assignments found)");
    }
    for a in assigns {
        let op = if a.append { "+=" } else { "=" };
        println!("{}{}{}", a.target.name(), op, "…");
    }
}

/// Pull the assignments off the first simple command (inline `a=1 cmd` prefix
/// assignments and bare `a=1` assignment-only commands).
fn collect_assignments(cmd: &Command) -> Vec<&Assignment> {
    match cmd {
        Command::Simple(SimpleCommand::Exec(exec)) => exec.inline_assignments.iter().collect(),
        Command::Simple(SimpleCommand::Assign(list, _line)) => list.iter().collect(),
        _ => Vec::new(),
    }
}
```

Note: `AssignTarget::name(&self) -> &str` and `Assignment { target, value, append }` are the existing public API (`command.rs`). If a field/method name differs at implementation time, adjust to the real signature rather than inventing one — verify with `grep -n 'pub fn name\|pub struct Assignment\|pub append' crates/huck-syntax/src/command.rs`.

- [ ] **Step 4: Run `list_assignments` and confirm output.**

Run: `cargo run --example list_assignments -p huck-syntax -- 'a=1 b+=2 echo hi' 2>&1 | tail -10`
Expected: builds, prints `a=…` and `b+=…` (the two prefix assignments), exits 0. Also sanity-check the empty case: `cargo run --example list_assignments -p huck-syntax -- '# comment'` prints the `(no command …)` line.

- [ ] **Step 5: Verify `lib.rs` example references are accurate.**

Run: `grep -n 'cargo run --example' crates/huck-syntax/src/lib.rs`
Expected: two lines naming `tokenize_dump` and `list_assignments` (matching the created files). If the invocation text differs (e.g. missing `-p huck-syntax`), fix it to match the exact working command from Steps 2/4. Do not otherwise reword the surrounding docs.

- [ ] **Step 6: Fix stale doc cross-references in `parser.rs`.**

Find them: `grep -n 'command::parse\|parse_cursor\|Mirrors' crates/huck-syntax/src/parser.rs`.
For each comment referencing a nonexistent `command::parse`, `command::parse_cursor`, or `command::parse_one_unit`: rewrite it to reference the real current functions (`parse_sequence` / `parse_one_unit` live in `parser.rs` itself), or delete the cross-reference if it no longer clarifies anything. Do not change any code — comments only. Example fix:
```rust
// BEFORE: /// Mirrors `parse` / `parse_cursor` in `command.rs`.
// AFTER:  /// The atom-path entry point; assembles a `Sequence` from the lexer.
```

- [ ] **Step 7: Confirm no doc comment references a nonexistent parse symbol.**

Run: `grep -rn 'command::parse\|parse_cursor' crates/huck-syntax/src/ || echo "no stale refs"`
Expected: `no stale refs`.

- [ ] **Step 8: Rebuild examples + run the syntax lib tests once more.**

```bash
cargo build --examples -p huck-syntax 2>&1 | tail -5
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -4
```
Expected: examples build; lib tests `ok`.

- [ ] **Step 9: Format and commit.**

```bash
cargo fmt --all
git add crates/huck-syntax/examples/tokenize_dump.rs crates/huck-syntax/examples/list_assignments.rs crates/huck-syntax/src/lib.rs crates/huck-syntax/src/parser.rs
git commit -m "$(printf 'docs(#99): add the referenced huck-syntax examples + fix stale doc refs\n\nWrite examples/tokenize_dump.rs (lex -> token stream) and\nexamples/list_assignments.rs (parse -> AST walk -> assignments), which the\nlib.rs docs already pointed at but did not exist. Fix parser.rs comments that\nreferenced nonexistent command::parse / parse_cursor / parse_one_unit.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Final verification (after all tasks)

- `cargo build -p huck-syntax -p huck-engine && cargo build -p huck-cli && cargo build -p huck` — all `Finished`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` — `ok`.
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — `ok`.
- `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --doc -- --test-threads 1` — `ok`.
- `cargo run --example tokenize_dump -p huck-syntax` and `cargo run --example list_assignments -p huck-syntax` — both exit 0.
- `cd site && npm run build` — succeeds.
- `cargo fmt --all --check` — clean.
