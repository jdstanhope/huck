# v205 `Engine` stderr capture + stdin feed — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add stderr capture + stdin feed to `huck_engine::Engine` so a test/diagnostic host can drive a script's input and observe both output streams.

**Architecture:** Two pieces of plumbing. (1) Add `StderrSink::{Terminal, Merged, Capture}` symmetric to `StdoutSink`; thread `err: &mut dyn Write` through builtin dispatch + diagnostic call sites so direct `eprintln!`-style writes are routed via the active sink. (2) Add a stdin pipe helper that dup2's a CLOEXEC pipe onto fd 0 for the duration of a call (with RAII save/restore). Wrap both behind a per-call `ExecBuilder` returned from `Engine::exec(src)`; preserve v204's `run(src)` / `capture(src)` shortcuts.

**Tech Stack:** Rust 2021, `nix` / direct `libc` for pipe / dup2 (already in deps via `huck-engine`), `std::thread` for the stdin writer path. No new crate deps.

**Branch:** Implement on `v205-engine-stderr-stdin`. Each task ends with a commit and a green suite run.

**Audit scale note:** The builtin-stderr audit (tasks 3–5) touches ~400 direct stderr writes across ~9 files. The mechanical work is replacing `eprintln!("huck: …")` with `e!(err, "huck: …")` (via the macro from Task 2) and threading `err: &mut dyn Write` to callers that lack it. Subagents handle one file group per task to keep context manageable.

**Spec:** `docs/superpowers/specs/2026-06-22-engine-stderr-stdin-design.md`.

---

## File structure

**Modify:**
- `crates/huck-engine/src/executor.rs` — add `StderrSink`; thread `err` through `run_command`/`run_*`/`run_simple_builtin_command`/etc.; external-process stderr pipe + merged-via-dup2; convert direct stderr writes.
- `crates/huck-engine/src/shell.rs` — generalize `run_program_in_sink` → `run_program_in_sinks`; same for `process_line_in_sink`. Convert direct stderr writes (8 sites).
- `crates/huck-engine/src/builtins.rs` — add `err: &mut dyn Write` to every builtin function + `run_builtin` / `run_declaration_builtin` dispatch; convert ~223 direct stderr writes.
- `crates/huck-engine/src/expand.rs` — thread `err` to functions that emit errors; convert 22 stderr writes.
- `crates/huck-engine/src/completion_builtins.rs` — `err` parameter to `builtin_complete`/`compgen`/`compopt`; convert 15 writes.
- `crates/huck-engine/src/param_expansion.rs` — convert 6 writes.
- `crates/huck-engine/src/shell_state.rs` — convert 9 writes.
- `crates/huck-engine/src/history.rs` — convert 2 writes.
- `crates/huck-engine/src/jobs.rs` — convert 1 write.
- `crates/huck-engine/src/engine.rs` — add `stderr` field to `Output`; add `exec(src) -> ExecBuilder<'_>`; update `run` / `capture` to use the new sinks (Terminal for both); wire stdin helper.
- `crates/huck-engine/src/lib.rs` — re-export `ExecBuilder` and `StderrSink`.
- `docs/architecture.md` — brief note on `StderrSink` + `ExecBuilder`.
- `docs/bash-divergences.md` — DELETE the L-25 entry (resolved by the audit).
- `crates/huck-engine/src/macros.rs` — NEW, exports the `e!` macro.

**Create:**
- `crates/huck-engine/src/exec_builder.rs` — NEW, hosts `ExecBuilder<'_>` (kept separate from `engine.rs`).
- `crates/huck-engine/src/stdin_pipe.rs` — NEW, hosts `with_stdin_fd0` + the inline-vs-thread switch.
- `tests/scripts/engine_capture_diff_check.sh` — NEW bash-diff harness.
- `crates/huck-engine/tests/engine_capture_diff.rs` — NEW Rust binary that drives `Engine` for the harness.

---

## Task 1: Add `StderrSink` enum + the `e!` stderr macro

**Files:**
- Modify: `crates/huck-engine/src/executor.rs:23-26` (add enum next to `StdoutSink`)
- Create: `crates/huck-engine/src/macros.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `macros` mod first so the macro is visible crate-wide)

- [ ] **Step 1: Create the macros module**

Create `crates/huck-engine/src/macros.rs`:

```rust
//! Crate-local stderr macro. `e!(err, "huck: foo {}", x)` is the structured
//! analog of `eprintln!("huck: foo {}", x)`, except it writes to the threaded
//! `err: &mut dyn Write` so the active `StderrSink` (Terminal / Merged /
//! Capture) routes correctly. The write is fallible (ignored) because stderr
//! is best-effort and a write error here must not abort the shell.

#[macro_export]
macro_rules! e {
    ($err:expr, $($arg:tt)*) => {{
        let _ = ::std::io::Write::write_fmt($err, format_args!($($arg)*));
        let _ = ::std::io::Write::write_all($err, b"\n");
    }};
}
```

- [ ] **Step 2: Register the macro module FIRST in lib.rs**

Add `#[macro_use] mod macros;` near the top of `crates/huck-engine/src/lib.rs`, BEFORE any other `pub mod`. Position is critical — `macro_use` must precede modules that consume the macro.

Find the existing module declarations in `crates/huck-engine/src/lib.rs` and prepend:

```rust
#[macro_use]
mod macros;
```

- [ ] **Step 3: Add `StderrSink` enum in executor.rs**

After the existing `pub enum StdoutSink<'a>` definition at `executor.rs:23-26`, add:

```rust
/// Where the active "errored" output stream goes. Symmetric to `StdoutSink`,
/// except for the extra `Merged` variant which routes stderr writes through
/// the active stdout writer (the `2>&1` analog).
pub enum StderrSink<'a> {
    Terminal,
    Merged,
    Capture(&'a mut Vec<u8>),
}
```

- [ ] **Step 4: Build the workspace, run the suite**

```bash
cargo build --workspace
cargo test --workspace --quiet
```

Expected: green. Macro and enum are unused at this point — Rust may emit a `dead_code` warning on `StderrSink`; ignore. The `e!` macro is `macro_export` so the warning doesn't apply.

- [ ] **Step 5: Commit**

```bash
git checkout -b v205-engine-stderr-stdin
git add crates/huck-engine/src/macros.rs crates/huck-engine/src/executor.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v205 task 1: add StderrSink enum + e! stderr macro (no behavior change)

Adds `StderrSink::{Terminal, Merged, Capture}` symmetric to `StdoutSink`, plus
the `e!(err, "fmt", args)` macro that writes through a `&mut dyn Write` so the
active sink routes correctly. No call sites converted yet; this task is
type-and-tool plumbing only. Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Thread `err: &mut dyn Write` through builtin dispatch + convert builtins.rs

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (every `fn builtin_*` signature + `run_builtin` + `run_declaration_builtin` + `run_sourced_contents_in_sink` and the chain of internal helpers; convert ~223 `eprintln!`/`writeln!(io::stderr(), …)` sites)
- Modify: `crates/huck-engine/src/executor.rs:1106-1110, 1506` (the `run_builtin`/`run_declaration_builtin` call sites — pass `&mut io::stderr()` as `err` for now)

This is the largest task. The pattern is mechanical: add `err: &mut dyn Write` to every `fn builtin_NAME(...)` signature, every internal helper called from one, and every layer up to the dispatch. Convert direct stderr writes with the `e!` macro from Task 1. At every call site, pass `&mut io::stderr()` so behavior stays identical to today.

- [ ] **Step 1: Update the dispatch signature in builtins.rs**

Change `run_builtin` and `run_declaration_builtin` at `builtins.rs:69`, `:233`, `:275`:

```rust
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,    // NEW
    shell: &mut Shell,
) -> ExecOutcome { ... }

