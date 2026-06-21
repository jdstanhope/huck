# v202: Extract a `huck-syntax` crate (lexer + parser + AST + generator) — Design

**Status:** approved 2026-06-21
**Iteration:** v202
**Type:** structural refactor (no behavior change)

## Goal

Split huck's Shell-free frontend — the lexer, the command AST + parser, brace
expansion, and the AST→source generator — into a separate **`huck-syntax`**
crate, with the existing single crate becoming a 2-member Cargo workspace. The
runtime (`huck`) depends on `huck-syntax`; the dependency is one-directional and
compiler-enforced.

## Motivation

A pre-feature architecture review flagged the frontend as Shell-free and a
`huck-syntax` crate as feasible. A fresh dependency audit (2026-06-21, at v201)
confirmed it on the current code:

- `lexer.rs` references only `crate::command` + `crate::brace_expand` types — no
  `Shell`/`ExecOutcome`/`executor`/`expand` references, no external crates.
- `command.rs` references only `crate::lexer` types + `crate::shell::lex_error_message`
  (3×, a pure formatter over `LexError`/`ParseError`).
- `brace_expand.rs` has zero `crate::` deps (used only by the lexer).
- `generate.rs` references `crate::command`/`crate::lexer` + one pure helper
  `crate::builtins::escape_double_quote_value`; no runtime types.
- `arith` is **not** a frontend dependency — it appears only in doc comments;
  `command.rs` stores arith bodies as raw `Word`s and the executor parses them at
  runtime.

Benefits: the Shell-free boundary becomes compiler-enforced (a frontend file
cannot accidentally `use crate::shell_state::Shell`), incremental builds of the
frontend get faster, and the parser becomes reusable standalone later.

## Decisions (from brainstorming)

1. **Crate scope:** `huck-syntax` = `lexer` + `command` (AST + parser) +
   `brace_expand` + `generate` (AST→source), plus the relocated helpers
   `lex_error_message`, `parse_error_message` (from `shell.rs`) and
   `escape_double_quote_value` (from `builtins.rs`). `continuation.rs` stays in
   `huck` (uses re-exported `lexer`).
2. **Internal workspace crate** — a `path` dependency, NOT published. No curated
   public API or separate versioning now; `pub(crate)`→`pub` widening only as the
   compiler requires.
3. **Re-export at the `huck` crate root** so existing `crate::lexer::` /
   `crate::command::` / `crate::generate::` paths in the runtime resolve
   unchanged (near-zero use-site churn).

## Architecture

### Workspace layout

```
huck/                          # workspace root + the `huck` package (bin + runtime lib)
  Cargo.toml                   # [workspace] members = [".", "crates/huck-syntax"]
                               # [dependencies] huck-syntax = { path = "crates/huck-syntax" }
  src/                         # executor, expand, shell_state, builtins, shell, … (unchanged)
    lib.rs                     # `pub use huck_syntax::{lexer, command, brace_expand, generate, …};`
    main.rs                    # unchanged (`huck::shell::run`)
    continuation.rs            # stays; uses re-exported lexer
  crates/huck-syntax/
    Cargo.toml                 # name = "huck-syntax"; no deps on `huck`; no external crates
    src/
      lib.rs                   # pub mod lexer/command/brace_expand/generate; + helpers
      lexer.rs                 # moved (8262 L)
      command.rs               # moved (6049 L)
      brace_expand.rs          # moved (371 L)
      generate.rs              # moved (~1000 L)
```

### Dependency direction

`huck-syntax` depends on **nothing** in `huck` and on **no external crates**
(pure `std`). `huck` depends on `huck-syntax` via `path`. The compiler forbids a
cycle, so the Shell-free boundary is structurally enforced.

### Visibility & re-export

- **`pub(crate)` → `pub`** for every moved item the `huck` crate references across
  the new boundary (e.g. `arith_string_to_word`, `word_literal_text`, the two
  error formatters, `escape_double_quote_value`, and any other helper). The exact
  set is enumerated mechanically during implementation by compiling and resolving
  each `E0603` (private item) error — the compiler produces the list. Intra-`huck-syntax`-only
  helpers stay `pub(crate)` (now scoped to the syntax crate).
- **Self-references inside the moved files** stay valid: `crate::lexer::` /
  `crate::command::` still resolve (both live in `huck-syntax`). The 3
  `crate::shell::lex_error_message` calls in `command.rs` become
  `crate::lex_error_message` (now local to `huck-syntax`).
- **`huck/src/lib.rs`** re-exports the moved modules + helpers at the crate root:
  ```rust
  pub use huck_syntax::{lexer, command, brace_expand, generate};
  pub use huck_syntax::{lex_error_message, parse_error_message, escape_double_quote_value};
  ```
  Every existing `crate::lexer::Token` / `crate::command::Command` /
  `crate::generate::…` path in the ~12 runtime files resolves unchanged.
