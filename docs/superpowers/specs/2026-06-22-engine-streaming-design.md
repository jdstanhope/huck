# v207: `Engine::exec` streaming-output callbacks — Design

**Status:** approved 2026-06-22
**Iteration:** v207
**Builds on:** v205 (`ExecBuilder` + capture), v206 (cwd / restricted / timeout knobs)

## Goal

Add two per-call builder methods so embedders can observe a script's stdout
and stderr **line-by-line, in real time, on their own thread** — without
adopting a multi-threaded API. Real-time delivery works even for long-running
external processes (e.g. `find /usr`, `tail -F`).

The use case is interactive embedders: TUIs/web UIs/AI agents watching a
script as it runs, log-tailers, anything that wants progress feedback before
the script finishes. v205 gave embedders capture; v206 gave them sandboxing;
v207 closes the IO-observability loop.

## Decisions (from brainstorming)

1. **v207 ships streaming-output only.** "Streaming + completion API" was two
   independent subsystems; we picked streaming. Completion API defers to v208.
2. **Lines only, not bytes.** `FnMut(&str)` callback. Internal `LineBuf`
   accumulates partial reads; complete lines dispatch on `\n`; final partial
   flushes at EOF.
3. **Single-threaded callback dispatch on the embedder's thread.** No `Send`
   bound, no internal drainer threads visible at the API surface. Embedders
   write closures that borrow their own state directly (`&mut Vec<String>`,
   `&mut self`, etc.) — borrow checker confirms the closure can't outlive what
   it borrows because the builder is consumed at `.run()`/`.capture()`.
4. **Real-time even for external processes.** Today's `Engine::capture`
   blocks the embedder's thread in `waitpid` for the duration of an external
   child. v207 replaces that with a poll-based wait loop (`signalfd` on Linux,
   `kqueue` on macOS) that interleaves pipe-readable wakeups with SIGCHLD —
   so a long-running `find` delivers lines in real time, microsecond latency,
   with no internal threads.
5. **Tee semantics with `.run()` and `.capture()`.** Callbacks compose with
   the existing terminal methods: `.on_stdout_line(cb).capture()` populates
   BOTH the callback AND `Output.stdout`; `.on_stdout_line(cb).run()` fires
   the callback AND re-writes each line to the embedder's real fd 1.

## Public API

Two new methods on `ExecBuilder<'a>`, both `Self`-consuming, both lifetime-tied
to the builder's `'a`:

```rust
impl<'a> ExecBuilder<'a> {
    // ... v205/v206 methods unchanged ...

    /// Invoke `f(line)` for each complete line written to stdout. Trailing
    /// `\n` stripped. Final partial line (if no trailing newline at EOF)
    /// fires once at stream close.
    ///
    /// Callback runs on the caller's thread (the one that called .run/.capture).
    /// No `Send` bound — the closure can borrow `&mut` of the caller's state.
    pub fn on_stdout_line<F: FnMut(&str) + 'a>(self, f: F) -> Self;

    /// Same for stderr. Under `.merge_stderr()`, stderr is dup2'd onto stdout
    /// at the fd level — `on_stderr_line` never fires; all output flows
    /// through `on_stdout_line`.
    pub fn on_stderr_line<F: FnMut(&str) + 'a>(self, f: F) -> Self;
}
```

Internally stored as `Option<Box<dyn FnMut(&str) + 'a>>` so the builder owns
the closure for the call's duration.

### Five usage patterns (will be in the rustdoc)

```rust
// 1. Stateless — just print, log, ignore.
let out = e.exec("for i in 1 2 3; do echo $i; done")
    .on_stdout_line(|line| println!("[stdout] {line}"))
    .capture();
```

```rust
// 2. Stateful — capture &mut of your own data.
let mut lines: Vec<String> = Vec::new();
let out = e.exec("find /etc -maxdepth 1")
    .on_stdout_line(|line| lines.push(line.to_string()))
    .capture();
```

