# v204: `Engine` embedding API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `huck_engine::Engine` facade (run/capture/run_file a script, get/set vars + args, persistent state) and route the CLI's headless path through it.

**Architecture:** Add a sink-parameterized `run_program_in_sink` (so `run`/`capture` share one path). Add `crates/huck-engine/src/engine.rs` with `Engine` owning an `Rc<RefCell<Shell>>`, exit-code semantics, stdout capture. Dogfood: the `huck-cli` headless arms call `Engine` instead of `run_program`. Pure-additive + a behavior-identical refactor.

**Tech Stack:** Rust (edition 2024), the existing huck-engine crate.

**Spec:** `docs/superpowers/specs/2026-06-21-engine-embedding-api-design.md`

**Branch:** `v204-engine-api`

**Key facts (verified; line numbers may drift — grep to confirm):**
- `crates/huck-engine/src/shell.rs`: `pub fn run_program(contents, argv0: Option<String>, args: Vec<String>, label: &str, push_main_frame: bool, shell_cell: &Rc<RefCell<Shell>>) -> i32` (~line 183). Its body sets `shell.is_interactive=false`, sets `shell.shell_argv0` (if `argv0` is `Some`) and `shell.positional_args = args`, optionally pushes a `main` `Frame`, calls `crate::builtins::run_sourced_contents(contents, Path::new(label), &mut shell)`, pops the frame, maps the `ExecOutcome` to an exit code, fires the EXIT trap + `hangup_jobs`, returns the code. `use crate::builtins::ExecOutcome;` is at the top.
- `crates/huck-engine/src/builtins.rs`: `pub(crate) fn run_sourced_contents_in_sink(contents: &str, path: &Path, shell: &mut Shell, sink: &mut crate::executor::StdoutSink) -> ExecOutcome` (~6035) and the `pub(crate) fn run_sourced_contents(...)` Terminal wrapper (~6255).
- `crates/huck-engine/src/executor.rs`: `pub enum StdoutSink<'a> { Terminal, Capture(&'a mut Vec<u8>) }`.
- `crates/huck-engine/src/shell_state.rs`: `Shell::new()`, `pub fn lookup_var(&self, &str) -> Option<String>`, `pub fn set(&mut self, name: &str, value: String)`, `pub fn last_status(&self) -> i32`, pub fields `positional_args: Vec<String>` and `shell_argv0: String`.
- `crates/huck-engine/src/lib.rs` declares `pub mod <name>;` for each module + re-exports `huck_syntax`.
- `crates/huck-cli/src/repl.rs`: the headless dispatch in `run` — `RunMode::Command` arm (~line 73) calls `run_program(&command, argv0, args, &label, false, &shell_cell)`; `RunMode::File` arm (~line 89) calls `run_program(&contents, Some(label.clone()), args, &label, true, &shell_cell)`. Signal handlers are installed earlier (~58-59).
- **Baseline** (capture FIRST): `cargo test --workspace 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print s}'` — record as `BASELINE` (~3463). Post-change MUST equal `BASELINE + <number of new Engine tests>` (no existing test lost).

---

## Task 1: `run_program_in_sink` (sink-parameterized; behavior-identical)

**Files:** Modify `crates/huck-engine/src/shell.rs` (`run_program`).

- [ ] **Step 1: Capture the baseline.** Run the BASELINE command above; record the number.

- [ ] **Step 2: Refactor `run_program` into `run_program_in_sink` + a thin wrapper.** Replace the existing `pub fn run_program(...) -> i32 { ... }` body so the work moves into a sink-taking function and `run_program` delegates with a `Terminal` sink:

```rust
/// Run a program/script `contents` against `shell_cell`, sending stdout to `sink`.
/// `run_program` is the `Terminal`-sink wrapper; the engine's `capture` passes a
/// `Capture` sink. Behavior with `Terminal` is identical to the old `run_program`.
pub fn run_program_in_sink(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    sink: &mut crate::executor::StdoutSink,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut shell = shell_cell.borrow_mut();
    shell.is_interactive = false;
    if let Some(a0) = argv0 {
        shell.shell_argv0 = a0;
    }
    shell.positional_args = args;

    if push_main_frame {
        shell.call_stack.push(crate::shell_state::Frame {
            funcname: "main".to_string(),
            source: label.to_string(),
            call_line: 0,
            kind: crate::shell_state::FrameKind::Main,
        });
        shell.sync_call_arrays();
    }

    let outcome = crate::builtins::run_sourced_contents_in_sink(
        contents,
        std::path::Path::new(label),
        &mut shell,
        sink,
    );

    if push_main_frame {
        shell.call_stack.pop();
        shell.sync_call_arrays();
    }

    let code = match outcome {
        ExecOutcome::Exit(n) => n,
        ExecOutcome::FunctionReturn(n) => n,
        ExecOutcome::Continue(s) => shell.take_pending_fatal_pe_error().unwrap_or(s),
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::Interrupted => 130,
    };
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    code
}

/// Run a program/script with stdout going to the terminal (the default).
pub fn run_program(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut sink = crate::executor::StdoutSink::Terminal;
    run_program_in_sink(contents, argv0, args, label, push_main_frame, &mut sink, shell_cell)
}
```
(This is a verbatim hoist of the old body into `run_program_in_sink`, swapping `run_sourced_contents` → `run_sourced_contents_in_sink(..., sink)`. NO logic change.)

