# v204: A polished `Engine` embedding API for `huck-engine` — Design

**Status:** approved 2026-06-21
**Iteration:** v204
**Builds on:** v203 (the rustyline-free `huck-engine` crate)

## Goal

Add a small, ergonomic **`Engine`** facade to `huck-engine` so a Rust program can
embed huck and run shell scripts/commands programmatically — without wrestling
the raw building blocks (`run_program`'s 6-arg `Rc<RefCell<Shell>>` signature,
`process_line` + `ExecOutcome`, `execute_capturing`'s pre-parsed `Sequence`,
scattered `Shell` getters). Dogfood it by routing the `huck-cli` binary's headless
path through the new API.

## Decisions (from brainstorming)

1. **Focused facade** — `Engine` owns a persistent `Shell`; run/capture a script
   string, run a file, get/set variables, set positionals + `$0`. A minimal
   builder for a couple of knobs. YAGNI on cwd/sink/restricted/timeout/custom-builtins.
2. **Exit-code model, capture stdout only** — `run -> i32`; `capture -> Output
   { stdout, exit_code }` (stderr inherits the process, like bash). **No `Result`**:
   parse errors → exit 2 + stderr message; missing file → 127. Shell-faithful.
3. **Dogfood via the CLI** — refactor the binary's headless `-c`/script path to
   build an `Engine` and call it; the gate is byte-identical CLI behavior.

## Public API

New file `crates/huck-engine/src/engine.rs`, re-exported as `huck_engine::Engine`
(+ `Output`, `EngineBuilder`). `Engine` owns an `Rc<RefCell<Shell>>`; state
(variables, functions, cwd, `$?`, positionals) persists across calls — a
persistent session.

```rust
pub struct Engine { /* Rc<RefCell<Shell>> */ }

#[derive(Debug, Clone)]
pub struct Output { pub stdout: String, pub exit_code: i32 }

impl Engine {
    /// A fresh shell ($0 = "huck"). Installs NO signal handlers, reads no rc file.
    pub fn new() -> Self;
    pub fn builder() -> EngineBuilder;
    /// Wrap a caller-owned, possibly pre-configured shell cell (the dogfood/
    /// advanced-embedder entry — the caller keeps signal/process ownership).
    pub fn from_shell_cell(cell: std::rc::Rc<std::cell::RefCell<Shell>>) -> Self;

    /// Run a script string with `bash -c` semantics (no "main" call frame).
    /// stdout+stderr inherit the process. Returns the exit status.
    pub fn run(&mut self, src: &str) -> i32;

    /// Run a script string, capturing stdout into the returned `String`
    /// (stderr still inherits). `bash -c` semantics; returns {stdout, exit_code}.
    pub fn capture(&mut self, src: &str) -> Output;

    /// Read and run a script FILE with script semantics (a "main" call frame,
    /// `$0` = the path). Missing/unreadable file -> stderr message + exit 127.
    pub fn run_file(&mut self, path: &std::path::Path) -> i32;

    pub fn var(&self, name: &str) -> Option<String>;     // read a shell variable
    pub fn set_var(&mut self, name: &str, value: &str);  // set a global variable
    pub fn set_args(&mut self, args: Vec<String>);       // positional params $1..$N
    pub fn set_arg0(&mut self, name: &str);              // $0
    pub fn last_status(&self) -> i32;                    // $? after the last run
}

pub struct EngineBuilder { /* arg0, args, env seeds */ }
impl EngineBuilder {
    pub fn env(self, key: &str, value: &str) -> Self;  // seed a variable
    pub fn arg0(self, name: &str) -> Self;
    pub fn args(self, args: Vec<String>) -> Self;
    pub fn build(self) -> Engine;
}
```

`Engine` derives nothing requiring `Shell: Clone` (it holds an `Rc`). `Default for
Engine` = `Engine::new()`.

## Semantics & implementation

- **`run(src)` / `capture(src)`** run a FULL script string (multi-line, functions,
  control flow, pipelines, redirects) — `bash -c` semantics: `is_interactive =
  false`, **no** "main" call frame pushed. They set `$0`/positionals from the
  Engine's state, run, fire the EXIT trap, and return the status (last command or
  `exit N`). `capture` threads `StdoutSink::Capture(&mut buf)`; stderr inherits.
- **`run_file(path)`** reads the file and runs its contents with **script**
  semantics: a "main" call frame is pushed (so top-level `FUNCNAME`/`BASH_SOURCE`
  match `bash script.sh`), `$0` = the path. A read failure prints
  `huck: <path>: <error>` to stderr and returns **127**.
- **Parse errors**: the normal lex/parse error path already prints to stderr and
  yields exit **2**; `run`/`capture` return that. No `Result`.
- **State persistence**: the owned `Shell` carries variables/functions/cwd/`$?`/
  positionals across calls. `var` reads via `Shell::lookup_var`; `set_var` writes
  a global (top-level assignment semantics) via `Shell::set`; `last_status` =
  `Shell::last_status`.
- **No process-global side effects**: `Engine` installs no signal handlers, reads
  no rc file, and does not touch `is_interactive` beyond the headless default.

### Required refactor: sink-parameterized run path

