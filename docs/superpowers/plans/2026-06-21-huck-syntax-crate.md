# v202: Extract `huck-syntax` crate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move huck's Shell-free frontend (`lexer`, `command` AST+parser, `brace_expand`, `generate`) into a new `huck-syntax` workspace crate, with the `huck` crate depending on it one-directionally and re-exporting its modules so the ~12 runtime files compile unchanged.

**Architecture:** Convert the single `huck` package into a 2-member Cargo workspace (`.` + `crates/huck-syntax`). `git mv` the four frontend files; relocate three pure helpers (`lex_error_message`, `parse_error_message`, `escape_double_quote_value`); add crate-root re-exports in `huck/src/lib.rs`; widen `pub(crate)`→`pub` exactly where the compiler reports `E0603`. Pure code-move + visibility refactor — byte-identical runtime behavior.

**Tech Stack:** Rust (edition 2024), Cargo workspaces.

**Spec:** `docs/superpowers/specs/2026-06-21-huck-syntax-crate-design.md`

**Branch:** `v202-huck-syntax-crate`

**Critical context for the implementer:**
- This is a REFACTOR. The success criterion is **byte-identical behavior**: the existing test suite + bash-diff harnesses are the proof. Do NOT change any logic. Do NOT rename anything except visibility (`pub(crate)`→`pub`).
- A code-move refactor does NOT compile until the move is complete. Tasks 1 and 3 leave a green build; **Task 2 is one atomic "make it compile again" task** with an iterative compile-fix loop — that's expected for a move.
- **Baseline test count** (capture BEFORE starting, the equal-count gate): run
  `cargo test 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print s}'`
  and record the number `BASELINE`. The post-move count MUST equal it.
- The four files to move and their current sizes: `src/lexer.rs` (~8262 L), `src/command.rs` (~6049 L), `src/brace_expand.rs` (~371 L), `src/generate.rs` (~1000 L). Each carries its own `#[cfg(test)] mod tests` (verified to reference only each other, never `Shell`).
- Helpers to relocate:
  - `lex_error_message` (`src/shell.rs:798-837`, `pub(crate)`, formats `LexError`, calls `parse_error_message`).
  - `parse_error_message` (`src/shell.rs:747-791`, `pub(crate)`, formats `ParseError`).
  - `escape_double_quote_value` (`src/builtins.rs:772-784`, `pub(crate)`, pure string escaper; callers at `builtins.rs:861,867,877,878` + `generate.rs:482`).
- `generate.rs` imports `crate::command::{…}` + `crate::lexer::{…}` (all moving together) and calls `crate::builtins::escape_double_quote_value` (→ becomes `crate::escape_double_quote_value` after relocation).
- `command.rs` calls `crate::shell::lex_error_message` at lines 1041, 1458, 2313 (→ becomes `crate::lex_error_message` after relocation).
- `huck/src/lib.rs` currently declares `pub mod lexer; pub mod command; pub mod brace_expand; pub mod generate;` (lines 13,15,22,27) among others.

---

## Task 1: Scaffold the workspace + empty `huck-syntax` crate

**Files:**
- Modify: `Cargo.toml` (root) — add `[workspace]` + the path dependency.
- Create: `crates/huck-syntax/Cargo.toml`, `crates/huck-syntax/src/lib.rs`.

- [ ] **Step 1: Record the baseline test count.**

