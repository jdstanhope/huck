# v203: Rustyline-free `huck-engine` crate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the runtime into a rustyline-free `huck-engine` crate (the execution core) + a `huck-cli` crate (the REPL + rustyline adapters), so `cargo tree -p huck-engine` shows no `rustyline`.

**Architecture:** 4-member workspace `syntax ← engine ← cli ← bin`. Move the rustyline-free modules wholesale into `huck-engine`; split three modules (`shell`, `completion`, `readline_bind`) along clean seams, relocating only their rustyline halves into `huck-cli`; re-point `main.rs` at `huck_cli::run`; move the `rustyline` dependency to `huck-cli`. Pure code-move + visibility refactor — byte-identical behavior.

**Tech Stack:** Rust (edition 2024), Cargo workspaces, rustyline.

**Spec:** `docs/superpowers/specs/2026-06-21-huck-engine-crate-design.md`

**Branch:** `v203-huck-engine-crate`

**CRITICAL context for the implementer:**
- This is a REFACTOR — **byte-identical behavior**. Do NOT change logic. Allowed edits: `git mv` files, move whole functions/`impl`s between files, change visibility (`pub(crate)`→`pub`), fix import paths, edit `Cargo.toml`/`lib.rs`. If you edit a function body's logic, STOP.
- It does NOT compile mid-refactor. **Tasks 2–4 are one atomic compile-fix span** (the workspace is broken until `huck-cli::run` exists and `main.rs` points at it). That's expected for a move; work the compile-fix loop.
- **Baseline test count** (capture FIRST, the equal-count gate): `cargo test --workspace 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print s}'` — record as `BASELINE` (~3460+). Post-refactor `cargo test --workspace` MUST equal it. (A bare `cargo test` only runs ONE package — always use `--workspace`.)

**Current structure (verify with grep; line numbers may drift):**
- The `huck` package is the workspace root: a lib (`src/lib.rs`, declares `pub mod <name>` for each runtime module) + a bin (`src/main.rs` → `huck::shell::run`). `huck-syntax` (v202) is at `crates/huck-syntax`.
- Engine-bound modules (rustyline-free): `alias_expand`, `arith`, `builtins`, `completion_builtins`, `completion_spec`, `continuation`, `executor`, `expand`, `glob_match`, `history`, `job_spec`, `jobs`, `param_expansion`, `procsub`, `prompt`, `shell_state`, `test_builtin`, `traps`.
- `lib.rs` also has `#[cfg(test)] pub(crate) mod test_support` (`CWD_LOCK`, a `Mutex<()>`) used by `builtins`/`completion`/`completion_spec`/`expand`/`executor` tests → must live in `huck-engine`.
- Split modules:
  - `shell.rs`: ENGINE half = `parse_cli`(70), `default_rc_path`(139), `maybe_source_rc_file`(147), `run_program`(206, the headless runner), the signal handlers `install_sigint_handler`(617)/`install_sigchld_handler`(625)/`install_job_control_signals`(641), `fire_prompt_command`(668), `process_line_in_sink`(696), `process_line`(742), and the `CliOptions`/`CliError` types (minus the rustyline `ReadlineError` variant). CLI half = `run`(257, the REPL entry + `Editor`), `apply_readline_settings`(444), `read_logical_command`(503), the top `use rustyline::*` + `use crate::completion::HuckHelper`, and the `CliError::Readline` variant.
  - `completion.rs`: ENGINE half = the `Candidate` struct (line 8) + `complete_command`(203)/`complete_variable`(252)/`complete_file`(272) + their tests. CLI half = `HuckHelper` (struct + `impl rustyline::completion::Completer`/`Hinter`/`Highlighter`/`Validator`/`Helper`, ~361–407) + its `#[cfg(test)]` adapter test (~1243).
  - `readline_bind.rs`: ENGINE half = `DEFAULT_EMACS_BINDS`, `readline_function_names`, `is_known_function`, `keyseq_is_valid` (pure data, used by `Shell` + the `bind` builtin). CLI half = `parse_keyseq`(19, →`rustyline::Event`), `function_to_cmd`(133, →`rustyline::Cmd`), the top `use rustyline::*`.

---

## Task 1: Scaffold `huck-engine` + `huck-cli` crates + workspace

