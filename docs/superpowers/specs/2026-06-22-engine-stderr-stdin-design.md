# v205: `Engine` stderr capture + stdin feed — Design

**Status:** approved 2026-06-22
**Iteration:** v205
**Builds on:** v204 (`huck_engine::Engine` embedding facade)

## Goal

Complete the IO surface of `huck_engine::Engine` for the test/diagnostic-host use
case: capture **stderr** alongside stdout (with an optional `2>&1`-style merge)
and feed **stdin** into the script. v204 shipped stdout capture and exit-code
return; v205 adds the other two axes so an embedder wrapping huck can drive a
script's input and observe both output streams.

## Decisions (from brainstorming)

1. **Test/diagnostic host is the use case.** Sandboxing (cwd, restricted) and
   interactive-backend (streaming, completion) are deferred to later iterations.
2. **Always-both capture, with a merge option.** `Output` grows a `stderr` field;
   `merge_stderr()` is the bash `2>&1` analog.
3. **Stdin is in scope.** The natural sibling for a test host; `read x` /
   piped-stdin scripts become testable.
4. **Builder-on-the-call API shape.** `engine.exec(src).stdin(...).merge_stderr().run()/.capture()`
   for the advanced cases; `run(src)`/`capture(src)` stay as simple shortcuts.

## Public API

`Output` gains a `stderr` field. New `ExecBuilder` returned from `Engine::exec`.

```rust
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,    // NEW: empty if nothing written to fd 2 (or merged into stdout)
    pub exit_code: i32,
}

impl Engine {
    // ... v204 surface unchanged ...

    /// Start an advanced execution chain. Borrows `&mut self` for the chain's lifetime.
    pub fn exec(&mut self, src: &str) -> ExecBuilder<'_>;
}

pub struct ExecBuilder<'a> { /* &'a mut Engine, src, stdin, merge */ }

impl<'a> ExecBuilder<'a> {
    /// Feed these bytes as the script's stdin (fd 0). EOF lands after the bytes.
    pub fn stdin(self, input: impl Into<Vec<u8>>) -> Self;

    /// Route the script's fd 2 to fd 1 (bash `2>&1`). Under `.capture()` the
    /// merged bytes land in `Output.stdout`; `Output.stderr` is empty. Under
    /// `.run()` fd 2 follows fd 1's destination (inherited terminal).
    pub fn merge_stderr(self) -> Self;

    /// Run the script; fd 1 and fd 2 inherit (or merged-to-fd1 if `merge_stderr`).
    /// Returns the exit status. Stdin per `.stdin(...)` (else inherited).
    pub fn run(self) -> i32;

    /// Run the script; capture fd 1 and fd 2 into `Output`. Stdin per `.stdin(...)`.
    pub fn capture(self) -> Output;
}
```

`run(src)` and `capture(src)` are unchanged externally; internally they are
shortcuts for `exec(src).run()` and `exec(src).capture()` (no stdin, no merge).
The builder is **per-call** — each `exec(...)…` chain is independent of any
prior or subsequent call. Engine state (vars, functions, cwd, `$?`, positionals)
persists across calls exactly as in v204.

## Semantics

### Capture (`exec(src).capture()`)

- Without `merge_stderr`: `Output.stdout` and `Output.stderr` populated
  separately. Empty string when the stream had no output.
- With `merge_stderr`: ALL bytes interleaved in execution order land in
  `Output.stdout`; `Output.stderr` is `""`. For external processes the
  interleaving is kernel-ordered (single pipe write-end shared by fd 1 and fd 2,
  matching bash `2>&1`). For pure-builtin scripts the order is source-code order
  (single sink, sequential calls).

### Inherit (`exec(src).run()`)

- Without `merge_stderr`: fd 1 and fd 2 of every command in the script inherit
  the embedder's fd 1 and fd 2 — v204's `run` behavior.
- With `merge_stderr`: fd 2 follows fd 1's destination. Builtin stderr writes
  route through `StderrSink::Merged` → the active stdout sink (fd 1 when
  inherited). External processes get `dup2(1, 2)` for the child. End result
  matches `bash -c 'cmd 2>&1'` byte-for-byte.

### Stdin (`exec(src).stdin(bytes)`)

- Default (no `.stdin(...)`): fd 0 inherits the process's fd 0 (v204 behavior).
- With `.stdin(bytes)`: a CLOEXEC pipe is created; the bytes are the script's
  stdin; EOF arrives immediately after. `read x; read y; …` consumes the bytes;
  reading more than was supplied sees EOF.