pub(crate) fn run_declaration_builtin_strs(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,    // NEW
    shell: &mut Shell,
) -> ExecOutcome { ... }

pub fn run_declaration_builtin(
    name: &str,
    decl_args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,    // NEW
    shell: &mut Shell,
) -> ExecOutcome { ... }
```

Similarly for `run_sourced_contents_in_sink` at `:6035`: add `err: &mut dyn Write` parameter.

In the dispatch match (`builtins.rs:86-142`), thread `err` to each arm — every `builtin_NAME(...)` call gets `err` inserted. Example:

```rust
match name {
    "cd" => builtin_cd(args, out, err, shell),
    "pwd" => builtin_pwd(args, out, err, shell),
    "echo" => builtin_echo(args, out, err),
    // ... etc for all arms ...
    "exec" => {
        e!(err, "huck: exec: not supported in this context");
        ExecOutcome::Continue(1)
    }
    // ... etc ...
}
```

- [ ] **Step 2: Add `err: &mut dyn Write` to every `fn builtin_*` signature in builtins.rs**

Sweep the file. For every function whose name begins with `builtin_` and that today takes `out: &mut dyn Write`, add `err: &mut dyn Write` as the parameter immediately after `out`. For builtins that today take NO `out` parameter (e.g. `builtin_exit`, `builtin_unset`, `builtin_shift`, `builtin_break`, `builtin_continue`, `builtin_return`, `builtin_colon`, `builtin_true`, `builtin_false`, `builtin_source`, `builtin_eval`, `builtin_let`, `builtin_test`, `builtin_unalias`, `builtin_disown`, `builtin_fg`, `builtin_getopts`, `builtin_mapfile`): add `err: &mut dyn Write` immediately before `shell: &mut Shell`.

Use `grep -n '^fn builtin_\|^pub fn builtin_\|^pub(crate) fn builtin_' crates/huck-engine/src/builtins.rs` to locate them all.

For internal helpers that today emit stderr (typically named `parse_*`, `check_*`, `validate_*`, `print_*_usage` etc.) — add `err: &mut dyn Write` and thread `err` to them from the calling builtin.

- [ ] **Step 3: Convert every direct stderr write in builtins.rs to `e!(err, …)`**

Use `grep -n 'eprintln!\|writeln!(io::stderr\|writeln!(std::io::stderr\|io::stderr()\.write\|stderr().write_all' crates/huck-engine/src/builtins.rs` to enumerate ~223 sites. Replace each:

- `eprintln!("huck: foo {}", x)` → `e!(err, "huck: foo {}", x)`
- `writeln!(io::stderr(), "huck: foo {}", x).ok()` → `e!(err, "huck: foo {}", x)`
- `io::stderr().write_all(b"…").ok()` → `let _ = err.write_all(b"…");`

A few sites print to stderr WITHOUT a trailing newline (`eprint!("…")` or a manual write of bytes without `\n`); for those use `let _ = write!(err, "…")` so the macro's auto-newline doesn't shift output.

- [ ] **Step 4: Update the executor's call sites at `executor.rs:1106-1110, 1506`**

At the `let run = …` closure around `executor.rs:1106`:

```rust
let run = |out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell| {
    if let Some(da) = resolved.decl_args.as_deref() {
        builtins::run_declaration_builtin(&resolved.program, da, out, err, shell)
    } else {
        builtins::run_builtin(&resolved.program, &resolved.args, out, err, shell)
    }
};
```

Then in the existing `if write_to_fd1 { … } else { … }` block, also build an `err` writer. For Task 2 we keep behavior identical: always pass `&mut io::stderr()` as `err`. Both branches:

```rust
let outcome = if write_to_fd1 {
    let mut out = io::stdout();
    let mut err = io::stderr();
    run(&mut out, &mut err, shell)
} else {
    match sink {
        StdoutSink::Terminal => unreachable!("Terminal handled by write_to_fd1"),
        StdoutSink::Capture(buf) => {
            let mut err = io::stderr();
            run(*buf, &mut err, shell)
        }
    }
};
```

At `executor.rs:1506` (the `read` call site):

```rust
let mut err = io::stderr();
crate::builtins::run_builtin("read", &[], &mut devnull, &mut err, shell)
```

- [ ] **Step 5: Build and run the suite**

```bash
cargo build --workspace
cargo test --workspace --quiet
```

Expected: green. No behavior change: every `err` is still `io::stderr()`.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v205 task 2: thread err: &mut dyn Write through builtins.rs (no behavior change)

Every builtin function gains an `err: &mut dyn Write` parameter; ~223 direct
stderr writes converted to `e!(err, ...)`. Dispatch (`run_builtin`,
`run_declaration_builtin`) and the executor's call sites pass `&mut io::stderr()`
so behavior stays identical to v204. Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Convert executor.rs's direct stderr writes

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (114 stderr writes)

Most executor.rs stderr writes live inside `run_*` functions that already take `sink: &mut StdoutSink`. Thread a `err_sink: &mut StderrSink` parameter alongside it, and at each `eprintln!` use `e!(err, …)` where `err` is materialized from `err_sink`. To keep step size manageable, this task does executor.rs ONLY; other files in Task 4.

- [ ] **Step 1: Add `err_sink: &mut StderrSink` to the public entry points**

Add to `execute_with_sink` (`executor.rs:76`), `execute` (`:122`), `execute_capturing` (`:132`). For `execute` and `execute_capturing` (which build the sinks internally), add a `StderrSink::Terminal` for `execute` and `StderrSink::Terminal` for `execute_capturing` (the in-memory in-process stderr of $() substitution is not in scope for v205 except as a side-effect resolution of L-25 in Task 7; for Task 3 it stays Terminal).

- [ ] **Step 2: Thread through every `fn run_*` that today takes `&mut StdoutSink`**

`run_command`, `run_pipeline`, `run_andor_group`, `run_simple_builtin_command`, `run_if`, `run_for`, `run_while`, `run_case`, `run_select`, `run_redirected`, `run_arith_command`, `run_test_command`, `run_subshell`, etc. — add `err_sink: &mut StderrSink` next to `sink: &mut StdoutSink`. Use grep to locate:

```bash
grep -n 'sink: &mut StdoutSink' crates/huck-engine/src/executor.rs
```

Each site gets the err sink parameter added.

- [ ] **Step 3: Materialize a writer at the leaf where `e!` is called**

At each `eprintln!`-call site inside `executor.rs`, materialize the writer from the sink and use `e!`:

```rust
// helper at top of executor.rs:
fn err_writer<'a>(err_sink: &'a mut StderrSink<'_>, out_sink: &'a mut StdoutSink<'_>)
    -> Box<dyn std::io::Write + 'a>
{
    match err_sink {
        StderrSink::Terminal => Box::new(std::io::stderr()),
        StderrSink::Capture(buf) => Box::new(&mut **buf),
        StderrSink::Merged => match out_sink {
            StdoutSink::Terminal => Box::new(std::io::stdout()),
            StdoutSink::Capture(buf) => Box::new(&mut **buf),
        },
    }
}
```

Then the call-site pattern becomes:

```rust
{
    let mut err = err_writer(err_sink, sink);
    e!(&mut *err, "huck: {}: {}", path, msg);
}
```

The brace scope releases the borrows on `err_sink` / `sink` before subsequent code.

- [ ] **Step 4: Update the closure at `executor.rs:1106-1110` (Task 2 patched this; finish here)**

Now that internal `err_sink` is threaded, the closure can build the `err` writer from the active `err_sink` rather than always `io::stderr()`:

```rust
let outcome = if write_to_fd1 {
    let mut out = io::stdout();
    let mut err = err_writer(err_sink, sink);
    run(&mut out, &mut *err, shell)
} else {
    match sink {
        StdoutSink::Terminal => unreachable!("Terminal handled by write_to_fd1"),
        StdoutSink::Capture(buf) => {
            let mut err = err_writer(err_sink, &mut StdoutSink::Capture(buf));
            run(*buf, &mut *err, shell)
        }
    }
};
```

(The exact borrow shape may need a small refactor — the writer's lifetime must overlap the `run(...)` call. Adjust as needed.)

- [ ] **Step 5: Convert the 114 direct stderr writes**

Use:

```bash
grep -n 'eprintln!\|writeln!(io::stderr\|writeln!(std::io::stderr' crates/huck-engine/src/executor.rs
```

Each site converts to the `let mut err = err_writer(err_sink, sink); e!(&mut *err, "…")` pattern. Top-level helpers that don't have a sink in scope (e.g. `install_sigint_handler`, `install_sigchld_handler`, `install_job_control_signals`, `flush_stdout`'s sister diagnostics) STAY as `eprintln!` — they run at process startup, no sink exists yet.

- [ ] **Step 6: Build and run the suite**

```bash
cargo build --workspace
cargo test --workspace --quiet
```

Expected: green. All sinks still Terminal at every call site so output goes to fd 2 as before.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v205 task 3: thread StderrSink through executor.rs (no behavior change)

Adds err_sink: &mut StderrSink alongside the existing sink: &mut StdoutSink on
every run_* function. Introduces err_writer() helper that materializes a
&mut dyn Write from the active sink pair (Terminal -> stderr, Capture -> buf,
Merged -> active stdout writer). Converts 114 direct stderr writes to the
e!(err, ...) macro. All callers pass StderrSink::Terminal so behavior is
identical to v204. Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Convert the remaining files' direct stderr writes

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (22 sites)
- Modify: `crates/huck-engine/src/completion_builtins.rs` (15 sites)
- Modify: `crates/huck-engine/src/shell_state.rs` (9 sites)
- Modify: `crates/huck-engine/src/shell.rs` (8 sites)
- Modify: `crates/huck-engine/src/param_expansion.rs` (6 sites)
- Modify: `crates/huck-engine/src/history.rs` (2 sites)
- Modify: `crates/huck-engine/src/jobs.rs` (1 site)

For each file, thread `err: &mut dyn Write` through the call chain from the executor entry points. Many of these (e.g. `expand.rs::expand`, `param_expansion.rs::expand_modifier_with_value`) are called from `executor.rs` paths — those callers will pass the `err` writer materialized via `err_writer(err_sink, sink)` from Task 3.

- [ ] **Step 1: shell.rs — generalize `run_program_in_sink` → `run_program_in_sinks`**

Rename and extend the signature at `shell.rs:187`:

```rust
pub fn run_program_in_sinks(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 { ... body unchanged, but pass err_sink to run_sourced_contents_in_sinks ... }
```

Keep `run_program_in_sink` as a thin wrapper (kept for v204 callers — only `engine.rs` currently, fixed in Task 7):

```rust
pub fn run_program_in_sink(
    contents: &str, argv0: Option<String>, args: Vec<String>,
    label: &str, push_main_frame: bool, sink: &mut crate::executor::StdoutSink,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut err_sink = crate::executor::StderrSink::Terminal;
    run_program_in_sinks(contents, argv0, args, label, push_main_frame, sink, &mut err_sink, shell_cell)
}
```

Same treatment for `process_line_in_sink` (`shell.rs:334`) → `process_line_in_sinks`. Inside `run_program_in_sinks` the call to `builtins::run_sourced_contents_in_sink` needs a renamed `run_sourced_contents_in_sinks` that accepts `err_sink` — rename and extend in `builtins.rs` (the function was updated in Task 2 to accept `err: &mut dyn Write`; here the signature is updated again to accept `err_sink: &mut StderrSink` because it's at the executor-boundary level — convert via `err_writer`).

The 8 direct stderr writes in shell.rs (`eprintln!("huck: syntax error: …")` etc.) use a local `let mut err = err_writer(err_sink, sink); e!(&mut *err, …)` block.

- [ ] **Step 2: expand.rs — thread err to error-emitting functions**

Use `grep -n 'eprintln!\|writeln!(io::stderr' crates/huck-engine/src/expand.rs` to find the 22 sites. Each is inside a function called (transitively) from `executor.rs::run_command` / `run_simple_builtin_command`. Add `err: &mut dyn Write` to those functions' signatures; the executor passes the materialized writer.

For the public `pub fn expand(...)` function — adding `err` is invasive (many call sites across the engine). Pragmatic alternative: store a `*mut dyn Write` keyed pointer in a `thread_local!` set by the executor before calling `expand`, consulted by the error sites. The pointer's lifetime is the synchronous `expand` call; safety is upheld by single-threaded execution.

Implement the thread-local model:

```rust
// expand.rs near the top:
thread_local! {
    static ERR_SINK_PTR: std::cell::Cell<Option<std::ptr::NonNull<dyn std::io::Write>>> =
        std::cell::Cell::new(None);
}

fn with_err<F: FnOnce(&mut dyn std::io::Write) -> R, R>(f: F) -> R {
    ERR_SINK_PTR.with(|c| {
        if let Some(mut p) = c.get() {
            // Safety: caller (`expand`) installed the pointer for the call duration.
            unsafe { f(p.as_mut()) }
        } else {
            f(&mut std::io::stderr())
        }
    })
}

pub fn install_err_sink<'a, F: FnOnce() -> R, R>(err: &'a mut dyn std::io::Write, f: F) -> R {
    let raw = std::ptr::NonNull::from(err as &mut dyn std::io::Write).cast::<dyn std::io::Write>();
    // SAFETY: we restore on scope exit; single-threaded by Engine contract.
    ERR_SINK_PTR.with(|c| c.set(Some(raw)));
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) { ERR_SINK_PTR.with(|c| c.set(None)); }
    }
    let _guard = Guard;
    f()
}
```

The executor wraps the `expand()` call: `expand::install_err_sink(&mut *err, || expand(...))`. The 22 sites in expand.rs use `with_err(|err| e!(err, "…"))`.

(The thread-local model avoids the cross-cutting threading work for deep call chains. Apply the same pattern to `param_expansion.rs` and `arith.rs`.)

- [ ] **Step 3: param_expansion.rs — same thread-local pattern**

Use the same `install_err_sink` / `with_err` helpers from expand.rs (re-export via `pub use expand::install_err_sink;` if needed, or duplicate). Convert the 6 sites.

- [ ] **Step 4: completion_builtins.rs — add `err` parameter (15 sites)**

Functions in `completion_builtins.rs` (`builtin_complete`, `builtin_compgen`, `builtin_compopt`, helpers) are called directly from `run_builtin` dispatch. Add `err: &mut dyn Write` to the signatures and thread through internal helpers; convert the 15 sites with `e!(err, …)`.

The dispatch in `builtins.rs::run_builtin` was already updated in Task 2 — verify the `complete`/`compgen`/`compopt` arms pass `err`.

- [ ] **Step 5: shell_state.rs (9 sites) — thread-local pattern**

`shell_state.rs` error sites are deep in `Shell` methods (`set`, `unset`, `set_indexed_element`, …) called from many places. Use the same thread-local pattern as expand.rs — `with_err(|err| e!(err, …))`. The executor sets the pointer for the duration of each `run_command` call (around the `run_builtin`/`run_declaration_builtin` dispatch).

- [ ] **Step 6: history.rs (2) + jobs.rs (1)**

History.rs and jobs.rs each have a handful of sites. Use the appropriate pattern (parameter for shallow, thread-local for deep). Most history.rs sites are called from `builtin_history` which already has `err` in scope (Task 2) — pass it through. Jobs.rs's one site is in a Shell method — use the thread-local pattern.

- [ ] **Step 7: Build and run the suite**

```bash
cargo build --workspace
cargo test --workspace --quiet
```

Expected: green. All sinks Terminal, all `with_err`-pointer absent so the leaf branch falls through to `io::stderr()` — behavior identical.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/{expand.rs,completion_builtins.rs,shell_state.rs,shell.rs,param_expansion.rs,history.rs,jobs.rs,builtins.rs}
git commit -m "$(cat <<'EOF'
v205 task 4: route remaining stderr writes through threaded err writer

Converts the remaining 63 direct stderr writes across expand.rs (22),
completion_builtins.rs (15), shell_state.rs (9), shell.rs (8), param_expansion.rs
(6), history.rs (2), jobs.rs (1) to the e!() macro. Shallow call chains (shell.rs,
completion_builtins.rs, history.rs) thread `err: &mut dyn Write` directly; deep
call chains (expand.rs, param_expansion.rs, shell_state.rs's Shell methods, the
single jobs.rs site) use a thread-local err-sink pointer installed by the
executor for the synchronous duration of each run_command. Renames
run_program_in_sink -> run_program_in_sinks and process_line_in_sink ->
process_line_in_sinks (old names kept as Terminal-passing wrappers). Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: External-process stderr — capture pipe + merged via dup2

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the external-process fork-and-exec paths)

Mirror the existing stdout-capture pipe pattern at `executor.rs:388-435` for stderr. For `StderrSink::Capture(buf)`, create a CLOEXEC pipe; child's fd 2 is the write end; parent drains the read end into `buf` after `waitpid`. For `StderrSink::Merged`, dup2 the existing stdout pipe's write end onto the child's fd 2 (kernel-level interleaving = bash byte order).

- [ ] **Step 1: Identify all external-process fork sites in executor.rs**

```bash
grep -n 'fork_and_run\|spawn_external\|run_exec_single\|fork(\|posix_spawn' crates/huck-engine/src/executor.rs
```

The primary sites: `run_exec_single` (the single-command path), `run_pipeline` (pipeline stages), `fork_and_run_in_subshell` (subshells), the background-job spawn paths.

- [ ] **Step 2: At each fork site, build stderr fd from `err_sink`**

Insert near the existing stdout pipe setup (mirroring `executor.rs:388-396`):

```rust
let (stderr_fd, capture_err_read_fd): (RawFd, Option<RawFd>) = match err_sink {
    StderrSink::Terminal => (libc::STDERR_FILENO, None),
    StderrSink::Merged => {
        // Reuse the stdout pipe write-end (already obtained above as `stdout_fd`).
        // For the inherited-stdout case (stdout_fd == STDOUT_FILENO), bash uses fd 1.
        (stdout_fd, None)
    }
    StderrSink::Capture(_) => match make_pipe() {
        Ok((r, w)) => (w, Some(r)),
        Err(e) => {
            eprintln!("huck: pipe: {e}");
            // Clean up the stdout pipe if we opened one.
            if let Some(r) = capture_read_fd { unsafe { libc::close(r); } }
            if stdout_fd != libc::STDOUT_FILENO { unsafe { libc::close(stdout_fd); } }
            return ExecOutcome::Continue(1);
        }
    },
};
```

Pass `stderr_fd` to the existing `fork_and_run_in_subshell` (or the equivalent fork helper) instead of the hardcoded `libc::STDERR_FILENO`.

- [ ] **Step 3: After the parent closes its write ends, drain both pipes**

Mirror the existing stdout drain at `executor.rs:429-435`. If `capture_err_read_fd` is `Some`, drain that fd into the `StderrSink::Capture` buffer.

```rust
// Close parent's stderr write end so child is sole writer.
if matches!(err_sink, StderrSink::Capture(_))
    && stderr_fd != libc::STDERR_FILENO
{
    unsafe { libc::close(stderr_fd); }
}

