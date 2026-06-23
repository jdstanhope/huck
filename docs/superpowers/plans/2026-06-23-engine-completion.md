# v208 `Engine::complete` Completion API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose huck's existing tab-completion machinery as a public `Engine::complete(line, cursor) -> Completion` method. Light enrichment: each `Candidate` carries a `kind: CandidateKind` tag (Command / Variable / File / Directory / Custom).

**Architecture:** v204-style thin facade — most code already exists. Extend the public `Candidate` struct with a `kind` field; stamp the correct kind at each of 4 internal producers; wrap `completion::dispatch::resolve` in a 15-LOC `Engine::complete`; re-export the new types at the crate root; update the CLI's `HuckHelper` adapter to drop the new field when mapping to `rustyline::Pair`.

**Tech Stack:** Rust 2021, no new crate deps. Pure additive change to existing in-tree types — the `kind` field is a structural addition. No published consumers, so the breaking-change cost is in-tree only.

**Branch:** `v208-engine-completion`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-23-engine-completion-design.md`.

---

## File structure

**Modify:**
- `crates/huck-engine/src/completion.rs` — add `CandidateKind` enum; extend `Candidate` with `kind` field; stamp the correct kind at 6 in-file `Candidate { ... }` literal sites (lines 247, 262, 301, 512, 518 + tests).
- `crates/huck-engine/src/engine.rs` — add `Completion` struct + `Engine::complete` method + ~10 unit tests + doc example update.
- `crates/huck-engine/src/lib.rs` — add 3 re-exports (`Candidate`, `CandidateKind`, `Completion`).
- `crates/huck-cli/src/completion_helper.rs` — drop `kind` in the `Candidate → Pair` mapping (one-line update).
- `docs/architecture.md` — add one paragraph on `Engine::complete`.

No new files; no module restructuring. Smallest iteration in the embedding arc.

---

## Task 1: Extend `Candidate` with `CandidateKind` field

**Files:**
- Modify: `crates/huck-engine/src/completion.rs`

Add the enum and field. All 6 in-file `Candidate { ... }` literal constructors must include the new field. For this task, every site stamps `CandidateKind::Command` as a placeholder — Task 2 refines per-producer. Existing tests that construct `Candidate { display, replacement }` for expected-result assertions also need the new field added. Suite must stay green.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b v208-engine-completion
```

- [ ] **Step 2: Add `CandidateKind` enum + extend `Candidate`**

In `crates/huck-engine/src/completion.rs`, find the existing `Candidate` struct at line ~8 and replace the block:

```rust
/// What kind of completion a `Candidate` represents. Useful for embedders
/// rendering icons or sorting by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// Command position: executable on PATH, shell function, builtin, or alias.
    Command,
    /// `$x`-style variable name.
    Variable,
    /// Regular file in an argument position.
    File,
    /// Directory in an argument position (display includes trailing `/`).
    Directory,
    /// Returned from a `complete -F func` callback — underlying kind unknown.
    Custom,
}

/// One completion candidate. `display` is shown in the Tab-Tab list;
/// `replacement` is the (possibly escaped) text inserted into the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub display: String,
    pub replacement: String,
    pub kind: CandidateKind,
}
```

- [ ] **Step 3: Update the 5 production `Candidate { ... }` constructors**

`grep -n 'Candidate {' crates/huck-engine/src/completion.rs` should show 5 production sites (and possibly some in `mod tests`). For each, add `kind: CandidateKind::Command` (placeholder — Task 2 refines):

```bash
grep -n 'Candidate {' crates/huck-engine/src/completion.rs
```

Sites (approximate line numbers):
- 247: in `complete_command`'s final `.map(...)`. Add `kind: CandidateKind::Command`.
- 262: in `complete_variable`'s final `.map(...)`. Add `kind: CandidateKind::Command`.
- 301: in `complete_file`'s push. Add `kind: CandidateKind::Command`.
- 512: in the filename-rendering branch of the spec-driven path. Add `kind: CandidateKind::Command`.
- 518: in the non-filename branch of the spec-driven path. Add `kind: CandidateKind::Command`.

Example for site 247:
```rust
.map(|n| Candidate {
    display: n.clone(),
    replacement: escape_filename(&n),
    kind: CandidateKind::Command,
})
```

Apply the same shape to all 5.

- [ ] **Step 4: Update test-site Candidate constructions**

Re-grep with `--include` to find Candidate literals in `#[cfg(test)] mod tests` blocks:

```bash
grep -rn 'Candidate {' crates/huck-engine/src/ crates/huck-cli/src/ | grep -v ':[0-9]*:[^/]*//'
```

For any test-site `Candidate { display, replacement }` assertion (in `completion.rs::mod tests` or anywhere else), add `kind: CandidateKind::Command`. The placeholder value is fine — tests assert on `display`/`replacement`, not `kind`, at this point.

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean build; full suite green; clippy clean. ZERO behavior change — every Candidate carries the placeholder Command kind, but no test asserts on kind yet.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
v208 task 1: extend Candidate with CandidateKind field

New CandidateKind enum (Command, Variable, File, Directory, Custom) and
Candidate gains a `kind` field. All in-file Candidate literal constructors
updated with kind: CandidateKind::Command as a placeholder; Task 2 refines
per-producer (Variable for complete_variable, File/Directory for
complete_file, Custom for spec-driven results).

No behavior change yet — kind is plumbed but always Command. HuckHelper
mapping (Task 5) drops the field, so the CLI is unaffected.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Stamp correct `kind` at each producer

**Files:**
- Modify: `crates/huck-engine/src/completion.rs`

Replace the Task 1 placeholder `kind: CandidateKind::Command` with the correct kind at each producer:

- `complete_command` → keep `Command` (no change from Task 1 placeholder).
- `complete_variable` → `Variable`.
- `complete_file` → `Directory` when `is_dir`, else `File`.
- Spec-driven path (filename + non-filename branches) → `Custom`.

- [ ] **Step 1: Update `complete_variable`'s producer**

`crates/huck-engine/src/completion.rs` line ~262:

```rust
matches
    .into_iter()
    .map(|n| Candidate {
        display: n.clone(),
        replacement: n,
        kind: CandidateKind::Variable,
    })
    .collect()
```

- [ ] **Step 2: Update `complete_file`'s producer (Directory/File branch)**

`crates/huck-engine/src/completion.rs` line ~301. The existing `is_dir` branch already drives the trailing-`/` append; reuse it for the kind tag:

```rust
let mut display = name.to_string();
let mut replacement = escape_filename(name);
let kind = if is_dir {
    display.push('/');
    replacement.push('/');
    CandidateKind::Directory
} else {
    CandidateKind::File
};
candidates.push(Candidate { display, replacement, kind });
```

- [ ] **Step 3: Update the spec-driven path (both branches)**

`crates/huck-engine/src/completion.rs` line ~512 (filename branch) and line ~518 (non-filename branch). Both stamp `CandidateKind::Custom`:

```rust
// Site 512 (filename branch):
Candidate { display, replacement, kind: CandidateKind::Custom }

// Site 518 (non-filename branch):
.map(|s| Candidate {
    display: s.clone(),
    replacement: s,
    kind: CandidateKind::Custom,
})
```

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green. Existing tests assert on `display`/`replacement`, not `kind`, so they don't care about the change.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/completion.rs
git commit -m "$(cat <<'EOF'
v208 task 2: stamp correct CandidateKind at each producer

complete_variable → Variable; complete_file → Directory (when is_dir) /
File (otherwise); the spec-driven `complete -F func` path stamps Custom
in both filename-rendering and non-filename branches. complete_command
stays Command (set in Task 1 placeholder).

No behavior change for existing test assertions (they check display /
replacement, not kind). Task 4 adds engine.rs unit tests that assert
on kind specifically.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `Completion` struct + `Engine::complete` method + re-exports

**Files:**
- Modify: `crates/huck-engine/src/engine.rs` (add `Completion` struct + `complete` method)
- Modify: `crates/huck-engine/src/lib.rs` (3 re-exports)

- [ ] **Step 1: Add `Completion` struct + `Engine::complete` method**

In `crates/huck-engine/src/engine.rs`, after the existing `Output` struct definition (search for `pub struct Output {`), add:

```rust
/// The result of a completion query — see [`Engine::complete`].
#[derive(Debug, Clone)]
pub struct Completion {
    /// Byte offset in the input line where the replacement starts.
    /// Embedders substitute `line[start..cursor]` with each candidate's
    /// `replacement` when the user picks it. `start <= cursor`.
    pub start: usize,
    /// Candidates in the order huck would offer them at the prompt.
    /// Alphabetical within each kind.
    pub candidates: Vec<crate::completion::Candidate>,
}
```