Run: `cargo test 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print s}'`
Write the number down as `BASELINE`. (It is used in Task 3's gate.)

- [ ] **Step 2: Create `crates/huck-syntax/Cargo.toml`:**

```toml
[package]
name = "huck-syntax"
version = "0.1.0"
edition = "2024"
description = "huck's Shell-free frontend: lexer, parser, command AST, and source generator"
license = "MIT"

[dependencies]
```
(No dependencies — pure `std`. No dependency on `huck`; this is the enforced leaf.)

- [ ] **Step 3: Create a placeholder `crates/huck-syntax/src/lib.rs`:**

```rust
//! `huck-syntax` — huck's Shell-free frontend.
//!
//! Contains the lexer, the command AST + parser, brace expansion, and the
//! AST->source generator. This crate MUST NOT depend on the `huck` runtime
//! crate (the dependency direction is enforced by Cargo: a cycle won't compile).
```

- [ ] **Step 4: Add the workspace + path dep to the root `Cargo.toml`.** Insert a `[workspace]` table immediately after the `[package]` table's fields (before `[dependencies]`), and add the path dep inside `[dependencies]`:

```toml
[workspace]
members = [".", "crates/huck-syntax"]
```
and in `[dependencies]` add:
```toml
huck-syntax = { path = "crates/huck-syntax" }
```

- [ ] **Step 5: Confirm both crates build (frontend not moved yet, so `huck` still has its own `lexer`/`command`/etc.).**

Run: `cargo build 2>&1 | tail -3`
Expected: `Finished` with no errors. (At this point `huck` does not yet USE `huck-syntax`; the path dep is just declared. An "unused crate dependency" is allowed — it is wired up in Task 2.)

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/huck-syntax/Cargo.toml crates/huck-syntax/src/lib.rs
git commit -m "$(cat <<'EOF'
v202 task 1: scaffold the huck-syntax workspace crate

Add a 2-member Cargo workspace with an empty huck-syntax leaf crate (no deps on
huck). The frontend files move in task 2.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Move the frontend into `huck-syntax` (one atomic compile-fix)

**Files:**
- Move (`git mv`): `src/lexer.rs`, `src/command.rs`, `src/brace_expand.rs`, `src/generate.rs` → `crates/huck-syntax/src/`.
- Modify: `crates/huck-syntax/src/lib.rs` (declare the modules + relocated helpers).
- Modify: `src/shell.rs` (remove the two formatters), `src/builtins.rs` (remove `escape_double_quote_value`), `src/lib.rs` (remove the 4 `mod` lines, add re-exports).
- Modify: `crates/huck-syntax/src/command.rs` (its `crate::shell::lex_error_message` → `crate::lex_error_message`), `crates/huck-syntax/src/generate.rs` (its `crate::builtins::escape_double_quote_value` → `crate::escape_double_quote_value`).

NOTE: the workspace will NOT compile mid-task. Work through all sub-steps, then run the compile-fix loop in Step 10.

- [ ] **Step 1: `git mv` the four files.**

```bash
git mv src/lexer.rs crates/huck-syntax/src/lexer.rs
git mv src/command.rs crates/huck-syntax/src/command.rs
git mv src/brace_expand.rs crates/huck-syntax/src/brace_expand.rs
git mv src/generate.rs crates/huck-syntax/src/generate.rs
```

- [ ] **Step 2: Relocate the two error formatters into `huck-syntax`.** Cut `parse_error_message` (`src/shell.rs:747-791`) and `lex_error_message` (`src/shell.rs:798-837`) verbatim from `src/shell.rs`. Paste them into a new file `crates/huck-syntax/src/errors.rs` with their imports, changing `pub(crate)` → `pub`:

```rust
//! Human-readable formatting for the frontend's `LexError` / `ParseError`.
use crate::command::ParseError;
use crate::lexer::LexError;

pub fn parse_error_message(error: ParseError) -> String {
    // ... PASTE the exact body from shell.rs:747-791 (the `match error { ... }`) ...
}

pub fn lex_error_message(error: LexError) -> String {
    // ... PASTE the exact body from shell.rs:798-837 ...
}
```
(Keep the bodies byte-identical. `lex_error_message` calls `parse_error_message` — now both are in this file, so the call resolves locally.)

- [ ] **Step 3: Relocate `escape_double_quote_value` into `huck-syntax`.** Cut it from `src/builtins.rs:772-784` and paste into a new file `crates/huck-syntax/src/util.rs`, changing `pub(crate)` → `pub`:

```rust
//! Small pure helpers shared by the frontend (and re-used by the runtime).

/// Escape `"`, `\`, `$`, `` ` `` with a backslash for emission inside a
/// double-quoted string (used by `generate` for AST->source and by
/// `declare -p` in the runtime).
pub fn escape_double_quote_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}
```

- [ ] **Step 4: Write `crates/huck-syntax/src/lib.rs`** declaring the modules and re-exporting the helpers:

```rust
//! `huck-syntax` — huck's Shell-free frontend.
//!
//! Contains the lexer, the command AST + parser, brace expansion, and the
//! AST->source generator. This crate MUST NOT depend on the `huck` runtime
//! crate (the dependency direction is enforced by Cargo: a cycle won't compile).

