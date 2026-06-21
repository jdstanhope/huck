# v203: Make the execution engine a rustyline-free `huck-engine` crate — Design

**Status:** approved 2026-06-21
**Iteration:** v203
**Type:** structural refactor (no behavior change)
**Builds on:** v202 (`huck-syntax` crate)

## Goal

Carve the rustyline-coupled REPL + line-editor adapters out of the runtime into a
new `huck-cli` crate, leaving a **`huck-engine`** crate that parses, expands, and
executes shell scripts/commands with **zero `rustyline` dependency** — an
embeddable, terminal-free shell interpreter. Success criterion:
`cargo tree -p huck-engine` shows no `rustyline`.

## Motivation

huck already runs non-interactively (`huck -c`, scripts, piped stdin via
`run_command`/`process_line`, gated by `is_interactive`). The capability exists;
this iteration packages it as a clean reusable crate so other Rust programs can
embed the interpreter without pulling a line editor / terminal deps.

A coupling audit (2026-06-21, at v202) found the `rustyline` surface is thin and
concentrated in adapters:
- The `Shell` struct (`shell_state.rs`) is rustyline-free (its `ReadlineSettings`
  is pure data).
- `executor.rs` is rustyline-free (libc `fork`/`setpgid` only).
- `prompt.rs`, `completion_spec.rs` mention rustyline only in comments.
- `completion.rs`'s real rustyline code is the `HuckHelper` `Completer` adapter
  (lines ~361–407) + one adapter test; its candidate-generation (`Candidate` +
  `complete_command`/`complete_variable`/`complete_file`) is rustyline-free.
- `readline_bind.rs` uses rustyline only in its apply functions
  (`parse_keyseq`→`Event`, `function_to_cmd`→`Cmd`); its keymap DATA
  (`DEFAULT_EMACS_BINDS`, `readline_function_names`, `is_known_function`,
  `keyseq_is_valid`) is pure and used by `Shell` + the `bind` builtin.
- Only `readline_bind.rs` and `shell.rs` carry a real top-level `use rustyline`.
- Zero integration tests use the `huck::` lib API (all drive the binary via
  `CARGO_BIN_EXE_huck`), so the lib move is invisible to them.

## Decisions (from brainstorming)

1. **Four-crate workspace** (`huck-syntax` already exists): add `huck-engine`
   (rustyline-free core) and `huck-cli` (REPL + rustyline adapters); `huck`
   becomes a thin binary calling `huck_cli::run`.
2. **`Shell` stays whole in the engine** — not split this iteration (it's already
   rustyline-free; its interactive data fields are pure data the engine's own
   builtins manipulate).
3. **First cut = "engine is rustyline-free"** — no polished `Engine`/builder
   embedding API yet; the engine exposes its modules + `run_command`/`process_line`.

## Architecture

### Workspace layout

```
crates/huck-syntax   (lib)  frontend: lexer, command AST+parser, brace_expand, generate   [no deps]
crates/huck-engine   (lib)  rustyline-FREE execution core                                 → huck-syntax
crates/huck-cli      (lib)  REPL + rustyline adapters                                      → huck-engine + rustyline
huck                 (bin)  thin main.rs → huck_cli::run(args)                             → huck-cli
```

Dependency direction (acyclic, compiler-enforced): `syntax ← engine ← cli ← bin`.
Only `huck-cli` and the `huck` bin may use `rustyline`; a stray `use rustyline`
in `huck-engine` will not compile. External deps: `glob`/`regex`/`libc`/
`signal-hook`/`signal-hook-registry` → `huck-engine`; `rustyline` → `huck-cli`
only. Dev-deps (`expectrl`/`tempfile`) follow the tests that use them.

### Module placement

**Wholesale → `huck-engine`** (rustyline-free): `executor`, `expand`,
`param_expansion`, `arith`, `shell_state`, `builtins`, `traps`, `jobs`,
`job_spec`, `history`, `glob_match`, `test_builtin`, `alias_expand`, `procsub`,
`prompt`, `continuation`, `completion_spec`, `completion_builtins`. Each carries
its `#[cfg(test)] mod tests`.

**Three modules split** (engine-core part + cli-adapter part):

1. `shell.rs` → **engine** keeps `process_line` (canonical execute-string path),
   `run_command` (headless `-c`/script), and the non-REPL CLI arg-parsing;
   **cli** gets `run` (the rustyline `Editor` loop + apply-readline-settings),
   the top-of-file `use rustyline`, and the `HuckHelper` wiring.
2. `completion.rs` → **engine** keeps the `Candidate` struct +
   `complete_command`/`complete_variable`/`complete_file` (candidate generation);
   **cli** gets `HuckHelper` (the `impl rustyline::completion::Completer` +
   `Hinter`/`Highlighter`/`Validator`/`Helper`, converting `Candidate` →
   `rustyline::completion::Pair`) and its adapter test.
3. `readline_bind.rs` → **engine** keeps the pure-data keymap model
   (`DEFAULT_EMACS_BINDS`, `readline_function_names`, `is_known_function`,
   `keyseq_is_valid`); **cli** gets the rustyline apply (`parse_keyseq`→`Event`,
   `function_to_cmd`→`Cmd`).

`huck-syntax` is unchanged.

### Public surface & re-exports

- `huck-engine/src/lib.rs` declares the engine modules as `pub mod` and
  re-exports `huck-syntax` at its root (`pub use huck_syntax::{lexer, command,
  brace_expand, generate};`) so downstream paths resolve.