// Drain both capture pipes BEFORE waitpid to avoid deadlock — a child that
// writes more than the pipe buffer (~64KB on Linux) will block in write(2)
// otherwise. Spawn a background drain for stderr; foreground-drain stdout
// (the most common case is a small writer; if either pipe could realistically
// fill, both must drain concurrently).
let err_drain = if let (Some(r), StderrSink::Capture(buf)) = (capture_err_read_fd, &mut *err_sink) {
    // Spawn a thread; pass the buffer ownership via a channel.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let handle = std::thread::spawn(move || {
        use std::os::fd::FromRawFd;
        let mut f = unsafe { File::from_raw_fd(r) };
        let mut local = Vec::new();
        let _ = io::copy(&mut f, &mut local);
        let _ = tx.send(local);
    });
    Some((handle, rx, buf))
} else {
    None
};

if let (Some(r), StdoutSink::Capture(buf)) = (capture_read_fd, &mut *sink) {
    use std::os::fd::FromRawFd;
    let mut f = unsafe { File::from_raw_fd(r) };
    let _ = io::copy(&mut f, *buf);
}

if let Some((handle, rx, buf)) = err_drain {
    let _ = handle.join();
    if let Ok(local) = rx.recv() {
        buf.extend_from_slice(&local);
    }
}
```

(The exact shape depends on the surrounding code's borrow structure. Adjust as needed to keep clippy happy.)

- [ ] **Step 4: Add a unit test for external-process stderr**

In `crates/huck-engine/src/executor.rs::mod tests` (or wherever the existing capture tests live), add:

```rust
#[test]
fn external_process_stderr_is_captured() {
    let mut buf_out: Vec<u8> = Vec::new();
    let mut buf_err: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    {
        let mut out = StdoutSink::Capture(&mut buf_out);
        let mut err = StderrSink::Capture(&mut buf_err);
        // Drive a small external command that writes to both fds.
        // Note: this test depends on /bin/sh being present; common on Linux/macOS.
        // (Use a single-command shell fragment; the executor's run_command path
        // forks /bin/sh -c.)
        let src = "/bin/sh -c 'echo out; echo err >&2'";
        execute_with_sink(
            &crate::lexer::tokenize_and_parse(src).unwrap(),
            &mut shell, src, &mut out, &mut err,
        );
    }
    assert_eq!(String::from_utf8_lossy(&buf_out), "out\n");
    assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
}
```

(Use the same parse helper the surrounding tests use; adjust to match.)

- [ ] **Step 5: Run the new test (expect failure first if external paths aren't wired) then make it pass**

```bash
cargo test --workspace external_process_stderr_is_captured -- --nocapture
```

Expected: PASS once Step 2 + Step 3 are complete.

- [ ] **Step 6: Add a merged-stderr test**

```rust
#[test]
fn external_process_merged_stderr_interleaves_via_kernel() {
    let mut buf: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    {
        let mut out = StdoutSink::Capture(&mut buf);
        let mut err = StderrSink::Merged;
        let src = "/bin/sh -c 'printf out; printf err 1>&2; printf out2'";
        execute_with_sink(
            &crate::lexer::tokenize_and_parse(src).unwrap(),
            &mut shell, src, &mut out, &mut err,
        );
    }
    // Both streams hit the same kernel pipe; ordering matches the writes.
    assert_eq!(String::from_utf8_lossy(&buf), "outerrout2");
}
```

Expected: PASS once Step 2's `StderrSink::Merged` branch dup2's stdout_fd onto fd 2 of the child.

- [ ] **Step 7: Run the suite end-to-end**

```bash
cargo test --workspace --quiet
```

Expected: green.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v205 task 5: external-process stderr capture + Merged via dup2

For StderrSink::Capture, fork sites create a CLOEXEC pipe whose write-end is the
child's fd 2; parent spawns a background drainer thread (concurrent with stdout
drain) to avoid PIPE_BUF deadlock and folds the bytes into the active buffer
after waitpid. For StderrSink::Merged, the existing stdout pipe write-end is
dup2'd onto the child's fd 2 (kernel-level interleaving matches bash 2>&1).
Two new unit tests cover the split-capture and merged paths. Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Stdin pipe helper (`with_stdin_fd0`)

**Files:**
- Create: `crates/huck-engine/src/stdin_pipe.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add `pub(crate) mod stdin_pipe;`)

