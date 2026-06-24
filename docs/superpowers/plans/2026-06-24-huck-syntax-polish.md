# v211 huck-syntax API polish — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish the `huck-syntax` public API to idiomatic Rust standards for external publication. Adds `Display` + `std::error::Error` on the 3 error enums; flips two free-function renderers to by-ref; adds a peek-first `try_split_assignment_ref`; marks 8 enums `#[non_exhaustive]`; adds curated root re-exports + a module-level doc with an embedding example.

**Architecture:** Pure API polish; no runtime behavior change. Trait impls + signature touch-ups + visibility-style attributes + doc comments. Internal consumers `huck-engine` and `huck-cli` are updated in lockstep with each breaking signature change.

**Tech Stack:** Rust 2021/2024 (the crate uses 2024 edition), no new deps.

**Branch:** `v211-huck-syntax-polish`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-24-huck-syntax-polish-design.md`.

**Key context — current state** (verified pre-plan):
- `crates/huck-syntax/src/lib.rs:1-13` — module declarations + 2 root re-exports today.
- `crates/huck-syntax/src/errors.rs` — free functions `lex_error_message(LexError) -> String` and `parse_error_message(ParseError) -> String` (take by value, not by ref).
- `crates/huck-syntax/src/lexer.rs:281` — `Token` enum (4 variants today).
- `crates/huck-syntax/src/lexer.rs:233` — `WordPart` enum (10 variants).
- `crates/huck-syntax/src/lexer.rs:147` — `TransformOp` enum (10 variants after v210).
- `crates/huck-syntax/src/lexer.rs:166` — `ParamModifier` enum.
- `crates/huck-syntax/src/lexer.rs` — `LexError` enum (search `pub enum LexError`).
- `crates/huck-syntax/src/command.rs:594` — `Command` enum (~13 variants).
- `crates/huck-syntax/src/command.rs:716` — `ParseError` enum.
- `crates/huck-syntax/src/brace_expand.rs` — `BraceError` enum + `expand` fn.
- `crates/huck-syntax/src/command.rs:96` — `try_split_assignment(Word) -> Result<Assignment, Word>`.
- `crates/huck-syntax/examples/{tokenize_dump,list_assignments}.rs` — current consumers that hit the API rough edges.
- All in-workspace call sites of `lex_error_message` / `parse_error_message`: grep before changing.

---

## File structure

**Modify:**
- `crates/huck-syntax/src/errors.rs` — change signatures to `&LexError` / `&ParseError`; add `Display` impls on the 3 error types (in their respective modules).
- `crates/huck-syntax/src/lexer.rs` — `Display` for `LexError`; `#[non_exhaustive]` on 5 enums.
- `crates/huck-syntax/src/command.rs` — `Display` for `ParseError`; `#[non_exhaustive]` on 2 enums; add `try_split_assignment_ref` helper.
- `crates/huck-syntax/src/brace_expand.rs` — `Display` for `BraceError`; `#[non_exhaustive]` on it.
- `crates/huck-syntax/src/lib.rs` — curated root re-exports + module-level doc with embedding example (doctest-checked).
- `crates/huck-engine/src/**` — call-site updates for the new signatures and `#[non_exhaustive]` matches (grep + fix).
- `crates/huck-cli/src/**` — same.
- `crates/huck-syntax/examples/tokenize_dump.rs` — drop the `format!("{e:?}")` workaround.
- `crates/huck-syntax/examples/list_assignments.rs` — drop the `try_split_assignment(w.clone())` clone using the new peek variant.
- `docs/architecture.md` — one sentence noting the polish (optional, see Task 5).

No new files.

---

## Task 1: `Display` + `std::error::Error` on the 3 error types + by-ref signatures

**Files:**
- Modify: `crates/huck-syntax/src/errors.rs` (signatures).
- Modify: `crates/huck-syntax/src/lexer.rs` (`Display` + `Error` for `LexError`).
- Modify: `crates/huck-syntax/src/command.rs` (`Display` + `Error` for `ParseError`).
- Modify: `crates/huck-syntax/src/brace_expand.rs` (`Display` + `Error` for `BraceError`).
- Modify: `crates/huck-engine/src/**` and `crates/huck-cli/src/**` — call-site adapters.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v211-huck-syntax-polish
```

- [ ] **Step 2: Locate every call site of the free renderers**

```bash
grep -rn 'lex_error_message\|parse_error_message' crates/ tests/ | grep -v target
```

Record every hit. Each one will need a small change:
- Before: `lex_error_message(e)` where `e: LexError`.
- After:  `lex_error_message(&e)` — OR using `Display` directly: `format!("{}", e)`.

If the call site already moved `e` into the renderer and never used it after, just add the `&`. If `e` is captured by closure and the closure was the consumer, no change.

- [ ] **Step 3: Add `Display` for `LexError`**

In `crates/huck-syntax/src/lexer.rs`, find the `pub enum LexError` definition. Add directly below it:

```rust
impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegate to the existing free-function body. Once Display
        // is in place, lex_error_message is a thin wrapper.
        f.write_str(&crate::errors::lex_error_message_impl(self))
    }
}