pub mod brace_expand;
pub mod command;
pub mod errors;
pub mod generate;
pub mod lexer;
pub mod util;

pub use errors::{lex_error_message, parse_error_message};
pub use util::escape_double_quote_value;
```

- [ ] **Step 5: Fix the relocated-helper call sites INSIDE `huck-syntax`.** In `crates/huck-syntax/src/command.rs`, replace the 3 occurrences of `crate::shell::lex_error_message` (lines ~1041, ~1458, ~2313) with `crate::lex_error_message`:

```bash
sed -i 's/crate::shell::lex_error_message/crate::lex_error_message/g' crates/huck-syntax/src/command.rs
```
In `crates/huck-syntax/src/generate.rs`, replace `crate::builtins::escape_double_quote_value` (line ~482) with `crate::escape_double_quote_value`:
```bash
sed -i 's/crate::builtins::escape_double_quote_value/crate::escape_double_quote_value/g' crates/huck-syntax/src/generate.rs
```

- [ ] **Step 6: Remove the moved `mod` declarations + add re-exports in `huck/src/lib.rs`.** Delete the four lines `pub mod brace_expand;`, `pub mod command;`, `pub mod generate;`, `pub mod lexer;` from `src/lib.rs`. Add, near the top of the `mod` block (e.g. right after the remaining `pub mod` declarations):

```rust
// Frontend modules live in the `huck-syntax` crate; re-export at the crate root
// so existing `crate::lexer::`/`crate::command::`/`crate::generate::` paths and
// the relocated helpers resolve unchanged across the runtime.
pub use huck_syntax::{brace_expand, command, generate, lexer};
pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};
```

- [ ] **Step 7: Fix the formatter call sites in `src/shell.rs`.** `shell.rs` referenced `lex_error_message`/`parse_error_message` as local fns (they were defined there). They are now re-exported at the crate root, so the existing unqualified calls inside `shell.rs` need a path. Find them:
```bash
grep -n "lex_error_message\|parse_error_message" src/shell.rs
```
Each call site that previously called the local fn now uses `crate::lex_error_message(...)` / `crate::parse_error_message(...)`. (If they were already called via `crate::shell::…` or bare, normalize to `crate::…`.)

- [ ] **Step 8: Fix the `escape_double_quote_value` call sites in `src/builtins.rs`** (lines ~861, ~867, ~877, ~878). They previously called the local fn; now use `crate::escape_double_quote_value(...)`:
```bash
grep -n "escape_double_quote_value" src/builtins.rs   # should be only call sites now, definition removed
```
Replace bare `escape_double_quote_value(` calls with `crate::escape_double_quote_value(` (only inside builtins.rs; the definition there was removed in Step 3).

- [ ] **Step 9: Handle `shell.rs`'s `use crate::command::{self, ParseError};` / `use crate::lexer::{self, LexError};`.** These still resolve via the re-export (`crate::command` / `crate::lexer` are now re-exported modules). No change needed unless the compiler reports otherwise in Step 10.

- [ ] **Step 10: The compile-fix loop.** Run `cargo build 2>&1 | tail -40` and resolve errors iteratively. EXPECTED error classes and their fixes:
  - **`E0603: ... is private`** — a moved item the `huck` crate uses is `pub(crate)`; in the moved file (`huck-syntax`), change that item's `pub(crate)` → `pub`. Repeat until none remain. (This is the bulk of the work; the compiler enumerates every one.)
  - **`E0432: unresolved import crate::shell::lex_error_message`** inside `command.rs` — means a Step-5 sed was missed; ensure it's `crate::lex_error_message`.
  - **`unresolved import crate::builtins`** inside `generate.rs` — Step-5 sed missed; ensure `crate::escape_double_quote_value`.
  - **`cannot find ... in crate::lexer` / `crate::command`** inside the runtime — the re-export covers these; if one fails, confirm the item is `pub` in `huck-syntax`.
  - Do NOT change any logic to satisfy the compiler. Only widen visibility or fix a path.

  Run repeatedly until: `cargo build 2>&1 | tail -3` shows `Finished` with no errors.

- [ ] **Step 11: Build the release binary and smoke-test it.**

Run:
```bash
cargo build --release 2>&1 | tail -1
ls -l target/release/huck
./target/release/huck -c 'echo ok'
```
Expected: `Finished`; the binary exists at `target/release/huck` (SAME path as before — confirms the deb script is unaffected); prints `ok`.

- [ ] **Step 12: Commit the move.**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v202 task 2: move lexer/command/brace_expand/generate into huck-syntax

git mv the four Shell-free frontend files into crates/huck-syntax/; relocate the
lex_error_message/parse_error_message formatters (from shell.rs) and
escape_double_quote_value (from builtins.rs) into the crate; re-export the
modules + helpers at the huck crate root so runtime paths resolve unchanged.
pub(crate)->pub widened only where the compiler required (E0603). No logic
changes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Verify equal behavior + boundary + docs

**Files:**
- Modify: `docs/architecture.md` (note the new crate boundary).

- [ ] **Step 1: Full workspace test suite — count must equal BASELINE.**

Run:
```bash
cargo test 2>&1 | grep -E "test result:" | grep -v "0 failed" || echo "ALL GREEN"
cargo test 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print "POST-MOVE total: " s}'
```
Expected: `ALL GREEN`; `POST-MOVE total` equals the `BASELINE` recorded in Task 1 Step 1. A mismatch means a test was lost/skipped — investigate (a moved `mod tests` not compiled, or a file not declared) before proceeding.

- [ ] **Step 2: All bash-diff harnesses green.**

Run: `for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -iE "Fail: [1-9]|[1-9] failed" || echo "ALL HARNESSES GREEN"`
Expected: `ALL HARNESSES GREEN`.

- [ ] **Step 3: Clippy clean across the workspace.**

Run: `cargo clippy --all-targets 2>&1 | grep -cE "^warning|^error" | xargs -I{} echo "clippy: {}"`
Expected: `clippy: 0`. (If new `unused` warnings appear in `huck-syntax` for items only used by `huck` via re-export, that should not happen — re-exported `pub` items are considered used; if one does, it indicates a genuinely-unused item that predated the move — leave it / `#[allow]` only if pre-existing.)

- [ ] **Step 4: Confirm the boundary is enforced.**

Run: `grep -rn "use crate::shell\|crate::shell_state\|crate::executor\|crate::expand\b\|crate::builtins" crates/huck-syntax/src/ || echo "NO RUNTIME REFS (boundary clean)"`
Expected: `NO RUNTIME REFS (boundary clean)`. (`crates/huck-syntax` must reference nothing in `huck`.)

- [ ] **Step 5: Confirm `huck-syntax` has no `huck` dependency.**

Run: `grep -n "huck" crates/huck-syntax/Cargo.toml || echo "no huck dep (leaf crate)"`
Expected: `no huck dep (leaf crate)`.

- [ ] **Step 6: Update `docs/architecture.md`.** Add a short note (in the module-map / crate-structure area) describing the split: the Shell-free frontend (`lexer`, `command` AST+parser, `brace_expand`, `generate`, + the `errors`/`util` helpers) now lives in the `huck-syntax` workspace crate; the `huck` crate depends on it one-directionally and re-exports its modules at the crate root, so `crate::lexer::`/`crate::command::` paths are unchanged. Keep it to a short paragraph; match the doc's existing style.

- [ ] **Step 7: Commit.**

```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v202 task 3: verify equal behavior + document the huck-syntax boundary

Full suite (equal pre/post test count) + all harnesses + clippy green; release
binary builds at the same path; huck-syntax references no runtime code. Note the
crate boundary in architecture.md.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Report-back (Task 3)

Report: STATUS, the commit SHAs, the BASELINE vs POST-MOVE test counts (must be equal), the full-suite + harness + clippy results, the release-binary smoke-test, the boundary-clean check, and the count of `pub(crate)`→`pub` widenings made (for the reviewer).