```rust
// 3. Mutating self in a method on the embedder.
impl MyApp {
    fn run_script(&mut self, src: &str) -> i32 {
        let mut e = Engine::new();
        e.exec(src)
            .on_stdout_line(|line| self.append_to_tui(line))
            .on_stderr_line(|line| self.append_to_log(line))
            .run()
    }
}
```

```rust
// 4. Trait-object dispatch — implement a Logger trait, adapt it.
trait Logger { fn line(&mut self, s: &str); }
fn run_with_logger(eng: &mut Engine, src: &str, log: &mut dyn Logger) -> i32 {
    eng.exec(src)
        .on_stdout_line(|line| log.line(line))
        .run()
}
```

```rust
// 5. Coalescing — buffer N lines, render every K.
let mut pending: Vec<String> = Vec::new();
e.exec(src)
    .on_stdout_line(|line| {
        pending.push(line.into());
        if pending.len() >= 100 { render(&pending); pending.clear(); }
    })
    .capture();
```

## Semantics

### Exit code

`.run() -> i32` and `.capture() -> Output` return the same exit code they
would without callbacks. Callbacks are observers; they don't alter status.

### Line delivery contract

- **Within a stream**: strict source order. Writes of "A\nB\nC\n" fire as
  three callbacks "A", "B", "C".
- **Across streams (not merged)**: best-effort time order via poll wake. Not
  guaranteed byte-faithful to what a terminal would show — same as bash's
  separate-fd behavior.
- **Under `.merge_stderr()`**: stderr is dup2'd onto stdout in-kernel for
  children, routed through the active stdout writer for builtins (v205
  behavior). `on_stdout_line` fires for everything; `on_stderr_line` never
  fires.
- **Empty lines**: `echo ""` → `cb("")` once.
- **Partial at EOF**: `printf "no-newline"` → `cb("no-newline")` once at close.
- **Mixed partials**: `printf "a\nb\nc"` (no trailing `\n`) → "a", "b", "c".

### Tee semantics

Setting a callback NEVER disables anything:

- **`.on_stdout_line(cb).capture()`**: callback fires AND `Output.stdout`
  accumulates the full transcript. The buffer is the v205 capture buffer; the
  callback observes lines as they arrive.
- **`.on_stdout_line(cb).run()`**: callback fires AND each line is re-written
  to the embedder's real fd 1 (with `\n` restored) AFTER the callback returns.
  Embedder still sees output on their terminal. Re-write happens post-callback
  so if the callback panics the line never reaches the terminal.
- **`.run()` with NO callbacks** is unchanged from v206: fd 1/2 inherit
  directly, no pipe interposition, no poll loop.

### Non-UTF-8 bytes

`LineBuf::next_line()` returns `&str` via `String::from_utf8_lossy`. Same
policy as v205's `Output.stdout`/`stderr` — invalid sequences become U+FFFD.
Documented; we don't claim byte-faithfulness. Embedders needing raw bytes use
`Engine::run` with their own pipe.

### Very long lines / no-newline programs

No explicit `LineBuf` cap. A 100MB-line program will accumulate 100MB in
`LineBuf.partial` — but the kernel pipe (~64 KB) bottlenecks the producer, so
allocation is bounded by how long we let the script run. A `.timeout(dur)`
caps the worst case.

### Callback that blocks / sleeps

The callback runs synchronously on the embedder's thread. If it sleeps for
100ms, the poll loop pauses for 100ms — external children's writes fill the
kernel pipe (~64 KB) and then block until we drain. **Natural single-thread
backpressure**: the script can't outrun the embedder. Documented.

### Callback panic

Propagates out via standard Rust unwinding through `LineBuf::next_line` →
poll loop → `run_program_in_sinks` → `.run()`/`.capture()`. RAII guards (sink
threading, PID registry, cwd scope, stdin pipe, timer handle) all drop on
unwind — resources released. The EXIT trap doesn't fire; acceptable for an
embedder bug. We do NOT `catch_unwind`.

### Pipeline + subshell ordering