impl std::error::Error for LexError {}
```

The `lex_error_message_impl` is a private helper introduced in Step 5 below — it owns the rendering logic so both the trait impl AND the existing free function delegate to it.

- [ ] **Step 4: Add `Display` for `ParseError`**

In `crates/huck-syntax/src/command.rs`, find the `pub enum ParseError` definition. Add directly below:

```rust
impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::errors::parse_error_message_impl(self))
    }
}

impl std::error::Error for ParseError {}
```

- [ ] **Step 5: Refactor `errors.rs`**

Open `crates/huck-syntax/src/errors.rs`. Today it has two free functions taking owned errors. Change to:

```rust
//! Error message rendering for huck-syntax's lex and parse stages.
//!
//! The canonical rendering lives in [`lex_error_message_impl`] /
//! [`parse_error_message_impl`] (crate-private). The error types
//! `LexError` / `ParseError` delegate their `Display` impls here.
//! The historical public free functions `lex_error_message` /
//! `parse_error_message` are kept as thin wrappers around the impls
//! but now take `&LexError` / `&ParseError` so callers can render
//! without moving.

use crate::command::ParseError;
use crate::lexer::LexError;

/// Render a `LexError` to a human-readable message.
///
/// Equivalent to `format!("{}", error)` via the `Display` impl;
/// kept as a free function for ergonomic call-sites that don't want
/// to import `std::fmt::Display`.
pub fn lex_error_message(error: &LexError) -> String {
    lex_error_message_impl(error)
}

/// Render a `ParseError` to a human-readable message.
pub fn parse_error_message(error: &ParseError) -> String {
    parse_error_message_impl(error)
}

pub(crate) fn lex_error_message_impl(error: &LexError) -> String {
    // [body migrated from the old lex_error_message function]
    // ... existing match arms, taking &LexError now ...
}

pub(crate) fn parse_error_message_impl(error: &ParseError) -> String {
    // [body migrated from the old parse_error_message function]
}
```

Migration of the bodies is mechanical: replace `LexError::Foo(x)` (consuming match) with `LexError::Foo(x)` (still works on `&LexError` since the variant data is borrowed). For any owned-string return value, no change. For any match that did `let foo = e.into_thing()`, switch to a borrow.

Read the existing function bodies first to confirm there's nothing exotic.

- [ ] **Step 6: Add `Display` + `Error` for `BraceError`**

In `crates/huck-syntax/src/brace_expand.rs`, find `pub enum BraceError`. Add:

```rust
impl std::fmt::Display for BraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BraceError::Variant1 => f.write_str("variant 1 message"),
            // ... real variants per the enum ...
        }
    }
}

impl std::error::Error for BraceError {}
```

Verify the enum's actual variants and write the rendering inline (no separate `_impl` helper needed since BraceError has no existing free renderer).

- [ ] **Step 7: Update internal call sites**

For each call site recorded in Step 2:
- Change `lex_error_message(e)` → `lex_error_message(&e)`.
- OR change to `format!("{e}")` / `e.to_string()` if more natural.

Both forms are equivalent and produce identical strings. The Display path is more idiomatic; the free-function path is sometimes more readable.

- [ ] **Step 8: Build + test**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: full suite green; clippy clean.

If clippy complains about `Box<dyn Error>` lifetime issues or `Error::source()`, leave it — `std::error::Error` has a default `source()` returning `None`, which is correct for these terminal errors.

- [ ] **Step 9: Add unit tests**

In `crates/huck-syntax/src/errors.rs` (or a new `mod tests` at the bottom), add:

```rust
#[cfg(test)]
mod tests {
    use crate::lexer::LexError;
    use crate::command::ParseError;
    use crate::brace_expand::BraceError;