- Cross-crate visibility: every engine item the cli references must be `pub`. The
  set is enumerated mechanically by compiling and resolving each `E0603` (private
  item) — the compiler produces the list. Intra-engine-only helpers stay
  `pub(crate)` (now scoped to the engine crate).
- `huck-cli/src/lib.rs` exposes `pub fn run(args: &[String]) -> i32` (the entry
  the binary calls) + the `HuckHelper`/readline-apply modules. It calls into
  `huck_engine::{shell, shell_state, completion, readline_bind, …}`.
- `huck/src/main.rs` becomes a thin shim: `std::process::exit(huck_cli::run(&args))`.

### Data flow

- **Interactive:** `main` → `huck_cli::run` builds a `huck_engine::shell_state::Shell`,
  wraps it `Rc<RefCell<>>`, drives the rustyline `Editor` with the cli `HuckHelper`;
  per input line calls `huck_engine::shell::process_line`; applies any dirty
  readline settings via the cli apply layer over the engine's keymap data model.
- **Headless:** `huck_cli::run` detects `-c`/script/non-tty and delegates to
  `huck_engine::run_command` (no `Editor`). An external embedder skips `huck-cli`
  entirely and calls `huck_engine::run_command` / `process_line` directly —
  rustyline-free.

## Build / CI / packaging

- Root `Cargo.toml` workspace `members` gains `crates/huck-engine` and
  `crates/huck-cli`. The `huck` package becomes bin-focused, depending on
  `huck-cli`.
- `rustyline` moves from the `huck` package's deps to `huck-cli`'s deps.
- `packaging/deb/build-deb.sh` (`cargo build --release` → `target/release/huck`)
  and Homebrew (`cargo install --path .`) still build the `huck` bin at the same
  path → no packaging change (re-verify during implementation, as in v202).
- No CI to update. `docs/architecture.md` gains a crate-graph note; `docs/RELEASING.md`
  unchanged (release version is still the root `huck` `Cargo.toml`).

## Testing & verification

- **Unit tests move with their modules** (engine tests in `huck-engine`, the
  HuckHelper/REPL tests in `huck-cli`, frontend tests already in `huck-syntax`).
- **Gates:**
  - `cargo tree -p huck-engine | grep -c rustyline` **== 0** (the goal). `cargo
    tree -p huck-cli | grep -c rustyline` > 0 (the adapters live there).
  - `cargo test --workspace` count **== the pre-refactor baseline (capture it
    first; currently 3460)**, all green — proves no test lost (the v202 lesson:
    a bare `cargo test` runs only one package).
  - All `tests/scripts/*_diff_check.sh` harnesses green (binary behavior
    unchanged); `cargo clippy --all-targets` clean.
  - Binary builds; **headless** `./target/release/huck -c 'echo ok'` AND an
    **interactive** smoke test (e.g. `printf 'echo hi\nexit\n' | ./target/release/huck -i`
    or an `expectrl` PTY check) both work.
  - Distribution: `cargo build --release` → same `target/release/huck`;
    `cargo install --path .` still installs the `huck` bin.
- **Success criterion:** byte-for-byte identical runtime behavior — a pure
  code-move + visibility/re-export refactor; the existing suite + harnesses are
  the proof.

## Risks & mitigations

- **Largest diff yet** (~30k LOC re-homed across two new crates). Use `git mv` so
  history follows; the splits are surgical (move whole functions/impls, no logic
  edits).
- **The three module splits** are the only non-mechanical part. Mitigate by
  splitting along the already-clean seams: the `Candidate` type (completion), the
  `ReadlineSettings`/keymap-data vs apply (readline_bind), and `process_line`/
  `run_command` vs `run` (shell). Verify each split file's engine half has no
  `rustyline` before wiring the cli half.
- **Hidden `pub(crate)` coupling across the engine→cli seam** — surfaced
  deterministically by `E0603`; fix by widening to `pub`. No guesswork.
- **A moved unit test referencing the wrong crate's item** — the compiler flags
  it; place the test with the code it exercises (HuckHelper test → cli).
- **`shell.rs`'s `process_line`/`run_command` must be rustyline-free** to live in
  the engine — confirm by grepping the engine half for `rustyline` before
  committing the split (they should be; only `run` touches the `Editor`).

## Out of scope

- A polished `Engine`/builder embedding API, config injection, or a stable
  semver public surface (a later iteration if embedding demand appears).
- Splitting the `Shell` god-object into core + interactive state.
- Moving `arith`/`expand`/`param_expansion` out of the engine (they belong there).
- Publishing any crate to crates.io.
- Any behavior change, feature, or bug fix.

## Task decomposition (for the plan)

1. Scaffold `huck-engine` + `huck-cli` crates; wire workspace members + path deps;
   capture the baseline `cargo test --workspace` count.
2. Move the wholesale rustyline-free modules into `huck-engine` (`git mv`);
   re-export `huck-syntax`; resolve the first wave of `E0603` widenings; get
   `huck-engine` compiling against a temporary stub binary.
3. Perform the three splits: relocate `run`/`HuckHelper`/readline-apply into
   `huck-cli`; keep `process_line`/`run_command`/`Candidate`/keymap-data in the
   engine; wire `huck-cli::run`; point `main.rs` at it. Resolve remaining `E0603`.
4. Move `rustyline` to `huck-cli`'s deps; confirm `cargo tree -p huck-engine` has
   no rustyline.
5. Verify: baseline-equal `cargo test --workspace`, harnesses, clippy, headless +
   interactive smoke tests, packaging path; update `docs/architecture.md`.