For `cmd1 | cmd2`, only the final stage's fd 1 reaches `on_stdout_line`
(matches `Output.stdout` semantics). Intermediate stages' fd 1 goes to the
inter-stage pipe — invisible. Each stage's stderr (if not merged into its
own fd 1) flows to `on_stderr_line`.

For `( cmd )`, the subshell's stdout/stderr flow through the same callbacks
in script-emission order.

### Builtin vs external delivery

Same contract: complete lines in source order. Internal mechanism differs:
- **Builtins** dispatch via the `StdoutSink::Capture` write site (in-process,
  synchronous).
- **Externals** dispatch via the poll loop (real-time, microsecond latency).

Embedders can't tell which mechanism fired a given line.

### Re-entrancy

A callback that tries to call `engine.exec(...)` recursively is **rejected
at compile time** — the builder borrows `&'a mut Engine`, the closure's `'a`
matches, so the borrow checker prevents nested `&mut Engine` calls during
the chain. No runtime check needed.

### Composition with v205/v206 knobs

- `.timeout(dur)`: `WaitLoop::poll` uses the timer's deadline as its poll
  timeout. When it expires, poll returns; `timeout_flag` already set by
  the timer thread; executor aborts on next `check_interrupt`.
- `.cwd()` / `.restricted()` / `.stdin()`: orthogonal — no interaction. Setup
  order is unchanged.

## Internal architecture

### New module: `crates/huck-engine/src/line_buf.rs` (~50 LOC)

```rust
//! Accumulate raw byte chunks, dispatch complete lines (newline-terminated).

pub struct LineBuf {
    partial: Vec<u8>,
}

impl LineBuf {
    pub fn new() -> Self { Self { partial: Vec::new() } }

    /// Append raw bytes. Caller pulls via `next_line()` after each push.
    pub fn push(&mut self, bytes: &[u8]);

    /// Pull the next complete line (without trailing `\n`). Returns `None`
    /// when no more `\n` is present in the buffer.
    pub fn next_line(&mut self) -> Option<String>;

    /// Pull whatever bytes remain (may be empty). For end-of-stream flush.
    pub fn drain_final(&mut self) -> Option<String>;
}
```

UTF-8 decoding via `String::from_utf8_lossy`. Returns owned `String` so the
caller can pass `&str` (deref) to the user callback without lifetime gymnastics.

### New module: `crates/huck-engine/src/wait_loop.rs` (~200 LOC)

Cross-platform abstraction over (poll on N file descriptors + a SIGCHLD signal source).

```rust
pub struct WaitLoop {
    // platform-specific internals — signalfd + epoll on Linux,
    // kqueue on macOS.
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Readable(RawFd),
    ChildExited,
}

impl WaitLoop {
    pub fn new() -> io::Result<Self>;
    pub fn register_pipe(&mut self, fd: RawFd) -> io::Result<()>;
    pub fn register_sigchld(&mut self) -> io::Result<()>;
    /// Block until at least one registered fd is readable or a child exited.
    /// `timeout` is the maximum wait; `None` for indefinite. Returns the
    /// list of events that became ready.
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>>;
}
```

`#[cfg(target_os = "linux")]` impl uses `signalfd(2)` for SIGCHLD + `poll(2)`
over the fd set. `#[cfg(target_os = "macos")]` impl uses `kqueue(2)` with
`EVFILT_SIGNAL(SIGCHLD)` + `EVFILT_READ` on pipes. Other targets get a
compile-time error (`compile_error!("v207 WaitLoop requires linux or macos")`).

Signal-disposition note: `signalfd` requires SIGCHLD to be blocked process-wide
(otherwise the default handler runs and the signalfd never reads anything).
The engine masks SIGCHLD with `pthread_sigmask` on `WaitLoop::new()` and
unmasks on Drop. Threaded code already accounts for SIGCHLD via
signal_hook (v138) — the existing flag-based dispatch continues to work as
long as we only mask during the wait scope.

### `ExecBuilder` additions