    #[test]
    fn lex_error_display_equals_free_function() {
        // Pick a representative variant. UnterminatedHeredoc is a known one.
        let err = LexError::UnterminatedHeredoc;
        let via_display = format!("{err}");
        let via_free = super::lex_error_message(&err);
        assert_eq!(via_display, via_free);
    }

    #[test]
    fn parse_error_display_equals_free_function() {
        let err = ParseError::MissingCommand;
        let via_display = format!("{err}");
        let via_free = super::parse_error_message(&err);
        assert_eq!(via_display, via_free);
    }

    #[test]
    fn brace_error_implements_display_and_error() {
        // Use a real BraceError variant — read brace_expand.rs to confirm.
        // The test asserts the trait bounds compile; the actual variant
        // is whatever's available.
        fn assert_traits<E: std::fmt::Display + std::error::Error>(_e: &E) {}
        // construction left as `todo!()` because the variant may need a
        // payload — adapt to the actual enum during implementation.
    }
}
```

The third test is shape-only; adapt to the actual `BraceError` variants. The point is to prove `Display` + `Error` impls compile.

- [ ] **Step 10: Commit**

```bash
git add crates/
git commit -m "$(cat <<'EOF'
v211 task 1: Display + Error on error types; by-ref renderers

LexError / ParseError / BraceError gain Display + std::error::Error
impls. Rendering logic stays canonical in errors.rs (lex_error_message_impl
/ parse_error_message_impl); the public free functions
lex_error_message / parse_error_message now take &Error and remain as
thin wrappers around the impls. Internal callers in huck-engine /
huck-cli updated to pass references. Three unit tests pin Display ==
free-function equivalence so user-facing error messages stay
byte-identical.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `try_split_assignment_ref` peek-first variant

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` — add the `_ref` peek helper.
- Modify: `crates/huck-syntax/examples/list_assignments.rs` — switch to the peek variant.

- [ ] **Step 1: Inspect `try_split_assignment`**

```bash
grep -n "pub fn try_split_assignment" crates/huck-syntax/src/command.rs
```

Read the body. It currently moves `Word` and returns `Result<Assignment, Word>` (Err returns the input on failure). The peek variant just needs to decide YES/NO without consuming.

- [ ] **Step 2: Factor or duplicate the recognizer**

Two implementation options:

**(A) Duplicate the recognition logic** (simplest): write a `recognize_assignment_shape(&Word) -> bool` private helper that mirrors the if/else in `try_split_assignment`. The peek variant is:

```rust
pub fn try_split_assignment_ref(word: &Word) -> bool {
    recognize_assignment_shape(word)
}
```

**(B) Refactor to share** (cleaner): split the recognition out of `try_split_assignment`. Both the peek and consuming forms call the recognizer; the consuming form additionally does the field extraction.

Prefer (B) but only if the recognition is non-trivial. Read the existing body and decide.

Actually the spec says the peek variant returns `Option<Assignment>`, not `bool`. Re-read the spec — peek-first should return the assignment so the caller doesn't need to re-extract. To do that without consuming the input Word, we have to either CLONE the Word internally and call the consuming form, OR write a parallel extraction that borrows.

Simpler approach: clone internally:

```rust
/// Peek variant of [`try_split_assignment`] that does not consume the
/// input. Returns `Some(Assignment)` (cloning the relevant parts) if
/// `word` has assignment shape, else `None`.
pub fn try_split_assignment_ref(word: &Word) -> Option<Assignment> {
    try_split_assignment(word.clone()).ok()
}
```

This works because Word is `Clone`. Performance cost is one clone (which is unavoidable if the result must own its parts). The win is API ergonomics — callers don't have to clone visibly.

- [ ] **Step 3: Add a unit test**

```rust
#[test]
fn try_split_assignment_ref_parity_with_consuming_form() {
    use crate::lexer::{Word, WordPart};
    let scalar = Word(vec![WordPart::Literal { text: "name=hello".into(), quoted: false }]);

    // Peek then consume — both should agree on outcome.
    let peek = try_split_assignment_ref(&scalar);
    let consume = try_split_assignment(scalar.clone()).ok();
    assert_eq!(peek, consume);
    assert!(peek.is_some());

    // Negative: word that's not an assignment.
    let plain = Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }]);
    assert!(try_split_assignment_ref(&plain).is_none());
}
```

- [ ] **Step 4: Update `examples/list_assignments.rs`**

Find the `try_parse_decl_arg_assignment` helper at the end of the file. It currently does:

```rust
fn try_parse_decl_arg_assignment(w: &Word) -> Option<Assignment> {
    huck_syntax::command::try_split_assignment(w.clone()).ok()
}
```

Replace with the new ergonomic call:

```rust
fn try_parse_decl_arg_assignment(w: &Word) -> Option<Assignment> {
    huck_syntax::command::try_split_assignment_ref(w)
}
```

- [ ] **Step 5: Build + test**

```bash
cargo build --workspace -q
cargo build --examples -p huck-syntax
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/
git commit -m "$(cat <<'EOF'
v211 task 2: try_split_assignment_ref peek variant