- [ ] **Step 3: Build + full suite (proves behavior-identical).**

Run:
```bash
cargo build 2>&1 | tail -1
cargo test --workspace 2>&1 | grep -E "test result:" | grep -v "0 failed" || echo "ALL GREEN"
```
Expected: `Finished`; `ALL GREEN` (no test changed — `run_program` behaves exactly as before).

- [ ] **Step 4: Commit.**
```bash
git add crates/huck-engine/src/shell.rs
git commit -m "$(cat <<'EOF'
v204 task 1: run_program_in_sink (sink-parameterized headless runner)

Hoist run_program's body into run_program_in_sink(..., sink); run_program is now
a thin Terminal-sink wrapper. Behavior-identical (uses run_sourced_contents_in_sink
instead of run_sourced_contents). Lets the Engine capture stdout.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: The `Engine` facade + tests + doc example

**Files:** Create `crates/huck-engine/src/engine.rs`; Modify `crates/huck-engine/src/lib.rs`.

- [ ] **Step 1: Write `crates/huck-engine/src/engine.rs`:**

```rust
//! `Engine` — the embedding entry point for `huck-engine`.
//!
//! Owns a persistent shell session; run/capture script strings, run files, and
//! get/set variables and positional parameters. Shells signal failure via exit
//! codes, so these methods return exit codes (no `Result`): a parse error is
//! exit 2 (+ a message on stderr), a missing file is 127.
//!
//! ```
//! use huck_engine::Engine;
//! let mut e = Engine::new();
//! e.set_var("NAME", "world");
//! assert_eq!(e.run("echo \"hi $NAME\""), 0);          // prints: hi world
//! let out = e.capture("echo $((6 * 7))");
//! assert_eq!(out.stdout, "42\n");
//! assert_eq!(out.exit_code, 0);
//! assert_eq!(e.var("NAME").as_deref(), Some("world"));
//! ```
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::executor::StdoutSink;
use crate::shell_state::Shell;

/// The captured result of [`Engine::capture`].
#[derive(Debug, Clone)]
pub struct Output {
    /// Everything the script wrote to stdout (stderr inherits the process).
    pub stdout: String,
    /// The script's exit status.
    pub exit_code: i32,
}

/// A persistent, embeddable huck shell session.
pub struct Engine {
    cell: Rc<RefCell<Shell>>,
}

impl Engine {
    /// A fresh session (`$0` = "huck"). Installs no signal handlers, reads no rc file.
    pub fn new() -> Self {
        Engine { cell: Rc::new(RefCell::new(Shell::new())) }
    }

    /// Start building a configured engine.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Wrap a caller-owned (possibly pre-configured) shell cell. The caller keeps
    /// ownership of any process-global concerns (e.g. signal handlers).
    pub fn from_shell_cell(cell: Rc<RefCell<Shell>>) -> Self {
        Engine { cell }
    }

    /// Run a script string with `bash -c` semantics (no "main" call frame).
    /// stdout + stderr inherit the process. Returns the exit status.
    pub fn run(&mut self, src: &str) -> i32 {
        let mut sink = StdoutSink::Terminal;
        self.run_with(src, false, &mut sink)
    }

    /// Run a script string, capturing stdout (stderr still inherits). `bash -c`
    /// semantics; returns `{ stdout, exit_code }`.
    pub fn capture(&mut self, src: &str) -> Output {
        let mut buf: Vec<u8> = Vec::new();
        let exit_code = {
            let mut sink = StdoutSink::Capture(&mut buf);
            self.run_with(src, false, &mut sink)
        };
        Output { stdout: String::from_utf8_lossy(&buf).into_owned(), exit_code }
    }