Add `run_program_in_sink(contents, argv0, args, label, push_main_frame, sink,
shell_cell)` (mirroring the existing `process_line_in_sink`) so `run` and
`capture` share ONE code path differing only in the `StdoutSink`. The existing
`run_program` becomes a thin `run_program_in_sink(..., &mut StdoutSink::Terminal)`
wrapper — **behavior-identical**. The shared script body runs via the existing
`run_sourced_contents_in_sink` (the sink-aware variant). `run` uses the `bash -c`
form (push_main_frame=false); `run_file`/`run_script` use the script form (true).

## CLI dogfood

`crates/huck-cli/src/repl.rs`'s `run` keeps building the `Rc<RefCell<Shell>>` and
installing its signal handlers (process-global concerns the CLI owns). Its two
headless dispatch arms then route through `Engine::from_shell_cell(cell)`:

- `RunMode::Command { command, argv0, args }` (line ~73, was
  `run_program(..., false, ...)`) → set arg0/args on the engine, `engine.run(&command)`.
- `RunMode::File { path, args }` (line ~89, was `run_program(..., true, ...)`) →
  the CLI already read `contents`; route through a small internal
  `Engine::run_script(contents, arg0)` (script/main-frame semantics) that
  `run_file` also uses, so the already-read-contents path and the read-from-path
  path share one implementation.

The `RunMode::Interactive` REPL path is unchanged.

## Build / packaging

No new crates, no new external deps. `Engine` is additive in `huck-engine`; the
CLI's dependency on `huck-engine` already exists. `huck-engine` stays
rustyline-free (the facade touches no terminal code). Release binary + packaging
paths unchanged.

## Testing & verification

- **Unit tests** in `crates/huck-engine/src/engine.rs` `mod tests`:
  - `run`: exit codes (`true`→0, `false`→1, `exit 3`→3), multi-line script,
    a function defined then called, state persistence across two `run` calls
    (set a var in call 1, read it in call 2), a parse error → 2.
  - `capture`: stdout captured (`echo hi`→`"hi\n"`), exit code, that a builtin's
    stdout is captured, and that state persists.
  - `run_file`: a tempfile script runs; a missing path → 127.
  - `var`/`set_var`/`set_args` (`$1`..)/`set_arg0` (`$0`)/`last_status`.
  - `builder` (`env`/`arg0`/`args` then `build`).
- **Doc example** on `Engine` (rustdoc, exercised by `cargo test --doc`) showing
  the Section-1 usage (`new` → `set_var` → `run` → `capture` → `var`).
- **CLI byte-identical gate** (the dogfood must change NOTHING observable):
  - All `tests/*.rs` integration tests (drive the binary) green.
  - All `tests/scripts/*_diff_check.sh` harnesses green — especially any
    `-c`/script + `FUNCNAME`/`$0` cases (the `run`-vs-`run_file` main-frame
    distinction must reproduce the old `-c` (false) / script (true) behavior).
  - `cargo test --workspace` count == the pre-change baseline **plus only the new
    Engine tests** — no existing test lost or changed.
  - `cargo clippy --all-targets` clean; release binary builds; headless
    `huck -c '…'` and `huck script.sh` behave exactly as before.

## Risks & mitigations

- **`run` (`-c`) vs `run_file` (script) main-frame parity.** The whole dogfood
  hinges on `run` reproducing `push_main_frame=false` and `run_file`/`run_script`
  reproducing `=true`. Mitigate: map each CLI arm to the matching method; verify
  with a FUNCNAME-at-top-level test for both `-c` and script (compare against
  bash) before and after.
- **`set_var` semantics.** Must be a plain global assignment (not an
  export/special); use `Shell::set`. Verify a set var is visible to `run`.
- **`capture` stderr.** Intentionally inherits (not captured) this cut; documented
  on the method. A `stderr`-capturing variant is a future add (out of scope).
- **Borrow discipline.** `Engine` holds `Rc<RefCell<Shell>>`; methods `borrow_mut`
  for the duration of a call only (no overlapping borrows across a re-entrant
  `$(…)`), matching `run_program`'s existing pattern.

## Out of scope

- Builder knobs beyond env/arg0/args (cwd, custom sinks, restricted/no-exec,
  timeout, custom builtins, stdin redirection).
- Separate stderr capture; a streaming/incremental output API.
- Splitting the `Shell` god-object; a stable semver public surface / crates.io
  publish.
- Any behavior change to the language or the CLI beyond routing through `Engine`.

## Task decomposition (for the plan)

1. Add `run_program_in_sink` (+ make `run_program` a thin wrapper); confirm
   behavior-identical (suite green).
2. Add `engine.rs`: `Engine`/`Output`/`EngineBuilder` + `new`/`from_shell_cell`/
   `run`/`capture`/`run_file`/`run_script`/var-access/builder; re-export at the
   crate root; unit tests + the doc example.
3. Dogfood: route `repl.rs`'s two headless arms through `Engine`; keep signals in
   the CLI; verify byte-identical CLI behavior.
4. Verify: equal-baseline-plus-new `cargo test --workspace`, harnesses, clippy,
   release binary, `-c`/script smoke; brief `docs/architecture.md` note that
   `huck_engine::Engine` is the embedding entry point.