```rust
pub struct ExecBuilder<'a> {
    // ... v205/v206 fields ...
    on_stdout_line: Option<Box<dyn FnMut(&str) + 'a>>,
    on_stderr_line: Option<Box<dyn FnMut(&str) + 'a>>,
}

impl<'a> ExecBuilder<'a> {
    pub fn on_stdout_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
        self.on_stdout_line = Some(Box::new(f));
        self
    }
    pub fn on_stderr_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
        self.on_stderr_line = Some(Box::new(f));
        self
    }
}
```

### Builtin-path dispatch hook

The existing `StdoutSink::Capture(&mut Vec<u8>)` path writes bytes to the
capture buffer. Add a sibling hook: after writing, if a `LineBuf` + callback
are active for stdout, push the just-written bytes into `LineBuf` and dispatch
any complete lines via the callback. Mirror for stderr's `StderrSink::Capture`.

The callback dispatch happens **on the calling thread** (the executor's
thread, which IS the embedder's thread under `!Send + !Sync` Engine). No
threads. No queue.

### External-process poll loop

Replace today's blocking `waitpid` + drainer-thread shape in:
- `run_subprocess` (single external command)
- `Command::Subshell` branch in `run_command` (forked subshell with capture)
- `run_multi_stage` (pipeline final-stage capture)

Today's shape is roughly:
```
make_pipe(); spawn child; spawn drainer thread reading pipe → buf;
waitpid(child); join drainer; (callback receives all bytes via in-memory buf)
```

New shape:
```
make_pipe_out(); make_pipe_err(); spawn child;
WaitLoop::new();
wl.register_pipe(pipe_out.read);
wl.register_pipe(pipe_err.read);
wl.register_sigchld();
loop {
    let events = wl.poll(timer.remaining())?;
    for event in events {
        match event {
            Readable(fd) if fd == pipe_out.read => {
                let n = read_chunk(fd, &mut chunk_buf)?;
                line_buf_out.push(&chunk_buf[..n]);
                while let Some(line) = line_buf_out.next_line() {
                    if let Some(cb) = &mut callbacks.on_stdout { cb(&line); }
                    capture_buf_out.extend(line.as_bytes());
                    capture_buf_out.push(b'\n');
                }
            }
            Readable(fd) if fd == pipe_err.read => { /* symmetric */ }
            ChildExited => {
                if let Some(status) = waitpid(child, WNOHANG)? {
                    drain_until_eof(pipe_out.read, &mut line_buf_out, ...);
                    drain_until_eof(pipe_err.read, &mut line_buf_err, ...);
                    if let Some(final_line) = line_buf_out.drain_final() {
                        callbacks.on_stdout(...);
                    }
                    return status;
                }
            }
        }
    }
}
```

Key properties:
- **No internal threads.** The embedder's thread (the one calling `.capture()`
  / `.run()`) is the only thread running the executor and dispatching
  callbacks.
- **Real-time:** poll wakes on the first pipe write or SIGCHLD. Microsecond
  latency, kernel-scheduled.
- **Tee with `Output.stdout`/`stderr`:** the same loop populates the capture
  buffer (line-by-line, with `\n` restored).
- **`.timeout(dur)` integration:** the timer thread (v206) sets
  `Shell.timeout_flag` AND SIGTERMs the child. The poll loop's
  `wl.poll(timer.remaining())` returns when the timer fires; the SIGCHLD
  wakeup then reaps and the executor's next `check_interrupt` returns
  `Interrupted(Timeout)` → exit 124.
- **SIGINT integration:** unchanged. The SIGINT handler sets `sigint_flag`;
  the poll loop's next iteration checks `check_interrupt`.

### `run()` tee for inherit + callbacks

When `.on_stdout_line(cb).run()` is called:
- We interpose a pipe (same as `.capture()` would) so we can line-buffer.
- After each callback returns successfully, re-write `line + "\n"` to the
  embedder's real fd 1 (saved via `dup(1)` at the call start; restored on
  exit).
- Symmetric for stderr's real fd 2.