**Files:** root `Cargo.toml`; create `crates/huck-engine/Cargo.toml`, `crates/huck-engine/src/lib.rs`, `crates/huck-cli/Cargo.toml`, `crates/huck-cli/src/lib.rs`.

- [ ] **Step 1: Record BASELINE.** Run the baseline command above; write down the number.

- [ ] **Step 2: `crates/huck-engine/Cargo.toml`:**
```toml
[package]
name = "huck-engine"
version = "0.1.0"
edition = "2024"
description = "huck's terminal-free execution core: expansion, execution, builtins, shell state"
license = "MIT"

[dependencies]
huck-syntax = { path = "../huck-syntax" }
libc = "0.2"
glob = "0.3"
regex = "1.10"
signal-hook = "0.4.4"
signal-hook-registry = "1.4"

[dev-dependencies]
tempfile = "3"
libc = "0.2"
```

- [ ] **Step 3: `crates/huck-engine/src/lib.rs`** — placeholder doc only for now:
```rust
//! `huck-engine` — huck's terminal-free execution core.
//!
//! Parses (via `huck-syntax`), expands, and executes shell scripts/commands
//! with NO terminal/line-editor dependency. MUST NOT depend on `rustyline` —
//! the REPL + line-editor adapters live in `huck-cli`.
```

- [ ] **Step 4: `crates/huck-cli/Cargo.toml`:**
```toml
[package]
name = "huck-cli"
version = "0.1.0"
edition = "2024"
description = "huck's interactive REPL + rustyline line-editor adapters"
license = "MIT"

[dependencies]
huck-engine = { path = "../huck-engine" }
huck-syntax = { path = "../huck-syntax" }
rustyline = "18.0.0"
libc = "0.2"

[dev-dependencies]
expectrl = "0.9.0"
tempfile = "3"
```

- [ ] **Step 5: `crates/huck-cli/src/lib.rs`** — placeholder:
```rust
//! `huck-cli` — huck's interactive REPL over `rustyline`, plus the line-editor
//! adapters (completion `HuckHelper`, readline keymap apply). Depends on
//! `huck-engine` for all execution.
```

- [ ] **Step 6: Root `Cargo.toml`** — add the two members + the bin's new dep. In `[workspace] members`, change to:
```toml
[workspace]
members = [".", "crates/huck-syntax", "crates/huck-engine", "crates/huck-cli"]
```
Leave the `huck` package's `[dependencies]` as-is for now (they're moved in Task 4). Do NOT yet remove `rustyline` from `huck`'s deps.

- [ ] **Step 7: Confirm the new empty crates build.** Run: `cargo build -p huck-engine -p huck-cli 2>&1 | tail -3` → `Finished`. (The `huck` package still has all its modules and still builds.)

- [ ] **Step 8: Commit.**
```bash
git add Cargo.toml crates/huck-engine crates/huck-cli
git commit -m "$(cat <<'EOF'
v203 task 1: scaffold huck-engine + huck-cli crates

Add two empty workspace member crates: huck-engine (no rustyline) and huck-cli
(rustyline). Modules move in tasks 2-3.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Move the rustyline-free modules into `huck-engine`

**Files:** `git mv` 18 modules + extract `test_support`; write `huck-engine/src/lib.rs`. (Workspace will NOT compile until Task 3 — that's expected.)

- [ ] **Step 1: `git mv` the wholesale modules** from `src/` to `crates/huck-engine/src/`:
```bash
cd /home/john/projects/shuck
for m in alias_expand arith builtins completion_builtins completion_spec continuation executor expand glob_match history job_spec jobs param_expansion procsub prompt shell_state test_builtin traps; do
  git mv "src/$m.rs" "crates/huck-engine/src/$m.rs"
done
```

- [ ] **Step 2: Extract `test_support` into `huck-engine`.** Cut the `#[cfg(test)] pub(crate) mod test_support { … CWD_LOCK … }` block out of `src/lib.rs` and put it in a new `crates/huck-engine/src/test_support.rs` (NOT `#[cfg(test)]`-gated at the file level — declare it `#[cfg(test)] pub mod test_support;` in the engine lib.rs so its `CWD_LOCK` is `pub`). Make `CWD_LOCK` `pub` (cross-module test use). Example `crates/huck-engine/src/test_support.rs`:
```rust
//! Shared test-only synchronization (the cwd-changing tests must not race).
use std::sync::Mutex;
pub static CWD_LOCK: Mutex<()> = Mutex::new(());
```