Adds pub fn try_split_assignment_ref(&Word) -> Option<Assignment>
as a peek-first variant of the existing consuming form. One unit test
pins parity. Updates examples/list_assignments.rs to use the new
helper (drops the visible .clone() on every candidate word).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `#[non_exhaustive]` on the 8 enums + internal call-site fixes

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — 5 enums.
- Modify: `crates/huck-syntax/src/command.rs` — 2 enums.
- Modify: `crates/huck-syntax/src/brace_expand.rs` — 1 enum.
- Modify: `crates/huck-engine/src/**` and `crates/huck-cli/src/**` — call-site fixes for newly-required catchalls.

- [ ] **Step 1: Add `#[non_exhaustive]` to lexer enums**

In `crates/huck-syntax/src/lexer.rs`, add `#[non_exhaustive]` to:
- `pub enum Token` (line ~281)
- `pub enum WordPart` (line ~233)
- `pub enum ParamModifier` (line ~166)
- `pub enum TransformOp` (line ~147)
- `pub enum LexError` (search `pub enum LexError`)

Example:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum Token {
    // existing variants ...
}
```

The attribute goes IMMEDIATELY ABOVE the `pub enum` line.

- [ ] **Step 2: Add `#[non_exhaustive]` to command enums**

In `crates/huck-syntax/src/command.rs`:
- `pub enum Command` (line ~594)
- `pub enum ParseError` (line ~716)

- [ ] **Step 3: Add `#[non_exhaustive]` to BraceError**

In `crates/huck-syntax/src/brace_expand.rs`:
- `pub enum BraceError`

- [ ] **Step 4: Build the workspace; find every match site that needs `_ =>`**

```bash
cargo build --workspace 2>&1 | grep -E 'non-exhaustive patterns|error\[E0004\]' -A 5
```

Each error names a file and line. For each, open the file and the match block, and add a catchall arm appropriate to the surrounding code:

- For result-returning code: `_ => return Err(...)` or similar.
- For string-rendering: `_ => "<unknown>".to_string()` (matches conservative defaults).
- For dispatch tables (e.g. `expand.rs:1463` trace-source): pick a sensible fallback that doesn't crash.

Do NOT use `unreachable!()` — that's the point of `#[non_exhaustive]`. Use a real fallback that produces a sensible value if a future variant lands without updating the call site.

Specifically for `expand.rs:1463` trace-source table: extend it with a catchall like:

```rust
_ => '?',  // forward-compatible fallback for any future TransformOp
```

The exact letter or fallback is up to the implementer; the rule is "don't crash; produce something sensible".

- [ ] **Step 5: Repeat until cargo build is clean**

```bash
cargo build --workspace 2>&1 | tail -20
```

Iterate Step 4 until clean.

- [ ] **Step 6: Run tests + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean. (Existing tests should still pass — none of them exercise a "future variant" path, so the `_ => …` catchalls are dormant.)

- [ ] **Step 7: Commit**