If the callback panics, the line never reaches the terminal (acceptable —
the panic propagates anyway).

### Composition in `run_with_sinks`

The v206 composition order is preserved, with callbacks living as a sibling
layer alongside the sink pair:

1. Build sinks (stdout/stderr).
2. Build `Callbacks { on_stdout_line, on_stderr_line }` from the builder.
3. Spawn timer (if `.timeout`).
4. Acquire cwd guard (if `.cwd`).
5. Acquire stdin guard (if `.stdin`).
6. Snapshot+set `Shell.restricted`.
7. Run script via `run_program_in_sinks` (passing callbacks).
8. Restore restricted, drop stdin guard, drop cwd guard.
9. Cancel timer.
10. If `timeout_flag` set, override exit to 124.

Callbacks are accessible to both the builtin write site (in-memory hook) and
the external poll loop. They're owned by the builder; passed by `&mut` down
the run path.

## CLI dogfood

No CLI changes. The CLI doesn't use callbacks; `Engine::run`/`run_file` with
no callbacks set behaves identically to v206 (fd 1/2 inherit, no pipe
interposition, no poll loop).

## Build / packaging

No new external crate deps. Uses `libc` directly for `signalfd`/`kqueue` (same
pattern as existing `libc::kill`/`libc::pipe2` in v205/v206). Workspace
structure unchanged.

## Testing & verification

### Unit tests in `engine.rs::mod tests`

**Builtin path (8 tests):**
- `on_stdout_line_fires_per_line`
- `on_stdout_line_empty_line`
- `on_stdout_line_partial_at_eof`
- `on_stdout_line_mixed_with_partials`
- `on_stderr_line_fires_per_line`
- `on_stdout_line_captures_too` (tee with .capture)
- `on_stdout_line_no_callback_capture_unchanged` (sanity: v205 behavior)
- `on_stdout_line_callback_borrows_state` (compile-test: `&mut Vec<String>`)

**External path (6 tests):**
- `on_stdout_line_external_real_time` (`/bin/sh -c 'echo first; sleep 0.1;
  echo second'` — verify ~100ms gap in callback timestamps)
- `on_stdout_line_external_fires_during_wait` (callback fires before sleep
  ends — proves real-time delivery)