- [ ] **Step 3: Write `crates/huck-engine/src/lib.rs`** declaring the engine modules + re-exporting `huck-syntax`. (The `shell`, `completion`, `readline_bind` modules are added in Task 3; declare them now as `pub mod` so the file is complete — Task 3 creates the files.)
```rust
//! `huck-engine` — huck's terminal-free execution core. (See crate docs above.)

pub mod alias_expand;
pub mod arith;
pub mod builtins;
pub mod completion;          // Task 3 (candidate-gen half)
pub mod completion_builtins;
pub mod completion_spec;
pub mod continuation;
pub mod executor;
pub mod expand;
pub mod glob_match;
pub mod history;
pub mod job_spec;
pub mod jobs;
pub mod param_expansion;
pub mod procsub;
pub mod prompt;
pub mod readline_bind;       // Task 3 (keymap-data half)
pub mod shell;               // Task 3 (process_line/run_program half)
pub mod shell_state;
pub mod test_builtin;
pub mod traps;

#[cfg(test)]
pub mod test_support;

// Re-export the frontend so `huck_engine::lexer::`/`::command::` resolve downstream.
pub use huck_syntax::{brace_expand, command, generate, lexer};
pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};
```

- [ ] **Step 4: Fix intra-engine references to `test_support`.** In the moved engine modules, references were `crate::test_support::CWD_LOCK` — still valid (test_support is now in this crate). No change needed; confirm with `grep -rn "test_support" crates/huck-engine/src/ | grep -v "test_support.rs"`.

- [ ] **Step 5: Do NOT compile yet** (shell/completion/readline_bind split happens in Task 3; the `huck` package is now missing 18 modules). Proceed to Task 3.

- [ ] **Step 6: Commit the wholesale move** (the tree is mid-refactor; commit anyway so the move is a discrete step).
```bash
git add -A
git commit -m "$(cat <<'EOF'
v203 task 2: move rustyline-free runtime modules into huck-engine

git mv the 18 wholesale modules + test_support into crates/huck-engine; declare
them in the engine lib.rs and re-export huck-syntax. shell/completion/readline_bind
are split in task 3; the workspace does not compile until then.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Split shell/completion/readline_bind; create `huck-cli`; wire it

**Files:** create `crates/huck-engine/src/{shell,completion,readline_bind}.rs` (engine halves) + `crates/huck-cli/src/{repl,completion_helper,readline_apply}.rs` (cli halves) + rewrite `huck-cli/src/lib.rs`; delete old `src/{shell,completion,readline_bind}.rs`; rewrite `src/main.rs`; delete `src/lib.rs`.

- [ ] **Step 1: Split `completion.rs`.** Create `crates/huck-engine/src/completion.rs` = the OLD `src/completion.rs` MINUS the `HuckHelper` struct + its `impl rustyline::…` blocks (~lines 361–407) + the adapter test (~1243). Create `crates/huck-cli/src/completion_helper.rs` = the cut `HuckHelper` + impls + adapter test, with imports adjusted: `use huck_engine::completion::Candidate;`, `use huck_engine::shell_state::Shell;`, etc., and `use std::{rc::Rc, cell::RefCell};`. Delete `src/completion.rs`. Verify the engine half has no `rustyline`: `grep -c rustyline crates/huck-engine/src/completion.rs` → 0.

- [ ] **Step 2: Split `readline_bind.rs`.** Create `crates/huck-engine/src/readline_bind.rs` = the OLD file MINUS `parse_keyseq`(19) + `function_to_cmd`(133) + the `use rustyline::*` line. Create `crates/huck-cli/src/readline_apply.rs` = the cut `parse_keyseq` + `function_to_cmd` + the rustyline imports + (if needed) `use huck_engine::readline_bind::…` for any data they reference. Delete `src/readline_bind.rs`. Verify: `grep -c rustyline crates/huck-engine/src/readline_bind.rs` → 0.

- [ ] **Step 3: Split `shell.rs`.** Create `crates/huck-engine/src/shell.rs` = the OLD `src/shell.rs` MINUS `run`(257), `apply_readline_settings`(444), `read_logical_command`(503), the top `use rustyline::*` lines, and `use crate::completion::HuckHelper`. Keep `parse_cli`, `default_rc_path`, `maybe_source_rc_file`, `run_program`, the signal handlers, `fire_prompt_command`, `process_line_in_sink`, `process_line`, and the `CliOptions`/`CliError` types. For `CliError`: keep the variants the engine half uses; if a `Readline(ReadlineError)` variant exists, MOVE it to the cli half (the engine's `CliError` must not name a rustyline type). Adjust the engine half's `crate::` paths as needed (they mostly stay `crate::` since the modules are in the engine now). Verify: `grep -c rustyline crates/huck-engine/src/shell.rs` → 0.

- [ ] **Step 4: Create the cli REPL** `crates/huck-cli/src/repl.rs` = the cut `run` + `apply_readline_settings` + `read_logical_command` + the rustyline imports. Rewrite their internal calls to use the engine: `huck_engine::shell::{parse_cli, run_program, maybe_source_rc_file, process_line, fire_prompt_command, install_* }`, `huck_engine::shell_state::Shell`, `crate::completion_helper::HuckHelper`, `crate::readline_apply::*`. The `run` fn keeps its signature `pub fn run(args: &[String]) -> i32`.

- [ ] **Step 5: Write `crates/huck-cli/src/lib.rs`:**
```rust
//! `huck-cli` — huck's interactive REPL + rustyline adapters (see crate docs).
mod completion_helper;
mod readline_apply;
mod repl;