```bash
git add crates/
git commit -m "$(cat <<'EOF'
v211 task 3: #[non_exhaustive] on 8 AST and error enums

Marks Token, WordPart, ParamModifier, TransformOp, LexError, Command,
ParseError, BraceError with #[non_exhaustive]. External downstream
matches now MUST use _ =>, so new variants (e.g. v212+ shell features)
don't trigger a SemVer breaking change for external consumers. Internal
call sites in huck-engine / huck-cli updated with forward-compatible
catchall arms — no unreachable!() (that defeats the purpose of
#[non_exhaustive]); each fallback produces a sensible value.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Curated root re-exports + module-level doc

**Files:**
- Modify: `crates/huck-syntax/src/lib.rs`.
- Modify: `crates/huck-syntax/examples/tokenize_dump.rs` — switch to root paths if they're cleaner.

- [ ] **Step 1: Read the current `lib.rs`**

```bash
cat crates/huck-syntax/src/lib.rs
```

Today's content is short — 6 `pub mod` lines + 2 `pub use` re-exports. Replace with a module-level doc comment + curated re-export block.

- [ ] **Step 2: Replace `lib.rs` content**

```rust
//! # huck-syntax
//!
//! Shell-free frontend for the [huck](https://github.com/jdstanhope/huck)
//! POSIX-ish shell: lexer, command-AST parser, brace expansion, and
//! source generator. Re-usable as a standalone library for shell
//! parsing, linting, and tooling.
//!
//! ## Pipeline
//!
//! ```text
//! source bytes  →  tokenize  →  parse        →  walk / regenerate
//! &str             Vec<Token>   Option<Sequence>   Command tree
//! ```
//!
//! ## Quick example
//!
//! ```rust
//! use huck_syntax::{parse, tokenize, Command, Sequence};
//!
//! let src = "a=1; echo \"$a\"";
//! let tokens = tokenize(src).expect("lex");
//! let seq: Option<Sequence> = parse(tokens).expect("parse");
//! let Some(seq) = seq else { return; };
//! // seq.first is a Command::Simple here.
//! assert!(matches!(seq.first, Command::Simple(_)));
//! ```
//!
//! For richer examples — token dumping, AST walking, assignment
//! extraction — see the `examples/` directory:
//!
//! - `cargo run --example tokenize_dump -p huck-syntax`
//! - `cargo run --example list_assignments -p huck-syntax`
//!
//! ## Crate layout
//!
//! - [`lexer`] — bytes → tokens + `Word` AST.
//! - [`command`] — tokens → command AST (`Sequence` / `Command`).
//! - [`generate`] — AST → source bytes (canonical round-trip).
//! - [`brace_expand`] — standalone brace expansion (`a{1,2}b` → words).
//! - [`errors`] — human-readable error message rendering (the
//!   `Display` impls on `LexError` / `ParseError` / `BraceError` are
//!   the canonical surface; the free functions kept here are
//!   convenience wrappers).
//!
//! ## Stability
//!
//! The AST enums (`Token`, `WordPart`, `ParamModifier`, `TransformOp`,
//! `Command`, `ParseError`, `LexError`, `BraceError`) are marked
//! `#[non_exhaustive]`. Downstream consumers MUST use `_ =>` arms
//! when matching, so new variants in future huck releases are not
//! SemVer-breaking.
//!
//! This crate has NO dependency on huck's runtime; it is buildable
//! and consumable on its own. The dependency direction is enforced
//! by Cargo (no cycle).

pub mod brace_expand;
pub mod command;
pub mod errors;
pub mod generate;
pub mod lexer;
pub mod util;

// --- curated root re-exports ----------------------------------------
// External consumers can `use huck_syntax::{Word, parse}` instead of
// hunting through six modules. Module paths remain valid for the few
// types not re-exported here.

pub use brace_expand::{expand as brace_expand, BraceError};
pub use command::{
    parse, parse_with_lines, try_split_assignment, try_split_assignment_ref,
    AssignTarget, Assignment, Command, ExecCommand, ParseError, Pipeline, Sequence,
    SimpleCommand,
};
pub use errors::{lex_error_message, parse_error_message};
pub use generate::{command_to_source, function_to_source};
pub use lexer::{
    tokenize, tokenize_with_opts, LexError, LexerOptions, ParamModifier, SubscriptKind,
    Token, TransformOp, Word, WordPart,
};
pub use util::escape_double_quote_value;
```

If the doctest at the top doesn't compile because of slight type mismatches, adapt to the real signatures. The point is a real, compiler-checked example at the docs.rs landing page.

- [ ] **Step 3: Update examples to use root paths**

In `crates/huck-syntax/examples/tokenize_dump.rs`, simplify the imports:

```rust
use huck_syntax::{tokenize_with_opts, LexerOptions, Token, Word, WordPart};
```

(Was `use huck_syntax::lexer::{...}`; the new shorter path works because of the root re-exports.)

In `crates/huck-syntax/examples/list_assignments.rs`, do the same:

```rust
use huck_syntax::{
    parse, tokenize_with_opts, AssignTarget, Assignment, Command, ExecCommand, IfClause,
    LexerOptions, Sequence, SimpleCommand, WhileClause, Word, WordPart,
};
```

(`IfClause` and `WhileClause` weren't re-exported in Step 2's list — either ADD them to the re-exports, or import them from `command` directly. Decide based on whether they're "common-enough"; if not, leave them at the module path.)

- [ ] **Step 4: Build + run doctest + examples**

```bash
cargo build --workspace -q
cargo test --workspace --doc --quiet
cargo build --examples -p huck-syntax
echo 'a=1' | cargo run -q --example tokenize_dump -p huck-syntax
echo 'a=1' | cargo run -q --example list_assignments -p huck-syntax
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. Doctest in lib.rs passes. Examples still work.