    /// Run a script STRING with script semantics (a "main" frame; `$0` = `arg0`).
    pub fn run_script(&mut self, src: &str, arg0: &str) -> i32 {
        self.cell.borrow_mut().shell_argv0 = arg0.to_string();
        let mut sink = StdoutSink::Terminal;
        self.run_with_label(src, arg0, true, &mut sink)
    }

    /// Read and run a script FILE with script semantics (`$0` = the path).
    /// A read failure prints `huck: <path>: <err>` and returns 127.
    pub fn run_file(&mut self, path: &Path) -> i32 {
        match std::fs::read_to_string(path) {
            Ok(contents) => self.run_script(&contents, &path.display().to_string()),
            Err(e) => {
                eprintln!("huck: {}: {}", path.display(), e);
                127
            }
        }
    }

    /// Read a shell variable.
    pub fn var(&self, name: &str) -> Option<String> {
        self.cell.borrow().lookup_var(name)
    }

    /// Set a (global) shell variable.
    pub fn set_var(&mut self, name: &str, value: &str) {
        self.cell.borrow_mut().set(name, value.to_string());
    }

    /// Set the positional parameters `$1`..`$N`.
    pub fn set_args(&mut self, args: Vec<String>) {
        self.cell.borrow_mut().positional_args = args;
    }

    /// Set `$0` (the program/script name).
    pub fn set_arg0(&mut self, name: &str) {
        self.cell.borrow_mut().shell_argv0 = name.to_string();
    }

    /// `$?` after the last run.
    pub fn last_status(&self) -> i32 {
        self.cell.borrow().last_status()
    }

    /// Access the underlying shell cell (advanced/dogfood use).
    pub fn shell_cell(&self) -> &Rc<RefCell<Shell>> {
        &self.cell
    }

    fn run_with(&mut self, src: &str, push_main_frame: bool, sink: &mut StdoutSink) -> i32 {
        let label = self.cell.borrow().shell_argv0.clone();
        self.run_with_label(src, &label, push_main_frame, sink)
    }