pub use repl::run;
```

- [ ] **Step 6: Rewrite `src/main.rs`** to call the cli, and **delete `src/lib.rs`** (its content moved to the engine; `huck` is now a bin-only package):
```rust
//! huck — thin binary shim. All logic lives in `huck-cli` (REPL) over
//! `huck-engine` (execution) over `huck-syntax` (frontend).
fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(huck_cli::run(&args));
}
```
```bash
git rm src/lib.rs
```

- [ ] **Step 7: Point the `huck` bin package at `huck-cli`.** In the root `Cargo.toml` `[dependencies]`, REPLACE the existing direct deps used only by the (now-deleted) lib with `huck-cli = { path = "crates/huck-cli" }`. Specifically: remove `rustyline`, `signal-hook`, `signal-hook-registry`, `glob`, `regex` from `huck`'s deps (they belong to engine/cli now); keep `huck-cli`. The bin only needs `huck-cli`. (Task 4 finalizes deps; do a first pass here so it can link.) Keep `[dev-dependencies]` for now.

- [ ] **Step 8: The compile-fix loop.** Run `cargo build 2>&1 | tail -50` and resolve iteratively. Expected classes + fixes:
  - **`E0603 private`** — an engine item the cli (or another engine module) uses across a boundary; widen `pub(crate)`→`pub` in the engine. The compiler lists each.
  - **`E0432 unresolved import`** in a cli file — fix the path to `huck_engine::…` / `crate::…`.
  - **`cannot find crate::X`** in a moved engine module where `X` was another now-moved module — should resolve (all in the engine crate); if `X` was a cli-only thing referenced by the engine, that's a real coupling — STOP and report (the engine must not depend on the cli).
  - Do NOT change logic. Only visibility, paths, and the documented moves.
  Repeat until `cargo build 2>&1 | tail -3` → `Finished`.

- [ ] **Step 9: Smoke test (headless + the binary).**
```bash
cargo build --release 2>&1 | tail -1
ls -l target/release/huck   # same path
./target/release/huck -c 'echo ok'
printf 'echo hi\nexit\n' | ./target/release/huck
```
Expected: `Finished`; binary at `target/release/huck`; prints `ok` then `hi`.

- [ ] **Step 10: Commit.**
```bash
git add -A
git commit -m "$(cat <<'EOF'
v203 task 3: split shell/completion/readline_bind; create huck-cli; wire main

Engine keeps process_line/run_program (shell), Candidate+complete_* (completion),
and the keymap-data (readline_bind). huck-cli gets the REPL `run`, the HuckHelper
completer adapter, and the readline rustyline-apply. main.rs -> huck_cli::run;
the old huck lib is deleted (bin-only package). pub(crate)->pub widened where the
compiler required. No logic changes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Finalize deps; prove the engine is rustyline-free

**Files:** root `Cargo.toml` (deps/dev-deps).

