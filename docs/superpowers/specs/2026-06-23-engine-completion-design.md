# v208: `Engine::complete` completion API — Design

**Status:** approved 2026-06-23
**Iteration:** v208
**Builds on:** v204 (Engine facade), v207 (streaming callbacks — the prior "embedding arc" iteration)

## Goal

Expose huck's existing tab-completion machinery as a public method on
`huck_engine::Engine` so embedders (TUIs, IDEs, AI agents wrapping huck as a
shell engine) can ask "what would complete at this input position?" without
running a REPL. Light enrichment: each candidate carries a `kind` tag
(Command / Variable / File / Directory / Custom) so embedders can render
icons, colors, or sort by kind.

## Decisions (from brainstorming)

1. **Expose + add `kind`.** Smallest enrichment that lets IDE/TUI tooling
   distinguish candidate types. No `description`, no sub-kinds for Command,
   no exposure of internal `CompletionContext`.
2. **Struct return** — `Completion { start, candidates }`. Self-documenting;
   room for future fields. Matches v205's `Output` pattern.
3. **Five `CandidateKind` variants** — `Command`, `Variable`, `File`,
   `Directory`, `Custom`. Command lumps executables, functions, builtins,
   aliases. `Directory` distinguished so embedders can render trailing `/`.
   `Custom` for `complete -F func` results where the underlying kind isn't
   known.

## Public API

One new method on `Engine`. Two new public types. Three crate-root re-exports.

```rust
impl Engine {
    /// Return the completion candidates at `cursor` (byte offset) in `line`.
    /// The embedder substitutes `line[start..cursor]` with each candidate's
    /// `replacement` when the user picks it.
    ///
    /// `&mut self` is required because `complete -F func` callbacks may
    /// mutate shell state.
    ///
    /// `cursor` is clamped to `line.len()`. Passing a cursor inside a
    /// multi-byte UTF-8 sequence panics (same as `&str` slicing in general).
    pub fn complete(&mut self, line: &str, cursor: usize) -> Completion;
}

/// The result of a completion query.
#[derive(Debug, Clone)]
pub struct Completion {
    /// Byte offset in `line` where the replacement starts. `start <= cursor`.
    pub start: usize,
    /// Candidates in the order huck would offer them at the prompt.
    /// Alphabetical within each kind; the result is homogeneous-kind unless
    /// any candidates are `Custom` (a `complete -F func` callback).
    pub candidates: Vec<Candidate>,
}

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Text shown in a Tab-Tab list (may include trailing `/` for directories).
    pub display: String,
    /// Text that replaces `line[start..cursor]` when this candidate is chosen.
    /// May be quote-escaped if the cursor was inside a quoted region.
    pub replacement: String,
    /// What kind of completion this is. Useful for IDE rendering.
    pub kind: CandidateKind,
}

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
```

Re-exports at the crate root:
```rust
pub use completion::{Candidate, CandidateKind};
pub use engine::Completion;
```

### Three usage patterns (will be in the rustdoc)

```rust
// 1. Complete at end-of-line — the most common case.
let line = "echo $HO";
let comp = engine.complete(line, line.len());
for c in &comp.candidates {
    println!("[{:?}] {}", c.kind, c.display);
}
// Prints: [Variable] HOME, [Variable] HOSTNAME (if set), etc.
```

```rust
// 2. Complete mid-line.
let line = "cd /us /etc";
let cursor = 6;  // just after "cd /us"
let comp = engine.complete(line, cursor);
// File candidates under /usr/, /usr/bin/, etc. — Directory variants for
// each, with trailing `/` in display.
```

```rust
// 3. Substitute the chosen candidate back into the line.
let line = "echo $HO";
let comp = engine.complete(line, line.len());
if let Some(chosen) = comp.candidates.first() {
    let mut new_line = line[..comp.start].to_string();
    new_line.push_str(&chosen.replacement);
    new_line.push_str(&line[line.len()..]);  // anything after the cursor (empty here)
    assert_eq!(new_line, "echo $HOME");
}
```

## Semantics

### Cursor offset

- `cursor: usize` is a **byte offset** into `line`. UTF-8 boundaries enforced
  by the same `&str` slicing the internal `analyze_full` already does.
- `cursor > line.len()` clamps to `line.len()`. Explicit and documented.
- `cursor == 0` is "what would complete at empty input?" — returns all
  command candidates.

### `start` interpretation

- `start <= cursor` always.
- The embedder substitutes `line[start..cursor]` with each candidate's
  `replacement`. The candidate's `display` is what they show in a chooser UI.
- Empty `candidates` is not an error — `start` is still set (typically equals
  cursor).

### Sort order

- Within each kind, candidates are returned alphabetically (today's
  `dispatch::resolve` sorts via `BTreeSet`).