- Setup is scoped to the single `run()`/`capture()` call. `dup(0)` saves the
  prior fd 0; `dup2(pipe_r, 0)` installs the new one; the call runs; `dup2(saved, 0)`
  restores. The embedder's fd 0 is intact on return.
- For short inputs (≤ a small internal threshold, e.g. `PIPE_BUF` — 4 KiB on
  Linux), the engine writes the input inline before swapping fd 0 (no thread).
  For longer inputs a writer thread writes the input to the pipe write-end and
  closes it. The threshold is internal; both paths are correct.

### Parse errors and setup failures

- A lex/parse error during `exec(src).capture()` writes the diagnostic into
  `Output.stderr` (this **changes** from v204, where parse-error stderr inherits
  during `capture`; the test-host use case wants the diagnostic in the buffer).
  Exit code stays `2`.
- Engine-level setup failures (pipe/dup2/fork) print `huck: pipe: <err>` /
  `huck: dup2: <err>` to the **real** fd 2 (always — these happen before the
  script runs) and return a `1`-exit `Output` with empty buffers. The user
  script is not executed.

### Process-state invariants (unchanged from v204)

- No signal handlers installed; no rc file read.
- All engine-created fds (stdin pipe ends, stderr capture pipe ends) are
  `CLOEXEC` — a script that `exec`s an external command does not leak them.
- `Engine` is not `Send`/`Sync`. One engine per thread.

### Reentrancy

- `exec(src)` returns `ExecBuilder<'_>` holding `&mut self`; the borrow is
  released at `.run()`/`.capture()`. A second `exec()` chain cannot start until
  the first completes (compile-time enforced).
- Nested re-entry from a script callback is out of scope (no callback path
  exists in v205).

## Internal architecture

### `StderrSink` — symmetrical to `StdoutSink`

New enum in `crates/huck-engine/src/executor.rs`:

```rust
pub enum StderrSink<'a> {
    Terminal,                    // inherit (write to real fd 2)
    Merged,                      // route to the active StdoutSink
    Capture(&'a mut Vec<u8>),    // in-memory buffer for builtins
}
```

Threaded through every site that today takes `&mut StdoutSink`:
`execute_with_sink`, `run_command`, `run_sequence_body`, `run_pipeline`,
`run_subshell`, the brace-group / if / for / while / case / select / `[[ ]]`
walkers, and the new `run_program_in_sinks` / `process_line_in_sinks` runners.

The v204 `run_program_in_sink` and `process_line_in_sink` become thin wrappers
that pass `StderrSink::Terminal` — behavior-identical when no embedder uses
stderr capture.

### External-process stderr capture / merge

Mirror the existing stdout pipe machinery (`executor.rs` ~line 390, the
`StdoutSink::Capture` branch that creates a pipe and drains the read-end into
the buffer post-`waitpid`):

- `StderrSink::Capture(buf)`: create a fresh CLOEXEC pipe for stderr; child's
  fd 2 is the write-end; parent drains the read-end into `buf` after `waitpid`.
  Pipe drain happens **before** waitpid only when both sinks have capture pipes
  to drain (to avoid the existing deadlock-avoidance pattern); when only stderr
  captures, the order matches the existing stdout pattern.
- `StderrSink::Merged`: reuse the stdout pipe's write-end — `dup2` it onto fd 2
  of the child. Kernel-level interleaving handles ordering. When stdout is
  `Terminal`, dup2 fd 1 onto fd 2 directly (matches bash).

### Builtin stderr — audit + helper

Every builtin write to stderr (today: `eprintln!`, `writeln!(io::stderr(), …)`,
`io::stderr().write_all(…)`) is routed through a small helper:

```rust
fn write_stderr(sink: &mut StderrSink, sink_out: &mut StdoutSink, args: fmt::Arguments);
// dispatch:
//   Terminal  -> writeln!(io::stderr(), ...)
//   Merged    -> route to `sink_out` (write into Capture buffer or fd 1)
//   Capture(b) -> writeln!(b, ...)
```

A `eprintln_huck!` / `write_stderr!` macro on top of the helper keeps call-site
ergonomics. The audit covers `builtins.rs`, `test_builtin.rs`, `executor.rs`'s
diagnostic paths, parse/lex error printing in `shell.rs`'s run wrappers, and
the error paths in `arith.rs` / `expand.rs` / `param_expansion.rs` that today
emit `huck:`-prefixed errors directly.

**Side effect:** L-25 (a builtin's `2>&1` inside a capture context can't
capture stderr) is resolved by this audit — the in-memory stderr sink is now
real, so the `2>&1` redirect that today reaches a non-existent in-memory fd 2
will now hit the buffer.