In the existing `impl Engine { ... }` block (find `impl Engine` near the top of the file), add the new method alongside the other public methods (e.g. near `run`, `capture`, `exec`):

```rust
/// Return the completion candidates at `cursor` (byte offset) in `line`.
/// The embedder substitutes `line[start..cursor]` with each candidate's
/// `replacement` when the user picks it.
///
/// `cursor` is clamped to `line.len()`. Passing a cursor inside a
/// multi-byte UTF-8 sequence panics (same as `&str` slicing).
///
/// `&mut self` is required because `complete -F func` callbacks may
/// mutate shell state.
pub fn complete(&mut self, line: &str, cursor: usize) -> Completion {
    let clamped = cursor.min(line.len());
    let mut shell = self.cell.borrow_mut();
    let (start, candidates) =
        crate::completion::dispatch::resolve(line, clamped, &mut shell);
    Completion { start, candidates }
}
```

(The exact field name on `Engine` for the shell cell — `self.cell` vs `self.shell_cell` or similar — depends on what's already in `engine.rs`. Grep for it first to use the right name.)

- [ ] **Step 2: Add the three re-exports to lib.rs**

In `crates/huck-engine/src/lib.rs`, find the existing `pub use` re-exports (likely near the bottom — `pub use engine::Engine;` etc.) and add:

```rust
pub use completion::{Candidate, CandidateKind};
pub use engine::Completion;
```

(If `pub use engine::Engine;` already exists, add `Completion` to it: `pub use engine::{Engine, Completion};` — matching the existing style.)

- [ ] **Step 3: Build + clippy check**

```bash
cargo build --workspace -q
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean build; clippy clean. No tests yet exercise `complete`; Task 4 adds them.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/engine.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v208 task 3: Completion struct + Engine::complete method + re-exports

New Engine::complete(&mut self, line: &str, cursor: usize) -> Completion
method exposes the existing completion::dispatch::resolve internal API to
embedders. Cursor is clamped to line.len(); &mut self is required because
`complete -F func` callbacks may mutate shell state.

Crate root re-exports: huck_engine::{Candidate, CandidateKind, Completion}.
Task 4 adds the unit tests. Task 5 updates the HuckHelper rustyline adapter
to drop the new `kind` field when mapping to rustyline::Pair.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: 10 unit tests in `engine.rs::mod tests`

**Files:**
- Modify: `crates/huck-engine/src/engine.rs` (append to `#[cfg(test)] mod tests`)

- [ ] **Step 1: Append the 10 tests**

In `crates/huck-engine/src/engine.rs`, at the end of the `#[cfg(test)] mod tests { ... }` block, add:

```rust
// ===== Completion API (v208) =====

#[test]
fn complete_returns_struct() {
    let mut e = Engine::new();
    let comp = e.complete("", 0);
    assert_eq!(comp.start, 0);
    // Empty-prefix command position: at minimum some builtins are present.
    assert!(!comp.candidates.is_empty(), "expected some builtins, got {:?}", comp.candidates);
}

#[test]
fn complete_at_end_of_line() {
    let mut e = Engine::new();
    let line = "echo $HO";
    let comp = e.complete(line, line.len());
    assert!(
        comp.candidates.iter().any(|c| c.display == "HOME"),
        "expected HOME in {:?}", comp.candidates
    );
}

#[test]
fn complete_with_cursor_beyond_line_len() {
    let mut e = Engine::new();
    let line = "ec";
    let at_end = e.complete(line, line.len());
    let beyond = e.complete(line, 9999);
    assert_eq!(beyond.start, at_end.start);
    assert_eq!(
        beyond.candidates.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(),
        at_end.candidates.iter().map(|c| c.display.as_str()).collect::<Vec<_>>(),
    );
}

#[test]
fn complete_command_position_stamps_command() {
    let mut e = Engine::new();
    let comp = e.complete("ec", 2);
    let echo = comp.candidates.iter().find(|c| c.display == "echo")
        .expect("echo should complete");
    assert_eq!(echo.kind, huck_engine::CandidateKind::Command);
}

#[test]
fn complete_variable_stamps_variable() {
    let mut e = Engine::new();
    e.set_var("MY_V208_TEST_VAR", "x");
    let line = "echo $MY_V208_T";
    let comp = e.complete(line, line.len());
    let v = comp.candidates.iter().find(|c| c.display == "MY_V208_TEST_VAR")
        .expect("var should complete");
    assert_eq!(v.kind, huck_engine::CandidateKind::Variable);
}

#[test]
fn complete_file_stamps_file() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("v208_test_file.txt");
    std::fs::write(&f, "hi").unwrap();
    let mut e = Engine::new();
    let line = format!("ls {}/v208_test", tmp.path().display());
    let comp = e.complete(&line, line.len());
    let cand = comp.candidates.iter()
        .find(|c| c.display == "v208_test_file.txt")
        .expect("file should complete");
    assert_eq!(cand.kind, huck_engine::CandidateKind::File);
}

#[test]
fn complete_directory_stamps_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("v208_test_dir");
    std::fs::create_dir(&d).unwrap();
    let mut e = Engine::new();
    let line = format!("ls {}/v208_test", tmp.path().display());
    let comp = e.complete(&line, line.len());
    let cand = comp.candidates.iter()
        .find(|c| c.display == "v208_test_dir/")
        .expect("dir should complete with trailing /");
    assert_eq!(cand.kind, huck_engine::CandidateKind::Directory);
}

#[test]
fn complete_custom_stamps_custom() {
    let mut e = Engine::new();
    // Register a -F func that produces a single candidate.
    let _ = e.run("_my_v208_completer() { COMPREPLY=( custom_v208_result ); }; complete -F _my_v208_completer mycmd");
    let comp = e.complete("mycmd ", 6);
    let cand = comp.candidates.iter()
        .find(|c| c.display == "custom_v208_result")
        .expect("custom result should appear");
    assert_eq!(cand.kind, huck_engine::CandidateKind::Custom);
}

#[test]
fn complete_does_not_modify_last_status() {
    let mut e = Engine::new();
    let _ = e.run("false");
    assert_eq!(e.last_status(), 1);
    let _ = e.complete("ec", 2);
    assert_eq!(e.last_status(), 1, "complete() must not alter $?");
}

#[test]
fn complete_sees_engine_vars() {
    let mut e = Engine::new();
    e.set_var("MY_V208_VAR", "x");
    let line = "echo $MY_V208_V";
    let comp = e.complete(line, line.len());
    assert!(
        comp.candidates.iter().any(|c| c.display == "MY_V208_VAR"),
        "live engine var should be visible to complete(), got {:?}",
        comp.candidates,
    );
}
```

- [ ] **Step 2: Run the new tests**

```bash
cargo test --workspace --quiet complete_
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 10 new tests pass; full suite green; clippy clean.

If `complete_custom_stamps_custom` fails (couldn't register the spec), check that `set -F`'s parsing accepts the function-definition-then-complete one-liner. If the issue is shell-syntactic, split into two `e.run(...)` calls — define the function, then register.

- [ ] **Step 3: Commit**

```bash
git add crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
v208 task 4: 10 unit tests for Engine::complete

Three groups: basic API shape (3), per-kind stamping (5), engine-state
interaction (2). Each kind variant has a dedicated test exercising the
producer that should stamp it: Command via `ec` → echo; Variable via
$MY_V208_TEST_VAR; File and Directory via a tempdir entry; Custom via a
locally-registered `complete -F func` spec.

complete_does_not_modify_last_status pins the no-side-effects invariant
for the common case (only `complete -F func` callbacks legitimately
mutate state, and that's documented).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Update `HuckHelper` mapping to drop `kind`

**Files:**
- Modify: `crates/huck-cli/src/completion_helper.rs`

- [ ] **Step 1: Drop `kind` in the `Candidate → Pair` mapping**

In `crates/huck-cli/src/completion_helper.rs`, find the `.map(|c: Candidate| Pair { ... })` block (around line 42-49). It currently has:

```rust
.map(|c: Candidate| Pair {
    display: c.display,
    replacement: c.replacement,
})
```

Update it to (optionally) acknowledge the dropped field in a comment:

```rust
.map(|c: Candidate| Pair {
    display: c.display,
    replacement: c.replacement,
    // c.kind dropped — rustyline doesn't model completion kinds.
})
```

This is a one-line change; the destructure naturally ignores any unmentioned fields.

- [ ] **Step 2: Verify the existing rustyline integration test still passes**

```bash
cargo test --workspace --quiet helper_holds_rc_refcell_shell
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `helper_holds_rc_refcell_shell` passes; full suite green; clippy clean. The existing test doesn't observe `kind`, so it passes unmodified after the destructure update.

- [ ] **Step 3: Commit**

```bash
git add crates/huck-cli/src/completion_helper.rs
git commit -m "$(cat <<'EOF'
v208 task 5: HuckHelper drops Candidate.kind when mapping to rustyline::Pair

rustyline doesn't have a completion-kind concept, so the new `kind` field
is silently discarded when mapping Engine's Candidate to rustyline::Pair.
The existing helper_holds_rc_refcell_shell integration test passes
unchanged — it observes only display / replacement, not kind. Note added
to the destructure for future readers.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Docs + rustdoc + final verify

**Files:**
- Modify: `docs/architecture.md`
- Modify: `crates/huck-engine/src/engine.rs` (rustdoc example update)

- [ ] **Step 1: Append architecture.md paragraph**

In `docs/architecture.md`, find the existing `huck-engine` paragraph (last modified in v207). Append (as a continuation or new sub-paragraph):

```
Completion (v208) is exposed as `Engine::complete(line, cursor) -> Completion
{ start, candidates }`. `Candidate` gains a `kind: CandidateKind` tag
(`Command` / `Variable` / `File` / `Directory` / `Custom`) so IDE / TUI
embedders can render icons or sort by kind. Thin wrapper over the existing
`completion::dispatch::resolve` internal API; `&mut self` because
`complete -F func` callbacks may mutate shell state. The CLI's `HuckHelper`
rustyline adapter drops `kind` (rustyline has no kind concept) — REPL
behavior unchanged.
```

- [ ] **Step 2: Append a rustdoc example to `Engine::exec` or `Engine` module-level docs**

In `crates/huck-engine/src/engine.rs`, find the existing `///` doc block on `Engine` (likely module-level, near the top — look for the v207 streaming example). Append:

```rust
//! // Tab-completion query: what would complete at the cursor?
//! let line = "echo $HO";
//! let comp = e.complete(line, line.len());
//! for c in &comp.candidates {
//!     println!("[{:?}] {}", c.kind, c.display);
//! }
//! // Prints lines like:  [Variable] HOME, [Variable] HOSTNAME (if set), etc.
```

- [ ] **Step 3: Run the full sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    if [ $? -ne 0 ]; then
        echo "FAIL: $h"
        tail -20 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"
```

Expected: all green; release binary builds; 128+ harnesses all PASS; smoke test prints `hello` and `exit=0`.

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
v208 task 6: architecture.md note + rustdoc example for Engine::complete

Architecture doc gains a paragraph on Engine::complete, the new Completion
and CandidateKind types, and the HuckHelper kind-dropping behavior.
Engine module-level rustdoc gains a completion-query example. No
bash-divergences.md change (embedder-facing API addition; CLI behavior
byte-identical to v207).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Stop — do NOT merge**

Final whole-branch code review is the controller's call after Task 6. Stop after this commit; the controller will dispatch the review and ask the user to confirm before merging to main.

---

## Self-review

**Spec coverage:**
- `Engine::complete(line, cursor) -> Completion`: Task 3.
- `Completion { start, candidates }` struct: Task 3.
- `Candidate { display, replacement, kind }` extension: Task 1.
- `CandidateKind::{ Command, Variable, File, Directory, Custom }`: Task 1.
- Kind stamping at 4 producers: Task 2.
- Crate-root re-exports of `Candidate`, `CandidateKind`, `Completion`: Task 3.
- `HuckHelper` drops `kind`: Task 5.
- 10 unit tests covering API shape, kind stamping, and state-interaction: Task 4.
- Cursor clamp at `line.len()`: Task 3 implementation.
- `&mut self`: Task 3.
- Architecture doc: Task 6.
- Rustdoc example: Task 6.
- CLI byte-identical: verified in Task 6 final sweep.

**Placeholder scan:** No "TBD" / "implement later". Each code block is complete enough to type-check. The "approximate line numbers" in Task 1 Step 3 are pinned to the grep output; the implementer re-runs grep to find current positions if the file has shifted.

**Type consistency:**
- `CandidateKind::{Command, Variable, File, Directory, Custom}` consistent across Tasks 1 + 2 + 4.
- `Candidate { display, replacement, kind }` consistent in Tasks 1, 2, 4.
- `Completion { start, candidates }` consistent in Tasks 3 + 4.
- `Engine::complete(&mut self, line: &str, cursor: usize) -> Completion` signature stable.

**6 tasks total** — smallest iteration in the embedding arc. No fixup-worthy traps anticipated; the work is well-contained in existing modules.