- The result is **homogeneous-kind** in non-Custom cases — huck's context
  analyzer picks one path (Command-position OR Variable OR File). `Custom`
  results from a `complete -F func` callback are stamped Custom uniformly.

### Empty / whitespace inputs

- `engine.complete("", 0)` → command candidates (PATH + builtins + functions
  + aliases).
- `engine.complete("   ", 3)` → ditto; whitespace-before-cursor is treated as
  command position.

### Variables

- `$x` → `Variable` candidates with prefix `x`.
- `${x` → ditto; the `replacement` does not include the brace.
- Special parameters (`$$`, `$?`, `$@`, `$#`) are NOT enumerated — same as
  today. Documented limitation.

### File completion

- Distinguishes files from directories using the existing path-stat path.
- `Directory` candidate's `display` and `replacement` BOTH include trailing
  `/` (cursor lands after the slash, ready to descend).
- Symlink-to-directory: treated as a directory (follows the link).
- Hidden files: not offered unless prefix starts with `.`. Same as bash.

### Programmable completion (`complete -F func`)

- Registered `-F` spec runs the function during `complete()`; results stamped
  `CandidateKind::Custom`.
- The function may mutate shell state — that's why `Engine::complete` takes
  `&mut self`.
- Panic / `exit` from inside the `-F` body aborts the query → empty result
  returned. Same posture as v207's callback rule.

### State persistence

- `complete()` does NOT modify positional args, `last_status()`, or any of
  the shell state that scripts care about. The only mutations are whatever
  a `complete -F func` callback explicitly performs.

### Reentrancy

- `complete()` borrows `&mut self`. Inside an `ExecBuilder` chain or a v207
  streaming callback, the chain holds `&mut Engine`, so calling `complete`
  is compile-time forbidden.

### Thread safety

- `Engine` stays `!Send + !Sync` (unchanged). Completion runs on the
  caller's thread.

### Error handling

- No `Result` return — empty `candidates` covers all "no useful suggestions"
  cases. Producer errors (e.g. unreadable PATH directory) are silently
  skipped, matching today's `dispatch::resolve` behavior.

## Internal architecture

### Extend `Candidate` with `kind`

```rust
// crates/huck-engine/src/completion.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind { Command, Variable, File, Directory, Custom }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub display: String,
    pub replacement: String,
    pub kind: CandidateKind,    // NEW
}
```

Breaking change to anyone literal-constructing `Candidate`. In-tree callers
(~10 sites across `completion.rs`, `completion_builtins.rs`,
`completion_spec.rs`) updated to add the field. No published consumers (only
the in-tree `HuckHelper` adapter, which drops the new field when mapping to
`rustyline::Pair`).

### Stamp `kind` at each producer

- `complete_command(prefix, path, funcs, aliases)` — all results stamped
  `Command`.
- `complete_variable(prefix, var_names)` — all results stamped `Variable`.
- `complete_file(dir, prefix, home)` — already distinguishes files from
  directories internally (appends `/` to directory display); reuse that
  branch to stamp `Directory` or `File`.
- `completion_spec::run_spec` (programmable completion via `complete -F`) —
  all results stamped `Custom`.

### `Engine::complete` method

```rust
// crates/huck-engine/src/engine.rs

#[derive(Debug, Clone)]
pub struct Completion {
    pub start: usize,
    pub candidates: Vec<Candidate>,
}

impl Engine {
    pub fn complete(&mut self, line: &str, cursor: usize) -> Completion {
        let clamped = cursor.min(line.len());
        let mut shell = self.cell.borrow_mut();
        let (start, candidates) = crate::completion::dispatch::resolve(line, clamped, &mut shell);
        Completion { start, candidates }
    }
}
```

Thin wrapper. ~15 LOC including the `cursor` clamp.

### `HuckHelper` update

```rust
// crates/huck-cli/src/completion_helper.rs
.map(|c: Candidate| Pair {
    display: c.display,
    replacement: c.replacement,
    // c.kind dropped — rustyline doesn't model completion kinds.
})
```