- [ ] **Step 1: Audit the `huck` bin package's deps.** It should depend ONLY on `huck-cli` (path) + whatever `src/main.rs` directly uses (just `std`). Remove any leftover `rustyline`/`glob`/`regex`/`signal-hook*`/`libc` from `huck`'s `[dependencies]` if `main.rs` doesn't use them. Move `[dev-dependencies]` (`expectrl`, `tempfile`, `libc`) — if the remaining `tests/*.rs` integration tests only drive the binary, keep `expectrl`/`tempfile` as `huck` dev-deps; if any moved unit test needs them they're already in engine/cli dev-deps.

- [ ] **Step 2: Prove the goal.** Run:
```bash
cargo tree -p huck-engine 2>&1 | grep -c rustyline | xargs echo "engine rustyline deps:"
cargo tree -p huck-cli 2>&1 | grep -c rustyline | xargs echo "cli rustyline deps:"
cargo build -p huck-engine 2>&1 | tail -1
```
Expected: **engine rustyline deps: 0**; cli rustyline deps: > 0; engine builds standalone. (If the engine shows rustyline, a split left a rustyline reference in an engine file — find it with `grep -rn rustyline crates/huck-engine/src/` and move it to cli.)

- [ ] **Step 3: Commit.**
```bash
git add Cargo.toml
git commit -m "$(cat <<'EOF'
v203 task 4: rustyline lives only in huck-cli/bin; huck-engine is rustyline-free

cargo tree -p huck-engine shows zero rustyline; the engine builds standalone.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Verify equal behavior + interactive smoke + docs

**Files:** `docs/architecture.md`.

- [ ] **Step 1: Equal test count (the gate).**
```bash
cargo test --workspace 2>&1 | grep -E "test result:" | grep -v "0 failed" || echo "ALL GREEN"
cargo test --workspace 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print "POST: " s}'
```
Expected: `ALL GREEN`; `POST` == `BASELINE` from Task 1. A mismatch = a test lost (a moved `mod tests` not compiled, or a `#[cfg(test)]` mod not declared) — investigate before proceeding.

- [ ] **Step 2: Harnesses + clippy.**
```bash
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -iE "Fail: [1-9]|[1-9] failed" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | grep -cE "^warning|^error" | xargs -I{} echo "clippy: {}"
```
Expected: `ALL HARNESSES GREEN`; `clippy: 0`.

- [ ] **Step 3: Interactive smoke test** (the REPL still works over a PTY). Run:
```bash
printf 'x=5\necho "[$((x*2))]"\necho done\nexit\n' | ./target/release/huck
```
Expected output includes `[10]` and `done`. (If an `expectrl` PTY test already covers the interactive editor, run it: `cargo test -p huck-cli 2>&1 | grep "test result:"`.)

- [ ] **Step 4: Distribution path unchanged.**
```bash
ls -l target/release/huck && ./target/release/huck -c 'echo dist-ok'
```
Expected: binary at `target/release/huck`; prints `dist-ok`. (Confirms `build-deb.sh` / brew `cargo install --path .` are unaffected, same as v202.)

- [ ] **Step 5: Update `docs/architecture.md`.** Update the crate-graph note (added in v202) to the 4-crate layout: `huck-syntax` (frontend) ← `huck-engine` (rustyline-free execution core) ← `huck-cli` (REPL + rustyline adapters) ← `huck` (thin bin). Note that `huck-engine` is the embeddable, terminal-free interpreter (`huck_engine::shell::run_program`/`process_line`) and that `rustyline` is confined to `huck-cli`. Keep `cargo test --workspace` guidance.

- [ ] **Step 6: Commit.**
```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v203 task 5: verify equal behavior + document the 4-crate graph

cargo test --workspace == baseline (all green); harnesses green; clippy 0;
headless + interactive smoke tests pass; release binary at the same path.
architecture.md updated to the syntax<-engine<-cli<-bin graph.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Report-back (Task 5)

Report: STATUS, the commit SHAs, BASELINE vs POST `cargo test --workspace` counts (must be equal), the `cargo tree -p huck-engine` rustyline count (must be 0), the full-suite + harness + clippy results, the headless + interactive smoke-test output, the release-binary path, and the count of `pub(crate)`→`pub` widenings (for the reviewer). Flag any engine file that still referenced rustyline and how it was resolved.