- **`shell.rs`** loses its `lex_error_message`/`parse_error_message` definitions
  (moved to `huck-syntax`); its call sites use the re-export.
- **`builtins.rs`** loses `escape_double_quote_value` (moved); its `declare -p`
  call sites use the re-export.

### Helper relocations

- `lex_error_message` + `parse_error_message` (`shell.rs` → `huck-syntax`): pure
  formatters over `LexError`/`ParseError`. `lex_error_message` calls
  `parse_error_message` (`Substitution` variants) — both move together.
- `escape_double_quote_value` (`builtins.rs` → `huck-syntax`): a pure 12-line
  string escaper used by `generate` (AST→source quoting) and `builtins`
  (`declare -p`). Moves into `huck-syntax`; `builtins.rs` uses the re-export.

## Build / CI / packaging

- Root `Cargo.toml`: add `[workspace] members` + the `huck-syntax` path dep.
  Keep `[package] name = "huck"` with the default lib (`src/lib.rs`) + bin
  (`src/main.rs`).
- `packaging/deb/build-deb.sh` runs `cargo build --release --manifest-path
  Cargo.toml` and installs `target/release/huck`. A workspace root build still
  produces `target/release/huck` at the same path → **no change needed**.
- Homebrew tap builds from source via `cargo build` → unchanged.
- No `.github/workflows/` → no CI to update.
- `docs/architecture.md`: add a note describing the `huck-syntax` boundary.
  `docs/RELEASING.md`: no change.

## Testing & verification

- **Unit tests move with their files.** `lexer.rs`/`command.rs`/`generate.rs`/
  `brace_expand.rs` carry their `#[cfg(test)] mod tests` into `huck-syntax`;
  verified they reference only each other (no `Shell`). They run via workspace
  `cargo test`.
- **Integration tests** (`tests/*.rs`) drive the binary (`CARGO_BIN_EXE_huck`) or
  the `huck` lib; the few that import `lexer`/`command` resolve via the re-export.
- **Verification gates:**
  - Full `cargo test` (workspace) all-green with the **same total test count** as
    before the move (proves no test was lost or silently skipped). Capture the
    pre-move count first.
  - All `tests/scripts/*_diff_check.sh` harnesses green.
  - `cargo clippy --all-targets` clean (workspace).
  - Release binary builds; `./target/release/huck -c 'echo ok'` runs.
- **Boundary enforcement:** `huck-syntax/Cargo.toml` has no `huck` dependency; the
  compiler rejects any `use` of a `huck` item from `huck-syntax`. (Document this;
  optionally add a comment in `huck-syntax/src/lib.rs`.)
- **Success criterion:** byte-for-byte identical runtime behavior. This is a pure
  code-move + visibility/re-export refactor — the existing suite + harnesses are
  the proof; no new behavioral tests are required.

## Risks & mitigations

- **Large diff (~15k LOC moved).** Mitigate with `git mv` so history follows the
  files; the move is mechanical (no logic edits).
- **Hidden `pub(crate)` coupling.** Surfaced deterministically by the compiler
  (`E0603`); fix by widening to `pub`. No guesswork.
- **A moved unit test that references a `huck` type.** The audit found none, but
  if the compiler surfaces one during the move, that test (and only that test)
  stays in `huck` as an integration-style test, or the referenced item is itself
  relocated. Flag rather than force.
- **Workspace target-dir / build-path surprises.** Verified `target/release/huck`
  is still produced; re-confirm during implementation before touching packaging.

## Out of scope

- Publishing `huck-syntax` to crates.io (curated public API, docs, versioning).
- Moving `continuation.rs`, `arith.rs`, or any runtime module.
- Any behavior change, new feature, or bug fix.
- Splitting `lexer.rs`/`command.rs` into smaller files (a separate concern).

## Task decomposition (for the plan)

1. Scaffold the workspace + empty `huck-syntax` crate; wire the path dep; confirm
   it builds.
2. `git mv` the four frontend files into `huck-syntax/src/`; add `huck-syntax/src/lib.rs`.
3. Relocate the three helpers (`lex_error_message`, `parse_error_message`,
   `escape_double_quote_value`) into `huck-syntax`; fix the intra-crate refs.
4. Add the re-exports to `huck/src/lib.rs`; remove the moved `mod` lines + old
   helper definitions; resolve every `E0603` by widening to `pub`.
5. Verify: pre/post test counts equal, full suite + harnesses + clippy green,
   release binary runs, deb path unchanged; update `docs/architecture.md`.