One-line change. The existing `helper_holds_rc_refcell_shell` test continues
to pass (it doesn't observe `kind`).

### Crate-root re-exports

```rust
// crates/huck-engine/src/lib.rs
pub use completion::{Candidate, CandidateKind};
pub use engine::Completion;
```

### What we DON'T touch

- `complete -F func` (programmable completion) — works as-is; just gets the
  `Custom` kind tag.
- `compgen` / `compopt` / `complete` builtins — unchanged.
- `CompletionContext` enum — kept internal (not re-exported).
- CLI REPL behavior — byte-identical (new `kind` field doesn't reach
  rustyline).

## CLI dogfood

No CLI changes. The CLI's `HuckHelper` already used `completion::dispatch::resolve`
directly; we update the one Candidate-to-Pair mapping to drop the new field.

## Build / packaging

No new crate deps. No new modules. All changes are additive (or in-place
extensions of existing types). Release binary build path unchanged.

## Testing & verification

### Unit tests in `crates/huck-engine/src/engine.rs::mod tests` (~10 tests)

**Basic API shape (3):**
- `complete_returns_struct`
- `complete_at_end_of_line`
- `complete_with_cursor_beyond_line_len`

**Kind stamping (5):**
- `complete_command_position_stamps_command`
- `complete_variable_stamps_variable`
- `complete_file_stamps_file`
- `complete_directory_stamps_directory`
- `complete_custom_stamps_custom` — register `complete -F func cmd` via
  `engine.run("...")`, then `engine.complete("cmd ", 4)` → results have
  `kind == Custom`.

**Engine state interaction (2):**
- `complete_does_not_modify_last_status`
- `complete_sees_engine_vars`

### Existing test updates

- The `HuckHelper` test `helper_holds_rc_refcell_shell` in
  `completion_helper.rs` continues to pass (doesn't observe kind).
- Existing completion unit tests in `completion.rs::mod tests` get the new
  `kind` field added wherever Candidates are constructed in expected-result
  assertions. Mechanical change.

### Doc example update

Append to the `Engine::exec` rustdoc:

```rust
//! // Tab-completion query: what would complete at the cursor?
//! let line = "echo $HO";
//! let comp = e.complete(line, line.len());
//! for c in &comp.candidates {
//!     println!("[{:?}] {}", c.kind, c.display);
//! }
//! // Prints lines like:  [Variable] HOME, [Variable] HOSTNAME (if set), etc.
```

### Bash-diff harness

Not applicable — v208 exposes an embedding-API surface that has no bash
equivalent (bash doesn't offer programmatic completion from outside). The
self-consistency check: the embedded `Engine::complete` produces the same
`Vec<Candidate>` as the CLI's `HuckHelper`. We verify indirectly via the
existing rustyline integration test continuing to pass.

### CLI byte-identical gate

`Engine::run` / `capture` / `exec(...)` paths unchanged. All 128 existing
harnesses + v205/v206/v207 harnesses pass. Headless CLI smoke test identical
to v207.

### Workspace gates

- `cargo test --workspace --quiet` — green, baseline + new tests.
- `cargo test --workspace --doc --quiet` — doc example passes.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo build --release --workspace` — clean.

## Risks & mitigations

- **Adding `kind` field is a structural change to `Candidate`.** In-tree
  literal constructors must be updated. Mitigate: grep for `Candidate {` and
  `Candidate::new` to enumerate sites; update all. Compiler enforces.
- **`HuckHelper` mapping must continue to drop the new field cleanly.** A
  silent breakage here would surface as a REPL behavior regression.
  Mitigate: the existing rustyline integration test exercises the mapping;
  it must pass after the change.
- **`complete -F func` callbacks executing during `complete()` may mutate
  user variables.** This is intentional bash behavior; not a bug. Documented
  on `Engine::complete`.
- **`cursor` UTF-8 boundary panic.** Documented; same as `&str` slicing.
  Embedders dealing with multi-byte content must align cursor to char
  boundaries (standard Rust practice).

## Out of scope

- Sub-kinds for `Command` (Function / Builtin / Alias / Executable).
- `description: Option<String>` on Candidate.
- Exposing `CompletionContext`.
- `Engine::register_completion_spec(...)` (programmatic spec install).
- Narrowed entry points (`complete_command_only`, etc.).
- Streaming / paginated completion.
- `#[non_exhaustive]` on `CandidateKind`.
- Hidden-file flag.
- Non-UTF-8 cursor / line.
- `Engine: Send` refactor.
- crates.io publish / semver freeze.

## Task decomposition (for the plan)

1. **Add `CandidateKind` enum + extend `Candidate`** in `completion.rs`.
   Update all in-tree literal constructors. Suite green.
2. **Stamp `kind` at each producer** (`complete_command`,
   `complete_variable`, `complete_file`, `run_spec`). Suite green.
3. **Add `Completion` struct + `Engine::complete` method** in `engine.rs`.
   Crate-root re-exports in `lib.rs`. 10 unit tests in `engine.rs::mod tests`.
4. **Update `HuckHelper` mapping** to drop `kind` when mapping to
   `rustyline::Pair`. Existing rustyline test must pass.
5. **Docs**: rustdoc example on `Engine::exec`; brief paragraph in
   `docs/architecture.md`.
6. **Verify**: full suite + harness sweep + release binary + CLI smoke.

Smallest iteration in the embedding arc — comparable to v204 in scope.