- [ ] **Step 1: Create the stdin helper module**

```rust
//! Replace fd 0 with a pipe carrying caller-supplied bytes for the duration of
//! a single closure call, then restore the original fd 0.
//!
//! For short inputs (≤ INLINE_STDIN_THRESHOLD) the bytes are written inline
//! before swapping fd 0, so no thread is needed. For longer inputs a writer
//! thread feeds the pipe until the input is exhausted or the reader closes.
//!
//! Pre-Engine fd 0 is saved via `dup(0)` and restored via `dup2(saved, 0)` in
//! an RAII guard that runs even on panic.

use std::io::{self, Write};
use std::os::fd::RawFd;

const INLINE_STDIN_THRESHOLD: usize = 4096;

/// Runs `f` with fd 0 backed by `input`. fd 0 is restored to its pre-call
/// value on return (even on panic).
pub fn with_stdin_fd0<R>(input: &[u8], f: impl FnOnce() -> R) -> R {
    let (r, w) = match make_pipe() {
        Ok(pair) => pair,
        Err(e) => {
            // Hard-fail before any state change.
            eprintln!("huck: pipe: {e}");
            return f(); // run anyway with caller's fd 0; matches "best effort"
        }
    };

    let saved = unsafe { libc::dup(0) };
    if saved < 0 {
        let e = io::Error::last_os_error();
        eprintln!("huck: dup: {e}");
        unsafe { libc::close(r); libc::close(w); }
        return f();
    }

    if unsafe { libc::dup2(r, 0) } < 0 {
        let e = io::Error::last_os_error();
        eprintln!("huck: dup2: {e}");
        unsafe { libc::close(r); libc::close(w); libc::close(saved); }
        return f();
    }
    unsafe { libc::close(r); }

    struct Restore { saved: RawFd }
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = io::stdout().flush();
            unsafe { libc::dup2(self.saved, 0); libc::close(self.saved); }
        }
    }
    let _restore = Restore { saved };

    if input.len() <= INLINE_STDIN_THRESHOLD {
        // Write inline, close, then run.
        let written = unsafe { libc::write(w, input.as_ptr().cast(), input.len()) };
        let _ = written; // best-effort; partial writes within a pipe ≤ PIPE_BUF are atomic
        unsafe { libc::close(w); }
        f()
    } else {
        // Spawn a writer thread that owns `w` and exits when it's closed by EPIPE
        // or by completing the write.
        let input_owned: Vec<u8> = input.to_vec();
        let handle = std::thread::spawn(move || {
            use std::os::fd::FromRawFd;
            let mut file = unsafe { std::fs::File::from_raw_fd(w) };
            let _ = file.write_all(&input_owned);
            // file dropped here -> w closed.
        });
        let result = f();
        // Restore drops fd 0; the writer's pipe peer is closed by the dup2(saved, 0)
        // overwriting the only reader; the writer will see EPIPE or already be done.
        let _ = handle.join();
        result
    }
}

fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_input_round_trip() {
        let captured = with_stdin_fd0(b"hello\n", || {
            let mut buf = [0u8; 16];
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            assert!(n >= 0);
            buf[..n as usize].to_vec()
        });
        assert_eq!(captured, b"hello\n");
    }

    #[test]
    fn fd0_is_restored_after_call() {
        let saved = unsafe { libc::dup(0) };
        with_stdin_fd0(b"x", || ());
        // After the call, fd 0 should still be a valid descriptor; reading
        // from it shouldn't be EBADF.
        let mut buf = [0u8; 1];
        // Use a poll to check fd 0 is open; reading would block on the
        // terminal in interactive contexts. Just verify the fd is valid:
        let mut pfd = libc::pollfd { fd: 0, events: 0, revents: 0 };
        let ret = unsafe { libc::poll(&mut pfd, 1, 0) };
        // ret >= 0 means the fd is valid (could be ready or not, doesn't matter).
        assert!(ret >= 0);
        unsafe { libc::close(saved); }
        let _ = buf;
    }

    #[test]
    fn large_input_uses_writer_thread() {
        let big = vec![b'a'; INLINE_STDIN_THRESHOLD + 100];
        let captured = with_stdin_fd0(&big, || {
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 { break; }
                got.extend_from_slice(&buf[..n as usize]);
            }
            got
        });
        assert_eq!(captured.len(), big.len());
    }
}
```