- [ ] **Step 5: Verify cargo doc**

```bash
cargo doc --no-deps -p huck-syntax 2>&1 | grep -E 'warning|error' | head -10
```

Expected: no warnings. Open `target/doc/huck_syntax/index.html` (optional manual check) and confirm the landing page reads well.

- [ ] **Step 6: Commit**

```bash
git add crates/
git commit -m "$(cat <<'EOF'
v211 task 4: curated root re-exports + module-level doc

lib.rs grows a module-level doc with a pipeline diagram, a
compiler-checked Quick example, a pointer to the examples/ directory,
a crate layout summary, and a Stability note about #[non_exhaustive].
Adds curated `pub use` re-exports of the common types so external
users can `use huck_syntax::{Word, Sequence, parse}` instead of
navigating 6 modules.

Examples updated to use the root paths.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Final sweep — verify, doc check, stop before merge

**Files:**
- Optional: `docs/architecture.md` — one sentence pointer.

- [ ] **Step 1: Final full-suite + harness sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet
cargo doc --no-deps -p huck-syntax 2>&1 | grep -E 'warning|error' | head -10

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"

# Examples smoke:
echo 'a=1; b=2' | cargo run -q --example tokenize_dump -p huck-syntax
echo 'a=1; b=2' | cargo run -q --example list_assignments -p huck-syntax
```

Expected: all green; release build clean; cargo doc clean; harnesses pass; CLI smoke prints `hello` + `exit=0`; examples produce sensible output.

- [ ] **Step 2: Add architecture.md pointer**

Open `docs/architecture.md`. Find an appropriate place near the crate-layout discussion. Add a sentence:

```markdown
- **huck-syntax** is the workspace's Shell-free frontend (`crates/huck-syntax/`) — lexer, parser, command AST, generator. As of v211 it ships polished public-API ergonomics (`Display` + `std::error::Error` on the error types, `#[non_exhaustive]` on the AST enums, curated root re-exports + module-level doc with a runnable Quick example) and is publication-ready as a standalone crate.
```

If the architecture doc has a specific "API surface" or "publication" section, place there. Otherwise pick the closest "crate layout" section. Keep it to one sentence.

- [ ] **Step 3: Commit**

```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v211 task 5: architecture.md note on huck-syntax publication readiness

One-sentence pointer noting v211's polish work.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Stop — do NOT merge**

The final whole-branch code review is the controller's call. Stop after this commit.

---

## Self-review

**Spec coverage:**
- Display + Error on 3 error types: Task 1.
- By-ref signatures on the 2 free renderers: Task 1.
- try_split_assignment_ref peek variant: Task 2.
- #[non_exhaustive] on 8 enums + call-site fixes: Task 3.
- Curated root re-exports + module-level doc with embedding example: Task 4.
- Examples updated for new ergonomics: Tasks 2 and 4.
- Architecture.md pointer: Task 5.
- Full sweep + smoke: Task 5.

**Placeholder scan:**
- BraceError variant placeholder in Task 1 Step 6 — implementer reads the real enum and writes real match arms.
- "[body migrated from the old lex_error_message function]" in Task 1 Step 5 — implementer reads the current body and converts to `&self` borrows. Concrete enough.
- Task 3 Step 4 catchall guidance — implementer picks sensible fallback per call site.

**Type consistency:**
- `&LexError` / `&ParseError` / `&BraceError` in renderers — consistent across Tasks 1.
- `try_split_assignment_ref(&Word) -> Option<Assignment>` — consistent in Tasks 2 (definition + caller + test).
- `#[non_exhaustive]` placement (directly above `pub enum`) — same in Task 3 for all 8.

**5 tasks. ~50 LOC production (Display impls + new helper + re-exports) + ~30 LOC tests + ~30 LOC doc text. Smaller than v210; this is API polish, not a feature.**