- `on_stdout_line_pipeline_last_stage` (`echo hi | tr a-z A-Z` → "HI")
- `on_stdout_line_pipeline_non_final_stderr` (non-final stage's stderr fires)
- `on_stdout_line_merge_stderr_routes_through_stdout` (merge_stderr → cb sees
  both; on_stderr_line never fires)
- `on_stdout_line_external_long_line` (200k chars, one callback)

**Composition with v205/v206 (5 tests):**
- `on_stdout_line_with_stdin`
- `on_stdout_line_with_cwd`
- `on_stdout_line_with_restricted`
- `on_stdout_line_with_timeout_fires_during_run` (sleep aborted by timeout;
  pre-sleep line delivered)
- `all_knobs_compose`

**Run() tee (3 tests):**
- `on_stdout_line_run_inherits_via_tee` (child-process capture verifies
  embedder's fd 1 saw the lines)
- `on_stdout_line_run_no_callback_no_pipe` (sanity: no-callback fast path)
- `on_stderr_line_run_inherits_via_tee`

**Robustness (3 tests):**
- `callback_panic_propagates_and_cleans_up`
- `callback_can_be_slow_backpressure_works` (script ≥ 5s with 50ms-per-line
  callback × 100 lines)
- `callback_closure_lifetime_borrows_mut_state` (compile-test)

### Doc example update

Append a streaming example to the `Engine::exec` rustdoc.

### Self-consistency harness

`tests/scripts/engine_stream_consistency_check.sh` drives `engine_stream_diff`
(new Rust example binary). The harness runs each fragment twice — once with
no callback, once with a string-accumulating callback — and asserts that the
joined-with-`\n` callback transcript equals `Output.stdout` from the
no-callback run. ~6 fragments covering builtin-only, external-only,
pipelines, redirects, merge_stderr.

This proves the streaming callback view and the buffer view agree on the
byte sequence — the only correctness property we can self-check without
involving bash.

### CLI byte-identical gate

All 128 existing harnesses + v205 + v206 still pass. `cargo test --workspace`
count == baseline + only the new tests.

### Workspace gates

- `cargo test --workspace --quiet` green.
- `cargo test --workspace --doc --quiet` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo build --release --workspace` clean.

### Platform sanity

Linux + macOS supported; gated by `#[cfg(target_os = ...)]`. The platform
module's tests run only on the matching target. Other targets get a
`compile_error!` at module-import time.

## Risks & mitigations

- **SIGCHLD masking interacts with existing signal-hook flag dispatch.**
  v138's signal_hook-based SIGINT/SIGCHLD flags use `sigaction` with no
  blocking. signalfd needs SIGCHLD blocked. Mitigation: mask SIGCHLD only
  within `WaitLoop`'s lifetime (RAII guard), restore on Drop. Existing flag
  dispatch still fires on SIGCHLD for non-WaitLoop paths (e.g. background
  jobs). Verify with a job-control test.
- **Poll-loop refactor across 3 fork sites is invasive.** Mitigate: a single
  `external_capture_loop(child, callbacks, ...)` helper shared by all three.
  Each site's diff is just "replace blocking wait with helper call."
- **Callback panic during partial line accumulation could orphan resources.**
  Mitigate: all per-call state (LineBuf, capture buffer, WaitLoop) lives in
  stack-allocated `Drop`-implementing structs. Unwind drops them in reverse
  order; pipes close; child gets EPIPE on next write; SIGTERM-on-timeout still
  applies if .timeout() set.
- **kqueue on macOS has a different signal-event contract than signalfd.**
  Mitigate: separate `#[cfg]` implementations of `WaitLoop`, each with its own
  platform-appropriate idioms. CI matrix on both targets.
- **Long-line OOM via runaway producer.** Mitigate: documented — `.timeout()`
  is the user's escape hatch; we don't impose a `LineBuf` cap. Adding one would
  require a "line truncated" callback signal that complicates the contract.

## Out of scope

- Byte-level callbacks (`.on_stdout_bytes`).
- A dedicated `.stream()` terminal method.
- Stop-the-script-from-callback (no `Result` / `bool` return).
- `async fn` callback support / `Future` integration.
- Backpressure beyond kernel pipe + `.timeout()`.
- Windows platform support.
- Completion API (deferred to v208).
- `Engine: Send` refactor.
- crates.io publish / semver freeze.

## Task decomposition (for the plan)

1. **`LineBuf`** — pure-stdlib accumulator + 6 unit tests.
2. **`WaitLoop`** — `wait_loop.rs` with Linux/macOS impls + 4 platform-gated
   unit tests.
3. **`ExecBuilder` fields + methods** — `on_stdout_line` / `on_stderr_line` +
   compile sanity. No behavior change yet.
4. **Builtin-path dispatch hook** — at `StdoutSink::Capture` write site,
   thread bytes through `LineBuf` and dispatch + 4 unit tests.
5. **External-process poll loop** — replace 3 fork sites' blocking
   `waitpid` + drainer-thread shape with the shared `external_capture_loop`
   helper using `WaitLoop`. + 6 external-path tests.
6. **`run()` tee re-write** — interpose pipe under `.run()` when callbacks are
   set; save real fd 1/2; re-write each line post-callback. + 3 tests.
7. **`ExecBuilder::run_with_sinks` composition** — wire callbacks into the
   v206 composition order; thread through to builtin + external paths. + 5
   composition tests.
8. **Self-consistency harness** — `engine_stream_diff` example binary +
   `engine_stream_consistency_check.sh` + ~6 fragments.
9. **Docs + architecture.md** — paragraph on `on_stdout_line`/`on_stderr_line`,
   `LineBuf`, `WaitLoop`, tee semantics.