- [ ] **Step 2: Wire the module into lib.rs**

Add `pub(crate) mod stdin_pipe;` to `crates/huck-engine/src/lib.rs` (after the existing module list).

- [ ] **Step 3: Run the suite**

```bash
cargo test --workspace stdin_pipe -- --nocapture
cargo test --workspace --quiet
```

Expected: green; the three new stdin_pipe tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/stdin_pipe.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v205 task 6: stdin pipe helper (with_stdin_fd0)

Replaces fd 0 with a pipe carrying caller-supplied bytes for the duration of a
closure, then restores fd 0 in an RAII guard (runs on panic too). Inputs up to
INLINE_STDIN_THRESHOLD (4 KiB) write inline before the swap; larger inputs are
fed by a writer thread that closes the pipe when done. Three unit tests cover
short round-trip, fd 0 restoration, and writer-thread path. Suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `Engine::exec` + `ExecBuilder` + `Output.stderr`

**Files:**
- Create: `crates/huck-engine/src/exec_builder.rs`
- Modify: `crates/huck-engine/src/engine.rs` (Output struct + new `exec` method; update `run`/`capture`)
- Modify: `crates/huck-engine/src/lib.rs` (re-export `ExecBuilder`)

- [ ] **Step 1: Extend `Output` with `stderr` in engine.rs**