### Stdin pipe machinery

A `with_stdin_fd0(input: &[u8], f: impl FnOnce() -> R) -> R` helper in
`engine.rs`:

1. `pipe2(O_CLOEXEC)` → `(r, w)`.
2. `saved = dup(0)`.
3. `dup2(r, 0)`; `close(r)`.
4. If `input.len() <= INLINE_STDIN_THRESHOLD` (e.g. 4 KiB):
   - `write(w, input)`; `close(w)`.
5. Else:
   - Spawn `std::thread::spawn(move || { write_all(w, input); close(w); })`.
6. Run `f()`.
7. `dup2(saved, 0)`; `close(saved)`.
8. If thread spawned: `join()` (must complete before return).

Any setup failure in steps 1–3 closes any opened fds, prints `huck: pipe:
<err>` to real fd 2, returns a `1`-exit `Output`/`i32` per the calling method.
The `dup(0)` save is needed for correct restoration even if the script's own
`exec` rewrites fd 0 internally.

### Shared run path

`run_program_in_sink` from v204 is generalized to `run_program_in_sinks(contents,
argv0, args, label, push_main_frame, stdout_sink, stderr_sink, shell_cell)`.
Existing `run_program` becomes a thin wrapper:

```rust
pub fn run_program(contents, argv0, args, label, push_main_frame, shell_cell) -> i32 {
    run_program_in_sinks(
        contents, argv0, args, label, push_main_frame,
        &mut StdoutSink::Terminal, &mut StderrSink::Terminal,
        shell_cell,
    )
}
```

`Engine::exec` builds the right pair of sinks per the builder state, wraps them
in the stdin helper, and calls `run_program_in_sinks` (for `bash -c` semantics)
or `run_script_in_sinks` (for script semantics — `run_file` keeps its existing
main-frame distinction).

## CLI dogfood

No CLI changes. The v204 dogfood already routes the headless `-c`/script path
through `Engine::run` / `Engine::run_file`, both of which keep their v204
behavior (inherit fd 1/2, no stdin override). The new `Engine::exec` builder is
embedder-only.

## Build / packaging

No new crates, no new external deps (`std::thread` is already used). All
changes are additive in `huck-engine`; the CLI crate is untouched. Release
binary + packaging paths unchanged.

## Testing & verification

### Unit tests (`crates/huck-engine/src/engine.rs` `mod tests`)

- `capture` populates `stdout` and `stderr` separately (`echo hi; echo err >&2`).
- `capture` returns empty `stderr` when none written.
- `exec(src).merge_stderr().capture()` interleaves stdout+stderr in execution
  order; `stderr` is empty.
- `exec(src).merge_stderr().run()` writes interleaved bytes to the inherited
  fd 1 (verified via a child-process roundtrip).
- `exec("read x; echo $x").stdin(b"hello\n").capture()` → `stdout = "hello\n"`.
- `stdin` short-input (inline) and large-input (writer-thread) paths both work
  end-to-end.
- Multi-line script under each combination still works.
- Parse error under `capture` puts the diagnostic into `Output.stderr`;
  `exit_code = 2`.
- Engine state (vars, functions) persists across two `exec(...)` calls (builder
  is per-call, engine is persistent).
- Borrow discipline: back-to-back `engine.exec(s).run()` then
  `engine.exec(s).capture()` works.

### Doc example

Rustdoc on `Engine::exec` showing the test-host pattern: feed stdin, capture
both buffers, assert exit code. Exercised by `cargo test --doc`.

### Builtin stderr coverage

Three targeted tests because of the audit risk:

- `declare -p NONEXISTENT 2>&1` under `capture()` → diagnostic lands in
  `Output.stdout` (the `2>&1` retargets the builtin's stderr; resolves L-25).
- `set -u; echo $UNSET` under `capture()` → `Output.stderr` contains the
  diagnostic.
- `echo hi > /nonexistent/path` under `capture()` → `Output.stderr` contains
  the `huck:`-prefixed open error.

### External-process stderr coverage

- `sh -c 'echo out; echo err >&2'` under `capture()` → buffers populated
  separately.
- Same fragment under `merge_stderr().capture()` → all bytes interleaved in
  `Output.stdout`; `Output.stderr` empty.

### Pipeline coverage

- `echo hi | cat` under `capture()` → last stage's stdout captured.
- A pipeline where any non-final stage writes to stderr → those bytes appear in
  `Output.stderr` (or merged into stdout under `merge_stderr`).

### Bash-diff harness

New `tests/scripts/engine_capture_diff_check.sh` drives a small Rust test
binary that uses `Engine` against ~10 fragments — builtin-only,
external-only, mixed, pipeline, redirects — and asserts byte-identical match
against `bash -c '…' 2>&1` (merged path) and `bash -c '…' 2>err 1>out` (split
path). Catches drift between huck's stderr-routing and bash's.

### CLI byte-identical gate

- All `tests/*.rs` integration tests green.
- All existing `tests/scripts/*_diff_check.sh` harnesses green.
- `cargo test --workspace` count == pre-change baseline **plus only the new
  Engine tests**.
- `cargo clippy --all-targets` clean; release binary builds; headless `huck -c
  '…'` and `huck script.sh` behave exactly as before.

## Risks & mitigations

- **Builtin-stderr audit churn.** Every `eprintln!` / `writeln!(stderr(), …)`
  in the engine becomes sink-aware. Mitigate: a single `write_stderr!` macro
  keeps call sites compact; do the audit as one focused task with a clean
  `git grep` checklist; the existing bash-diff harnesses catch regressions in
  diagnostic text routing.
- **Merge byte-ordering for external processes.** Must match bash. Mitigate:
  the `dup2(1, 2)` approach is exactly what bash uses; the diff harness
  verifies it on fragments that mix stdout/stderr writes.
- **Stdin save/restore correctness.** A script that itself rewrites fd 0 with
  `exec` must not break restoration. Mitigate: `dup(0)` happens once at entry
  before any user code; `dup2(saved, 0)` always runs in a guard / RAII drop on
  the way out, so even a panic during the script restores fd 0.
- **Writer-thread join blocks shell exit.** If the script exits without
  reading the whole input, the writer's `write` blocks on a full pipe. Mitigate:
  the engine closes the **read** end (the fd 0 that was dup2'd in) before
  joining; the write end's next write returns `EPIPE`; the thread catches it
  silently and returns. (Same pattern as `Command::stdin(Stdio::piped())`.)
- **Parse-error stderr routing change.** v204 had parse errors inherit during
  `capture`; v205 routes them into `Output.stderr`. This is a deliberate change
  (test-host use case wants the diagnostic in the buffer), but it's a behavior
  change for any existing v204 caller relying on parse errors going to the
  terminal. Since v204 has no external callers (only the in-tree CLI, which
  uses `run`), this is safe.

## Out of scope

- Asymmetric capture (capture-stdout-only / capture-stderr-only). v206 can add
  `.capture_stdout_only()` / `.capture_stderr_only()` to the same builder.
- Streaming / line-by-line callbacks. A separate interactive-backend iteration.
- Cwd, restricted/no-exec, timeout, custom builtins, custom signal handlers.
  v204's "Out of scope" list stays out.
- Stable semver / crates.io publish. v205 is **not** an API freeze; the
  breaking change to `Output` is fine because nothing is published yet.
- A `Result`-typed surface. Exit-code-as-error stays.
- `stdin` from a `Read`-impl. Only `impl Into<Vec<u8>>` for v205.
- Reentrant nested engine calls from a script callback. No callback path exists.

## Task decomposition (for the plan)

1. **`StderrSink` plumbing (behavior-identical).** Add the enum; thread
   `&mut StderrSink` through every site that takes `&mut StdoutSink`; add
   `run_program_in_sinks` / `process_line_in_sinks`; make existing entry points
   thin wrappers passing `StderrSink::Terminal`. Full suite green = parity gate.
2. **Builtin-stderr audit.** Route every direct stderr write through
   `write_stderr!`. Verify the L-25 test case (`declare -p X 2>&1` under
   `$( … )`) now captures the diagnostic. Full suite + bash-diff harnesses green.
3. **External-process stderr (capture + merged).** Mirror the stdout pipe
   machinery for stderr; implement merged via `dup2(stdout_w, 2)` on the child.
   Targeted external-process tests pass.
4. **Stdin pipe helper.** Add `with_stdin_fd0`; inline vs writer-thread branch;
   save/restore fd 0 in an RAII guard. Targeted stdin tests pass.
5. **`Engine::exec` + `ExecBuilder`.** Add the builder; extend `Output` with
   `stderr`; update `Engine::capture` to populate both buffers via the builder.
   Doc example + unit tests + `engine_capture_diff_check.sh` harness pass.
6. **Verify.** Equal-baseline-plus-new `cargo test --workspace`, all
   `*_diff_check.sh` harnesses, clippy, release binary, CLI smoke. Brief
   `docs/architecture.md` update noting `ExecBuilder` and `StderrSink`. Update
   `docs/bash-divergences.md` to DELETE the L-25 entry (resolved as a side
   effect of task 2).