    fn run_with_label(
        &mut self,
        src: &str,
        label: &str,
        push_main_frame: bool,
        sink: &mut StdoutSink,
    ) -> i32 {
        // Preserve the shell's current $0 + positionals (don't clobber them).
        let args = self.cell.borrow().positional_args.clone();
        crate::shell::run_program_in_sink(src, None, args, label, push_main_frame, sink, &self.cell)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

/// Builder for a configured [`Engine`].
#[derive(Default)]
pub struct EngineBuilder {
    arg0: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl EngineBuilder {
    /// Seed a shell variable.
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
    /// Set `$0`.
    pub fn arg0(mut self, name: &str) -> Self {
        self.arg0 = Some(name.to_string());
        self
    }
    /// Set the positional parameters.
    pub fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
    /// Build the engine.
    pub fn build(self) -> Engine {
        let mut e = Engine::new();
        if let Some(a0) = self.arg0 {
            e.set_arg0(&a0);
        }
        e.set_args(self.args);
        for (k, v) in self.env {
            e.set_var(&k, &v);
        }
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_exit_codes() {
        let mut e = Engine::new();
        assert_eq!(e.run("true"), 0);
        assert_eq!(e.run("false"), 1);
        assert_eq!(e.run("exit 3"), 3);
    }

    #[test]
    fn run_multiline_script_and_function() {
        let mut e = Engine::new();
        let code = e.run("greet() { echo \"hi $1\"; }\ngreet there\n");
        assert_eq!(code, 0);
    }

    #[test]
    fn state_persists_across_runs() {
        let mut e = Engine::new();
        assert_eq!(e.run("x=5"), 0);
        let out = e.capture("echo $((x * 2))");
        assert_eq!(out.stdout, "10\n");
    }

    #[test]
    fn capture_collects_stdout_and_code() {
        let mut e = Engine::new();
        let out = e.capture("echo hi; echo bye; exit 4");
        assert_eq!(out.stdout, "hi\nbye\n");
        assert_eq!(out.exit_code, 4);
    }

    #[test]
    fn parse_error_is_exit_2() {
        let mut e = Engine::new();
        // unterminated `if` — bash exits 2 on a syntax error.
        assert_eq!(e.run("if ["), 2);
    }

    #[test]
    fn var_get_set_and_args() {
        let mut e = Engine::new();
        e.set_var("NAME", "world");
        assert_eq!(e.var("NAME").as_deref(), Some("world"));
        e.set_args(vec!["a".to_string(), "b".to_string()]);
        let out = e.capture("echo \"$1-$2-$#\"");
        assert_eq!(out.stdout, "a-b-2\n");
    }

    #[test]
    fn set_arg0_visible_as_dollar_zero() {
        let mut e = Engine::new();
        e.set_arg0("myprog");
        let out = e.capture("echo $0");
        assert_eq!(out.stdout, "myprog\n");
    }

    #[test]
    fn last_status_reflects_last_run() {
        let mut e = Engine::new();
        e.run("exit 7");
        assert_eq!(e.last_status(), 7);
    }

    #[test]
    fn run_file_runs_and_missing_is_127() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "echo from-file").unwrap();
        let mut e = Engine::new();
        assert_eq!(e.run_file(f.path()), 0);
        assert_eq!(e.run_file(Path::new("/no/such/huck/script.sh")), 127);
    }

    #[test]
    fn builder_configures_engine() {
        let mut e = Engine::builder()
            .arg0("prog")
            .args(vec!["x".to_string()])
            .env("GREETING", "yo")
            .build();
        let out = e.capture("echo \"$GREETING $0 $1\"");
        assert_eq!(out.stdout, "yo prog x\n");
    }
}
```

- [ ] **Step 2: Re-export in `crates/huck-engine/src/lib.rs`.** Add `pub mod engine;` (alphabetically with the other `pub mod` lines) and, after the module list, add:
```rust
pub use engine::{Engine, EngineBuilder, Output};
```

- [ ] **Step 3: Run the engine tests + the doc test.**
```bash
cargo test -p huck-engine engine:: 2>&1 | grep "test result:"
cargo test -p huck-engine --doc 2>&1 | grep "test result:"
```
Expected: both PASS. If `parse_error_is_exit_2` fails, check what huck actually returns for `if [` (adjust the test to the real bash-matching code only if huck≠bash is a PRE-EXISTING divergence — verify with `bash -c 'if ['; echo $?`).

- [ ] **Step 4: Full suite + clippy.**
```bash
cargo test --workspace 2>&1 | grep -E "test result:" | grep -v "0 failed" || echo "ALL GREEN"
cargo clippy --all-targets 2>&1 | grep -cE "^warning|^error" | xargs -I{} echo "clippy: {}"
```
Expected: `ALL GREEN`; `clippy: 0`.

- [ ] **Step 5: Commit.**
```bash
git add crates/huck-engine/src/engine.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v204 task 2: huck_engine::Engine embedding facade

A persistent shell session: run(src)->i32 (bash -c semantics), capture(src)->
Output{stdout,exit_code}, run_script/run_file (script semantics, missing->127),
var/set_var/set_args/set_arg0/last_status, builder, from_shell_cell. Exit-code
model, stdout capture (stderr inherits). 10 unit tests + a doc example.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Dogfood — route the CLI's headless path through `Engine`

**Files:** Modify `crates/huck-cli/src/repl.rs`.

- [ ] **Step 1: Replace the two `run_program` call sites.** In `crates/huck-cli/src/repl.rs`'s `run`, the `RunMode::Command` arm (the `-c` case) currently reads (repl.rs ~69-74):
```rust
        RunMode::Command { command, argv0, args } => {
            let label = argv0
                .clone()
                .unwrap_or_else(|| shell_cell.borrow().shell_argv0.clone());
            return run_program(&command, argv0, args, &label, false, &shell_cell);
        }
```
Replace the WHOLE arm body with the following — the `let label = …` binding becomes dead, so DELETE it. `Engine::run` derives the label from the shell's `$0`, which `set_arg0` updates, reproducing the old `label = argv0.unwrap_or(shell_argv0)`:
```rust
        RunMode::Command { command, argv0, args } => {
            let mut engine = huck_engine::Engine::from_shell_cell(std::rc::Rc::clone(&shell_cell));
            if let Some(a0) = argv0 {
                engine.set_arg0(&a0);
            }
            engine.set_args(args);
            return engine.run(&command);
        }
```
The `RunMode::File` arm reads the file itself (keeping its 127-NotFound / 126-other distinction) and currently ends with (repl.rs ~88-89):
```rust
            let label = path.display().to_string();
            return run_program(&contents, Some(label.clone()), args, &label, true, &shell_cell);
```
Leave the file-read + error block ABOVE untouched; replace ONLY those two lines with:
```rust
            let label = path.display().to_string();
            let mut engine = huck_engine::Engine::from_shell_cell(std::rc::Rc::clone(&shell_cell));
            engine.set_args(args);
            return engine.run_script(&contents, &label);
```
(`engine.run` = `bash -c` semantics / no main frame, matching the old `false`; `engine.run_script` = script semantics / main frame + `$0`=path, matching the old `Some(label), …, true`. The CLI keeps owning the file read so its 126/127 distinction is preserved — it does NOT use `Engine::run_file`. The signal handlers installed earlier on `shell_cell`'s flags still apply — `Engine::from_shell_cell` shares the same cell.)

- [ ] **Step 2: Remove the now-unused `run_program` import.** `crates/huck-cli/src/repl.rs` imports `run_program` from `huck_engine::shell` (check the `use huck_engine::shell::{…}` line near the top). If `run_program` is no longer referenced in the file, remove it from that import list. Run `cargo build 2>&1 | grep -E "unused|warning" | head` and clear any unused-import warning this introduced.

- [ ] **Step 3: Build + the CLI byte-identical gate.**
```bash
cargo build --release 2>&1 | tail -1
# headless -c, with arg0 + positionals:
./target/release/huck -c 'echo "$0:$1:$((2+2))"' myprog A
# script mode:
printf 'echo "script $0 $1"\n' > /tmp/huck_v204.sh
./target/release/huck /tmp/huck_v204.sh X
# FUNCNAME parity (the main-frame distinction): -c vs script vs bash
echo "-c FUNCNAME:"; ./target/release/huck -c 'echo "[${FUNCNAME[@]}]"'; bash -c 'echo "[${FUNCNAME[@]}]"'
echo "script FUNCNAME:"; printf 'echo "[${FUNCNAME[@]}]"\n' > /tmp/huck_fn.sh; ./target/release/huck /tmp/huck_fn.sh; bash /tmp/huck_fn.sh
rm -f /tmp/huck_v204.sh /tmp/huck_fn.sh
```
Expected: `myprog:A:4`; `script /tmp/huck_v204.sh X`; the `-c` and script FUNCNAME lines must match what huck produced BEFORE this change (run the same probes on `git stash`'d / `main` binary if unsure) — the dogfood must not shift `-c` (no main frame) vs script (main frame) behavior.

- [ ] **Step 4: Full integration suite + harnesses + clippy (the real gate).**
```bash
cargo test --workspace 2>&1 | grep -E "test result:" | grep -v "0 failed" || echo "ALL GREEN"
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -iE "Fail: [1-9]|[1-9] failed" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | grep -cE "^warning|^error" | xargs -I{} echo "clippy: {}"
```
Expected: `ALL GREEN`; `ALL HARNESSES GREEN`; `clippy: 0`. A harness/integration regression here means the dogfood changed CLI behavior — investigate (most likely the `-c` vs script main-frame mapping).

- [ ] **Step 5: Commit.**
```bash
git add crates/huck-cli/src/repl.rs
git commit -m "$(cat <<'EOF'
v204 task 3: dogfood — CLI headless path runs through huck_engine::Engine

repl.rs's -c arm -> Engine::run (bash -c semantics); script arm -> Engine::run_script
(script semantics). Signals stay in the CLI (shared shell cell via from_shell_cell).
Byte-identical CLI behavior (integration + harnesses green).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Verify + docs

**Files:** Modify `docs/architecture.md`.

- [ ] **Step 1: Final equal-baseline-plus-new check.**
```bash
cargo test --workspace 2>&1 | grep -E "test result: ok\." | awk -F'[:.]' '{print $3}' | awk '{s+=$1} END {print "POST: " s}'
```
Expected: `POST` == `BASELINE` (Task 1 Step 1) **+ 10** (the new Engine unit tests) **+ 1** (the doc test, which `cargo test --doc` counts) — i.e. BASELINE + 11. (Confirm the delta is exactly the new tests; no existing test was lost.)

- [ ] **Step 2: `docs/architecture.md` note.** In the crate-graph section (the `huck-engine` bullet, added v203), append one sentence: the embedding entry point is `huck_engine::Engine` (`new`/`builder` → `run`/`capture`/`run_file` + `var`/`set_var`/`set_args`), and the `huck` binary's headless `-c`/script path runs through it (`run` = `bash -c`, `run_file`/`run_script` = script semantics).

- [ ] **Step 3: Commit.**
```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v204 task 4: document huck_engine::Engine as the embedding entry point

cargo test --workspace == baseline + the new Engine tests; harnesses + clippy
green; CLI dogfooded. Note Engine in architecture.md.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Report-back (Task 4)

Report: STATUS, the commit SHAs, BASELINE vs POST test counts (POST should be BASELINE+11), the full-suite + harness + clippy results, the Task-3 CLI smoke outputs (`-c` arg0/positional + script + the FUNCNAME parity lines), and confirmation `cargo tree -p huck-engine` still has 0 rustyline (the Engine is terminal-free). Flag any harness/integration regression and how it was resolved.