Replace `engine.rs:25-32`:

```rust
/// The captured result of [`Engine::capture`] (or [`ExecBuilder::capture`]).
#[derive(Debug, Clone)]
pub struct Output {
    /// Everything the script wrote to stdout. Under `merge_stderr` this also
    /// contains the script's stderr bytes, interleaved in execution order.
    pub stdout: String,
    /// Everything the script wrote to stderr. Empty when none was written, or
    /// when `merge_stderr` routed it into `stdout`.
    pub stderr: String,
    /// The script's exit status.
    pub exit_code: i32,
}
```

- [ ] **Step 2: Update `Engine::capture` to populate both buffers**

Replace `engine.rs:64-72`:

```rust
pub fn capture(&mut self, src: &str) -> Output {
    self.exec(src).capture()
}
```

Add the new `exec` method:

```rust
/// Start an advanced execution chain. Borrows `&mut self` for the chain's
/// lifetime. See [`ExecBuilder`].
pub fn exec(&mut self, src: &str) -> crate::exec_builder::ExecBuilder<'_> {
    crate::exec_builder::ExecBuilder::new(self, src.to_string())
}
```

`Engine::run` stays as-is (it's already `exec(src).run()` semantically — kept as a shortcut).

- [ ] **Step 3: Create `exec_builder.rs`**

```rust
//! `ExecBuilder` — per-call builder for [`Engine::exec`].
//!
//! Holds the script source + optional stdin bytes + merge flag, and runs them
//! through the engine's sink-aware path on `.run()` / `.capture()`.

use crate::engine::{Engine, Output};
use crate::executor::{StderrSink, StdoutSink};

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
}

impl<'a> ExecBuilder<'a> {
    pub(crate) fn new(engine: &'a mut Engine, src: String) -> Self {
        ExecBuilder { engine, src, stdin: None, merge: false }
    }

    /// Feed these bytes as the script's stdin (fd 0). EOF arrives immediately
    /// after the bytes are consumed.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Route the script's fd 2 to fd 1 (bash `2>&1`). Under `.capture()` the
    /// merged bytes land in `Output.stdout` and `Output.stderr` is empty.
    pub fn merge_stderr(mut self) -> Self {
        self.merge = true;
        self
    }

    /// Run the script; fd 1 and fd 2 inherit (or merged-to-fd1 if `merge_stderr`).
    pub fn run(self) -> i32 {
        let mut out = StdoutSink::Terminal;
        let mut err = if self.merge { StderrSink::Merged } else { StderrSink::Terminal };
        self.run_with_sinks(&mut out, &mut err)
    }

    /// Run the script; capture fd 1 and fd 2 into `Output`.
    pub fn capture(self) -> Output {
        let mut buf_out: Vec<u8> = Vec::new();
        let mut buf_err: Vec<u8> = Vec::new();
        let (exit_code, stderr_str) = {
            let mut out = StdoutSink::Capture(&mut buf_out);
            // Merged: stderr writes go to the active stdout writer (buf_out).
            // Non-merged: stderr writes go to a separate capture buffer.
            if self.merge {
                let mut err = StderrSink::Merged;
                let code = self.run_with_sinks(&mut out, &mut err);
                (code, String::new())
            } else {
                let mut err = StderrSink::Capture(&mut buf_err);
                let code = self.run_with_sinks(&mut out, &mut err);
                (code, String::from_utf8_lossy(&buf_err).into_owned())
            }
        };
        Output {
            stdout: String::from_utf8_lossy(&buf_out).into_owned(),
            stderr: stderr_str,
            exit_code,
        }
    }

    fn run_with_sinks(self, out: &mut StdoutSink, err: &mut StderrSink) -> i32 {
        let ExecBuilder { engine, src, stdin, .. } = self;
        let run = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 {
            let label = engine.shell_cell().borrow().shell_argv0.clone();
            let args = engine.shell_cell().borrow().positional_args.clone();
            let code = crate::shell::run_program_in_sinks(
                &src, None, args, &label, false, out, err, engine.shell_cell(),
            );
            engine.shell_cell().borrow_mut().set_last_status(code);
            code
        };
        match stdin {
            Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || run(out, err)),
            None => run(out, err),
        }
    }
}
```

- [ ] **Step 4: Re-export from lib.rs**

In `crates/huck-engine/src/lib.rs`, after the existing `pub use engine::Engine;` line:

```rust
pub mod exec_builder;
pub use exec_builder::ExecBuilder;
pub use executor::{StderrSink, StdoutSink};
```

(`StderrSink`/`StdoutSink` are re-exported so advanced embedders / tests can drive the sinks directly.)

- [ ] **Step 5: Add unit tests for the new ExecBuilder behavior**

Append to `engine.rs::mod tests`:

```rust
#[test]
fn exec_capture_stdout_and_stderr_separately() {
    let mut e = Engine::new();
    let out = e.exec("echo hi; echo err >&2").capture();
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.stderr, "err\n");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn exec_merge_stderr_interleaves_into_stdout() {
    let mut e = Engine::new();
    let out = e.exec("echo hi; echo err >&2; echo bye")
        .merge_stderr()
        .capture();
    assert_eq!(out.stdout, "hi\nerr\nbye\n");
    assert_eq!(out.stderr, "");
}

#[test]
fn exec_feeds_stdin() {
    let mut e = Engine::new();
    let out = e.exec("read x; read y; echo \"$x-$y\"")
        .stdin(b"hello\nworld\n".to_vec())
        .capture();
    assert_eq!(out.stdout, "hello-world\n");
}

#[test]
fn exec_large_stdin_uses_writer_thread() {
    // Feeds 5 KiB - above the 4 KiB inline threshold; ensures the writer-thread
    // path completes the read.
    let big: Vec<u8> = std::iter::repeat(b'a').take(5000).chain(std::iter::once(b'\n')).collect();
    let mut e = Engine::new();
    let out = e.exec("read line; echo \"len=${#line}\"")
        .stdin(big)
        .capture();
    assert_eq!(out.stdout, "len=5000\n");
}

#[test]
fn capture_includes_stderr_field() {
    let mut e = Engine::new();
    let out = e.capture("echo a; echo b >&2");
    assert_eq!(out.stdout, "a\n");
    assert_eq!(out.stderr, "b\n");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn parse_error_diagnostic_in_stderr() {
    let mut e = Engine::new();
    let out = e.capture("if [");
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("syntax error"), "got: {:?}", out.stderr);
}

#[test]
fn exec_run_inherits_then_exec_capture_works() {
    // Borrow discipline: back-to-back exec chains compile and work.
    let mut e = Engine::new();
    e.exec("x=set-in-first").run();
    let out = e.exec("echo \"$x\"").capture();
    assert_eq!(out.stdout, "set-in-first\n");
}
```

- [ ] **Step 6: Update the rustdoc example on `Engine::exec`**

The doctest already on `engine.rs` uses `capture` returning `stdout` + `exit_code`. Update the example to demonstrate the new `Output.stderr`:

```rust
//! ```
//! use huck_engine::Engine;
//! let mut e = Engine::new();
//! e.set_var("NAME", "world");
//! assert_eq!(e.run("echo \"hi $NAME\""), 0);
//! let out = e.capture("echo $((6 * 7)); echo done >&2");
//! assert_eq!(out.stdout, "42\n");
//! assert_eq!(out.stderr, "done\n");
//! assert_eq!(out.exit_code, 0);
//!
//! // For stdin + stderr capture:
//! let out = e.exec("read x; printf 'got=%s\\n' \"$x\"")
//!     .stdin(b"hello\n".to_vec())
//!     .capture();
//! assert_eq!(out.stdout, "got=hello\n");
//! ```
```

- [ ] **Step 7: Run the suite (including doc tests)**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
```

Expected: green. All new ExecBuilder tests + the updated doc example pass.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/{exec_builder.rs,engine.rs,lib.rs}
git commit -m "$(cat <<'EOF'
v205 task 7: Engine::exec + ExecBuilder + Output.stderr field

Adds Engine::exec(src) -> ExecBuilder<'_> with stdin(...) / merge_stderr() /
run() / capture() methods. Output gains a `stderr` field populated by
capture() (empty when nothing was written or when merge_stderr was used).
Wires up the new StderrSink::{Capture, Merged} variants and the stdin pipe
helper from task 6. Seven new unit tests cover split/merged capture, stdin
feed (inline + writer-thread paths), parse-error stderr routing, and
back-to-back exec chains. Suite green; doc test updated.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Bash-diff harness for engine capture

**Files:**
- Create: `tests/scripts/engine_capture_diff_check.sh`
- Create: `crates/huck-engine/tests/engine_capture_diff.rs`

- [ ] **Step 1: Create the Rust driver binary**

`crates/huck-engine/tests/engine_capture_diff.rs`:

```rust
//! Driver for the engine_capture_diff_check.sh bash-diff harness.
//!
//! Reads two args from argv: the mode ("split" | "merged") and the fragment.
//! Runs the fragment through `Engine` with the matching mode and prints:
//!   STDOUT:<n>\n<bytes>STDERR:<n>\n<bytes>EXIT:<code>\n
//! The harness diffs huck's output against the equivalent bash run.

use huck_engine::Engine;
use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let out = if mode == "merged" {
        e.exec(&fragment).merge_stderr().capture()
    } else {
        e.exec(&fragment).capture()
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", out.stdout.len()).unwrap();
    h.write_all(out.stdout.as_bytes()).unwrap();
    writeln!(h, "STDERR:{}", out.stderr.len()).unwrap();
    h.write_all(out.stderr.as_bytes()).unwrap();
    writeln!(h, "EXIT:{}", out.exit_code).unwrap();
}
```

- [ ] **Step 2: Create the diff harness**

`tests/scripts/engine_capture_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Drives the huck Engine + bash on the same fragments and asserts byte-identical
# (stdout, stderr, exit_code) (or merged-stdout + exit_code for merged mode).
#
# Requires: bash 5+, /bin/sh in PATH, the huck workspace built (`cargo build`).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --workspace --bin huck >/dev/null 2>&1 || true
cargo build --quiet -p huck-engine --tests >/dev/null 2>&1
DRIVER=$(find target/debug/deps -maxdepth 1 -name 'engine_capture_diff-*' -executable -print0 | xargs -0 ls -t | head -1)
if [ -z "$DRIVER" ]; then
    echo "FAIL: could not locate engine_capture_diff driver" >&2
    exit 1
fi

run_huck() { "$DRIVER" "$1" "$2"; }

run_bash_split() {
    local frag=$1
    local out_file err_file exit_code
    out_file=$(mktemp)
    err_file=$(mktemp)
    bash -c "$frag" >"$out_file" 2>"$err_file"
    exit_code=$?
    local out_bytes err_bytes
    out_bytes=$(wc -c <"$out_file")
    err_bytes=$(wc -c <"$err_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:%s\n' "$err_bytes"
    cat "$err_file"
    printf 'EXIT:%s\n' "$exit_code"
    rm -f "$out_file" "$err_file"
}

run_bash_merged() {
    local frag=$1
    local out_file exit_code
    out_file=$(mktemp)
    bash -c "$frag" >"$out_file" 2>&1
    exit_code=$?
    local out_bytes
    out_bytes=$(wc -c <"$out_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:0\n'
    printf 'EXIT:%s\n' "$exit_code"
    rm -f "$out_file"
}

FAIL=0
check() {
    local label=$1 mode=$2 frag=$3
    local huck_out bash_out
    huck_out=$(run_huck "$mode" "$frag")
    if [ "$mode" = "merged" ]; then
        bash_out=$(run_bash_merged "$frag")
    else
        bash_out=$(run_bash_split "$frag")
    fi
    if [ "$huck_out" != "$bash_out" ]; then
        echo "FAIL [$label] mode=$mode"
        diff <(printf '%s' "$huck_out") <(printf '%s' "$bash_out") || true
        FAIL=1
    else
        echo "PASS [$label] mode=$mode"
    fi
}

# Builtin-only fragments
check 'echo-only'        split  'echo hi'
check 'echo-and-err'     split  'echo hi; echo err >&2'
check 'echo-and-err'     merged 'echo hi; echo err >&2'
check 'exit-status'      split  'exit 5'

# External-process fragments
check 'sh-mixed'         split  '/bin/sh -c "echo out; echo err >&2"'
check 'sh-mixed'         merged '/bin/sh -c "echo out; echo err >&2; echo out2"'

# Pipeline fragments
check 'pipeline'         split  'echo hi | cat'
check 'pipeline-err'     split  '/bin/sh -c "echo err >&2" | cat'

# Redirect fragments
check 'redirect-2to1'    split  'echo hi 2>&1'
check 'redirect-err-to-out' merged 'echo err >&2; echo out'

if [ $FAIL -ne 0 ]; then
    echo "engine_capture_diff_check FAILED" >&2
    exit 1
fi
echo "engine_capture_diff_check OK"
```

`chmod +x tests/scripts/engine_capture_diff_check.sh`.

- [ ] **Step 3: Run the harness**

```bash
chmod +x tests/scripts/engine_capture_diff_check.sh
bash tests/scripts/engine_capture_diff_check.sh
```

Expected: every check `PASS`. If any fragment diverges, the diff is printed — fix the underlying bug.

- [ ] **Step 4: Run the full suite + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --all-targets --workspace -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/engine_capture_diff_check.sh crates/huck-engine/tests/engine_capture_diff.rs
git commit -m "$(cat <<'EOF'
v205 task 8: bash-diff harness for engine capture (split + merged modes)

A small Rust driver binary (engine_capture_diff) runs fragments through the
Engine in either split or merged mode and emits a parseable STDOUT/STDERR/EXIT
report. The harness diffs that against bash's output on the same fragment
(via temp-files for split; 2>&1 for merged). Ten fragments span builtins-only,
externals (/bin/sh), pipelines, and redirects. PASS for all on a clean checkout.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Final verify + docs + L-25 removal

**Files:**
- Modify: `docs/architecture.md` (add the ExecBuilder/StderrSink note)
- Modify: `docs/bash-divergences.md` (delete the L-25 entry)

- [ ] **Step 1: Update `docs/architecture.md`**

Find the existing `huck-engine` description (around the four-crate workspace overview, line ~19) and append after the `huck_engine::Engine` sentence:

```
The advanced embedding path is `huck_engine::ExecBuilder` returned from
`Engine::exec(src)` — it supports stdin feed (`.stdin(bytes)`) and stderr-as-merged
into stdout (`.merge_stderr()`), then runs either as `.run() -> i32` (fd 1/2 inherit)
or `.capture() -> Output { stdout, stderr, exit_code }` (both buffers populated).
Internally, `huck_engine::StderrSink::{Terminal, Merged, Capture}` is the
symmetric counterpart of `StdoutSink`, threaded through the executor and the
builtin-dispatch path; engine-level stdin redirection lives in
`crates/huck-engine/src/stdin_pipe.rs` (CLOEXEC pipe + dup2(r, 0) save/restore
guard).
```

- [ ] **Step 2: Delete the L-25 entry from `docs/bash-divergences.md`**

Find and delete the entire `### L-25: a builtin's `2>&1` inside a capture context can't capture stderr` block (lines ~333-340). Update the Tier 4 count in the summary table at the top if needed.

- [ ] **Step 3: Final suite + harness pass**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --all-targets --workspace -- -D warnings
cargo build --release --workspace --quiet
bash tests/scripts/engine_capture_diff_check.sh
# Run every existing bash-diff harness too:
for h in tests/scripts/*_diff_check.sh; do
    echo "--- $h ---"
    bash "$h" || echo "FAIL: $h"
done
```

Expected: all green. Release binary builds. No regressions in existing harnesses.

- [ ] **Step 4: Smoke-test the headless CLI**

```bash
./target/release/huck -c 'echo hello; echo err >&2'
echo "exit=$?"
# Expected: 'hello' on stdout, 'err' on stderr, exit=0
```

```bash
echo 'echo from-stdin' | ./target/release/huck
# Expected: 'from-stdin'
```

Expected: identical to v204.

- [ ] **Step 5: Commit and prepare for merge**

```bash
git add docs/architecture.md docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
v205 task 9: document ExecBuilder/StderrSink + remove L-25 (resolved)

Architecture doc gains a paragraph on the ExecBuilder embedding path and the
StderrSink/stdin-pipe internals. The L-25 divergence (a builtin's 2>&1 inside
a $() capture context didn't capture stderr) is resolved as a side effect of
the stderr-sink audit and is deleted from the current-divergences doc per the
docs/bash-divergences.md "current-only" policy.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Confirm with user before merging to main**

Per CLAUDE.md: "Don't push directly to main without confirmation. Use `AskUserQuestion` before merging an iteration branch."

When the implementer reaches this point, ASK the user before running:

```bash
git checkout main
git merge --no-ff v205-engine-stderr-stdin -m "Merge v205: Engine stderr capture + stdin feed"
git push origin main
git branch -d v205-engine-stderr-stdin
git push origin --delete v205-engine-stderr-stdin
```

---

## Self-review

**Spec coverage:**
- Public API (`Output.stderr`, `Engine::exec`, `ExecBuilder` with `stdin`/`merge_stderr`/`run`/`capture`): Task 7 implements; Task 9 documents.
- `StderrSink::{Terminal, Merged, Capture}`: Task 1 introduces; Tasks 3–5 thread through.
- Builtin stderr audit (~400 sites): Tasks 2–4 cover (builtins.rs, executor.rs, plus the 7 leaf files).
- External-process stderr (capture + merged via dup2): Task 5.
- Stdin pipe + save/restore guard: Task 6.
- Parse-error diagnostic routed to `Output.stderr`: Task 4 (shell.rs's syntax-error path now uses the threaded `err`) + Task 7 unit test.
- L-25 resolution as a side effect: Task 9 deletes the entry.
- Bash-diff harness `engine_capture_diff_check.sh`: Task 8.
- Architecture doc note: Task 9.

**Placeholder scan:** No "TBD" / "TODO" / "implement later". Mechanical sweep steps have explicit grep commands. The thread-local `err` pointer in Task 4 has full code (not a sketch).

**Type consistency:** `Output { stdout, stderr, exit_code }` consistent across Task 7's code and Task 8's harness driver. `StderrSink` variants (`Terminal`/`Merged`/`Capture`) consistent. `with_stdin_fd0` signature consistent between Task 6's definition and Task 7's `ExecBuilder::run_with_sinks` call site. `run_program_in_sinks` signature consistent between Task 4 (Step 1) and Task 7 (`exec_builder.rs`).

**Scope check:** Each task ends with a green-suite commit; no task depends on later tasks for the suite to pass. Tasks 2–4 are the audit's heaviest sweep — they remain mechanical and reviewable per file group.

**Audit cost surfaced:** The plan acknowledges in its preamble + in tasks 2–4 that the builtin-stderr audit is ~400 sites. The thread-local fallback (Task 4) avoids threading parameters through deep call chains in `expand.rs` / `param_expansion.rs` / `shell_state.rs`, trading explicit parameters for a contained, RAII-guarded thread-local pointer. If during execution this trade feels wrong, the alternative is to thread `err: &mut dyn Write` through those functions explicitly — same external behavior, larger diff.
