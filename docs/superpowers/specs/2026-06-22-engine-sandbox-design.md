# v206: `Engine::exec` sandbox knobs — cwd + restricted + timeout — Design

**Status:** approved 2026-06-22
**Iteration:** v206
**Builds on:** v205 (`Engine::exec` builder + `ExecBuilder.stdin/.merge_stderr/.run/.capture`)

## Goal

Add three sandbox knobs to `ExecBuilder` so embedders running untrusted or
generated shell can constrain it:

- `.cwd(path)` — chdir for the call, restored on exit.
- `.restricted(true)` — refuse the bash `rbash` subset of escape operations.
- `.timeout(dur)` — abort the script if it hasn't finished within `dur`.

The use case is sandboxed-runner embedders — AI agents running model-generated
shell, test harnesses running fixtures, scripted automation that wraps huck
around inputs of varying trust. v205 gave them an IO story; v206 gives them a
containment story.

## Decisions (from brainstorming)

1. **All three knobs in one iteration.** Coherent sandbox slice.
2. **Per-call builder only.** Same lifetime model as `.stdin()` / `.merge_stderr()`.
   No `Engine::set_cwd`/`set_restricted`/`set_default_timeout`; nothing
   sticky across calls.
3. **Restricted is bool, bash `rbash` subset.** No policy struct in v206; if
   demand surfaces, v207 can add `.restricted(Restrictions { … })` without
   breaking the bool form.
4. **Timeout is cooperative-at-command-boundaries + SIGTERM-in-flight-children
   + exit 124.** Matches GNU `timeout(1)`. Tight loops in external processes
   that ignore SIGTERM die when the parent reaps; not v206's problem.

## Public API

`ExecBuilder` gains three methods; all `Self`-consuming for chain ergonomics.

```rust
impl<'a> ExecBuilder<'a> {
    // ... existing: stdin, merge_stderr, run, capture ...

    /// Run the script with CWD = `path` for the duration of the call. The
    /// process's prior cwd plus `Shell.vars["PWD"]` / `["OLDPWD"]` are
    /// snapshot-and-restored on exit (including panic unwind).
    pub fn cwd(self, path: impl Into<std::path::PathBuf>) -> Self;

    /// Enable restricted mode for this call only (a bash `rbash` subset).
    /// Refused operations write a `huck: restricted: <op>` diagnostic to the
    /// active stderr sink and return exit 1 from the offending builtin;
    /// the script keeps running unless it `set -e`s.
    pub fn restricted(self, on: bool) -> Self;

    /// Abort the script if it hasn't finished within `dur`. Returns exit
    /// 124 on timeout (matches GNU `timeout(1)`). In-flight external children
    /// receive SIGTERM. Builtins finish their current command, then the next
    /// command-boundary `check_interrupt` aborts.
    pub fn timeout(self, dur: std::time::Duration) -> Self;
}
```

`Engine::run(src)` / `Engine::capture(src)` shortcuts unchanged — they
construct an `ExecBuilder` with all three knobs absent (off).

The builder fields under the hood:

```rust
pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
    cwd: Option<PathBuf>,           // NEW
    restricted: bool,               // NEW
    timeout: Option<Duration>,      // NEW
}
```

## Semantics

### `.cwd(path)`

- Snapshots OS cwd via `env::current_dir()` (None if unreachable) and
  `Shell.vars["PWD"]` + `["OLDPWD"]`.
- `env::set_current_dir(path)` runs. Failure prints `huck: cwd: <path>:
  <err>` to stderr and the script runs anyway with the embedder's original
  cwd — best-effort, same posture as `with_stdin_fd0` in v205.
- The shell's `PWD` is set to `path` (canonicalized when possible — fall back
  to the input path on failure); `OLDPWD` is set to the prior `PWD`. This
  matches bash's `cd` builtin's behavior for these variables, so a script
  that reads `$PWD` sees the sandbox path.
- The script may itself `cd` during the call; those `cd`s update `PWD`/
  `OLDPWD`/OS cwd normally.
- On call exit, the RAII guard restores OS cwd (via `env::set_current_dir`)
  and overwrites `Shell.vars["PWD"]` / `["OLDPWD"]` to the snapshotted
  values. The Engine's persistent session does NOT see the script's
  intra-call `cd` movements.
- All other shell state mutations (vars set, functions defined, `$?`)
  persist into the Engine — same as v205.
- Process-global: `chdir` is process-global. Engine's `!Send + !Sync`
  contract prevents two concurrent chdirs. Tests gate on `test_support::CWD_LOCK`.

### `.restricted(true)`

A new `Shell.restricted: bool` field (default `false`). Snapshot-and-restore
around the call. While `true`, the following are refused with a
`huck: restricted: <op>` diagnostic to the active stderr sink and exit 1
from the offending builtin / command:

| Restriction | Enforcement site | Diagnostic |
|---|---|---|
| `cd` | `builtin_cd` head | `huck: restricted: cd` |
| `exec` | `run_exec_single` exec-builtin branch | `huck: restricted: exec` |
| Command name containing `/` | `run_simple_command` after PATH resolution | `huck: restricted: <name>: restricted` |
| `.` / `source` of path containing `/` | `builtin_source` head | `huck: restricted: source: paths with '/'` |
| Redirect target absolute OR contains `..` | `RedirectScope::apply` for write redirects (`>`, `>>`, `>\|`, `<>`, `&>`, `&>>`) | `huck: restricted: <path>` |
| Assignment to `SHELL`/`PATH`/`ENV`/`BASH_ENV` | `Shell::set` head; also `apply_one_assignment` for inline assigns | `huck: restricted: <name>: readonly variable` |
| `set +r` | `builtin_set` `+r` arm | `huck: restricted: cannot turn off restricted mode` |

Inheritance:
- **Subshells** (`( … )`) inherit `Shell.restricted` (subshell-fork already
  inherits all shell state).
- **Functions** called during the restricted run inherit it.
- **`source`d files** inherit it.
- **Once set, can't be turned off mid-call** — `set +r` is on the refusal list.

The redirect check is syntactic — absolute paths OR `..` components are
rejected without canonicalization. Strict (rejects `/tmp/x` even if `/tmp/x`
is inside cwd via symlink), avoids any TOCTOU canonicalization race, matches
bash rbash.

The restricted-refusal of a builtin's own operation returns exit 1 from that
builtin; the script CONTINUES (unless `set -e`). The script's final exit
status is whatever it would otherwise have been, NOT a single "1 because
something was refused". A persistent embedder can detect refusal by
inspecting `Output.stderr` for `restricted:` prefix lines, or via overall
nonzero exit if the script propagates the failure.

### `.timeout(dur)`

A new `Shell.timeout_flag: Arc<AtomicBool>` field (default a fresh `AtomicBool::new(false)`).
Plus a per-call PID registry: `Shell.live_external_children: Arc<Mutex<Vec<libc::pid_t>>>`
populated at every fork site and drained on `waitpid` success.

On `.timeout(dur)`, the ExecBuilder:

1. Clones `Arc<AtomicBool>` for the timer.
2. Spawns a timer thread via `timeout::spawn_timer(dur, flag, pids)` that
   `recv_timeout`s on a cancel channel. On timeout (`Err(RecvTimeoutError::Timeout)`),
   sets the flag AND iterates the PID registry sending `SIGTERM` to each.
3. Receives a `TimerHandle { handle, cancel_tx }`.
4. After the script returns, sends on `cancel_tx` (unblocks `recv_timeout`
   immediately) and `join`s the handle.
5. If `timeout_flag.swap(false, Ordering::Relaxed) == true`, the call returns
   exit 124, overriding whatever the interrupted script's natural exit code
   was.

The existing `executor::check_interrupt(shell)` polls `sigint_flag` at every
command boundary. Extend it to also poll `timeout_flag`. Both reach the
top-level `ExecOutcome::Interrupted { reason: Reason }`. The reason
discriminates: SIGINT → exit 130 (today's behavior); Timeout → exit 124.

`ExecOutcome::Interrupted` widens:

```rust
pub enum ExecOutcome {
    // ... existing variants ...
    Interrupted { reason: InterruptReason },
}

pub enum InterruptReason {
    Sigint,   // Ctrl-C (today's `Interrupted` cases)
    Timeout,  // v206
}
```

Every existing site producing `Interrupted` becomes `Interrupted { reason:
Sigint }`. The new timeout path produces `Interrupted { reason: Timeout }`.
The top-level run loop maps reasons to exit codes.

PID registry maintenance:
- Every `fork()` / `posix_spawn` / `ProcessCommand::spawn` success path
  pushes the child pid onto `Shell.live_external_children`.
- Every `waitpid` success removes that pid.
- On timeout, the timer thread iterates and `libc::kill(pid, SIGTERM)`s each.
  Children's death wakes the parent's `waitpid`; the executor then notices
  `timeout_flag` and aborts.

The cancel channel is critical: a finished-before-deadline script must not
leave a dangling timer thread sleeping out the full duration. The cancel
send wakes `recv_timeout`; the thread returns; `join()` reaps.

### Composition and order-of-operations

`ExecBuilder::run_with_sinks` performs setup in this fixed order:

1. Build `StdoutSink` / `StderrSink` from `merge`.
2. Spawn timer (if `timeout`); obtain `TimerHandle`.
3. Acquire `with_cwd(path)` guard (if `cwd`) — snapshot OS cwd + shell's
   `PWD`/`OLDPWD`, chdir to `path`.
4. Acquire `with_stdin_fd0(bytes)` guard (if `stdin`) — dup-replace fd 0.
5. Snapshot `Shell.restricted`; set to `true` if builder's `restricted`.
6. Run the script via `shell::run_program_in_sinks`.
7. Restore `Shell.restricted` to snapshot.
8. Stdin guard drops — fd 0 restored.
9. Cwd guard drops — OS cwd restored, `Shell.vars["PWD"]` / `["OLDPWD"]`
   restored.
10. Timer cancel + join.
11. If `timeout_flag.swap(false)` was `true`, override exit code to 124.

The builder methods themselves (`stdin`, `merge_stderr`, `cwd`, `restricted`,
`timeout`) can be called in any order; they just set fields on `ExecBuilder`.

### Exit codes

| Situation | Exit |
|---|---|
| Script exits normally / `exit N` | N |
| Parse error | 2 |
| Timeout fired | **124** (override) |
| Restricted op refused (per refusal) | 1 from that builtin; script may continue |
| `cwd` chdir failed | Script runs with embedder's cwd; whatever the script returns |
| Stdin pipe setup failed | Same as v205 |
| SIGINT during run | 130 (today's behavior, unchanged) |

### Reentrancy

Unchanged from v205. `ExecBuilder` borrows `&mut Engine`; nested builders
prevented at compile time. Script callbacks reaching back into Engine are out
of scope (no such path exists).

### Thread safety

`Shell.timeout_flag` is `Arc<AtomicBool>`; `Shell.live_external_children` is
`Arc<Mutex<Vec<libc::pid_t>>>`. These ARE accessed cross-thread by the timer
thread. Everything else on `Shell` remains non-`Send`/non-`Sync`; the Engine
contract is unchanged.

## Internal architecture

### New module: `crates/huck-engine/src/cwd_scope.rs` (~70 LOC)

```rust
pub fn with_cwd<R>(
    path: &Path,
    shell: &mut Shell,
    f: impl FnOnce(&mut Shell) -> R,
) -> R {
    let saved_os = std::env::current_dir().ok();
    let saved_pwd = shell.lookup_var("PWD");
    let saved_oldpwd = shell.lookup_var("OLDPWD");

    match std::env::set_current_dir(path) {
        Ok(_) => {
            let canonical = std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| path.display().to_string());
            if let Some(prev) = &saved_pwd {
                shell.set("OLDPWD", prev.clone());
            }
            shell.set("PWD", canonical);
        }
        Err(e) => {
            eprintln!("huck: cwd: {}: {e}", path.display());
            // Best-effort: run anyway. No guard installed (nothing to restore).
            return f(shell);
        }
    }

    struct Restore {
        saved_os: Option<PathBuf>,
        saved_pwd: Option<String>,
        saved_oldpwd: Option<String>,
        shell: *mut Shell,   // see SAFETY
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(p) = &self.saved_os {
                let _ = std::env::set_current_dir(p);
            }
            // SAFETY: shell pointer is borrowed live for f's lifetime;
            // Drop runs before with_cwd returns, while the borrow is valid.
            let shell = unsafe { &mut *self.shell };
            match &self.saved_pwd {
                Some(v) => shell.set("PWD", v.clone()),
                None => shell.unset("PWD"),
            }
            match &self.saved_oldpwd {
                Some(v) => shell.set("OLDPWD", v.clone()),
                None => shell.unset("OLDPWD"),
            }
        }
    }
    let _restore = Restore {
        saved_os,
        saved_pwd,
        saved_oldpwd,
        shell: shell as *mut Shell,
    };
    f(shell)
}
```

The `*mut Shell` workaround is needed because the closure consumes the
`&mut Shell` borrow but Drop wants it back. The pointer is valid for the
lifetime of `with_cwd`'s frame; documented in a SAFETY block.

(Alternative: structure `with_cwd` so the closure runs with a separate
shorter-lived borrow, leaving `&mut Shell` free for Drop. This is more
idiomatic but adds plumbing. Pick whichever pattern reads cleanest in
implementation — both are sound.)

### New module: `crates/huck-engine/src/restricted.rs` (~150 LOC)

The seven check functions listed in Semantics. Each is short (5-15 LOC):
takes the relevant input, returns `Result<(), String>`. Caller writes the
diagnostic via the existing `e!`/`with_err` macro and returns the appropriate
exit code. No new error types.

```rust
pub fn is_restricted(shell: &Shell) -> bool { shell.restricted }

pub fn check_cd() -> Result<(), &'static str> {
    Err("huck: restricted: cd")
}

pub fn check_exec() -> Result<(), &'static str> {
    Err("huck: restricted: exec")
}

pub fn check_command_name(name: &str) -> Result<(), String> {
    if name.contains('/') {
        Err(format!("huck: restricted: {name}: restricted"))
    } else { Ok(()) }
}

pub fn check_source_path(path: &str) -> Result<(), &'static str> {
    if path.contains('/') {
        Err("huck: restricted: source: paths with '/'")
    } else { Ok(()) }
}

pub fn check_redirect_path(path: &str) -> Result<(), String> {
    if path.starts_with('/') || path.split('/').any(|c| c == "..") {
        Err(format!("huck: restricted: {path}"))
    } else { Ok(()) }
}

pub fn check_special_assign(name: &str) -> Result<(), String> {
    if matches!(name, "SHELL" | "PATH" | "ENV" | "BASH_ENV") {
        Err(format!("huck: restricted: {name}: readonly variable"))
    } else { Ok(()) }
}

pub fn check_set_plus_r() -> Result<(), &'static str> {
    Err("huck: restricted: cannot turn off restricted mode")
}
```

Each check is called from a guarded site that uses `is_restricted(shell)` as
the gate. Builtins use their existing `err: &mut dyn Write` to emit:

```rust
if restricted::is_restricted(shell) {
    if let Err(msg) = restricted::check_command_name(name) {
        e!(err, "{msg}");
        return ExecOutcome::Continue(1);
    }
}
```

### New module: `crates/huck-engine/src/timeout.rs` (~100 LOC)

```rust
pub fn spawn_timer(
    deadline: Duration,
    flag: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<libc::pid_t>>>,
) -> TimerHandle {
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        match cancel_rx.recv_timeout(deadline) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {} // cancelled
            Err(RecvTimeoutError::Timeout) => {
                flag.store(true, Ordering::Relaxed);
                if let Ok(guard) = pids.lock() {
                    for &pid in guard.iter() {
                        unsafe { libc::kill(pid, libc::SIGTERM); }
                    }
                }
            }
        }
    });
    TimerHandle { handle, cancel_tx }
}

pub struct TimerHandle {
    handle: std::thread::JoinHandle<()>,
    cancel_tx: std::sync::mpsc::Sender<()>,
}

impl TimerHandle {
    pub fn cancel(self) {
        let _ = self.cancel_tx.send(());
        let _ = self.handle.join();
    }
}
```

### `ExecOutcome::Interrupted` widening

```rust
pub enum InterruptReason {
    Sigint,
    Timeout,
}

pub enum ExecOutcome {
    // ... other variants ...
    Interrupted(InterruptReason),
}
```

Every existing `ExecOutcome::Interrupted` site (in `executor.rs`) updates to
`ExecOutcome::Interrupted(InterruptReason::Sigint)`. The new timeout-flag
poll in `check_interrupt` produces `Interrupted(InterruptReason::Timeout)`.
The top-level reducer in `run_program_in_sinks` maps:

```rust
ExecOutcome::Interrupted(InterruptReason::Sigint) => 130,
ExecOutcome::Interrupted(InterruptReason::Timeout) => 124,
```

### PID registry plumbing

Three executor fork sites (from v205 Task 5: `run_subprocess`,
`Command::Subshell`, `run_multi_stage`) gain a small wrapper:

- Just after a successful spawn / fork that returns a child pid, push to
  `shell.live_external_children.lock().unwrap().push(pid)`.
- After the corresponding `waitpid` returns success, remove that pid.
- The lock is held only briefly; no nesting.

For pipelines, all stage pids land in the registry; on timeout, every stage
gets SIGTERM in one pass.

### `Shell` field additions

```rust
pub struct Shell {
    // ... existing fields ...
    pub restricted: bool,
    pub timeout_flag: Arc<AtomicBool>,
    pub live_external_children: Arc<Mutex<Vec<libc::pid_t>>>,
}

impl Shell {
    pub fn new() -> Self {
        Shell {
            // ...
            restricted: false,
            timeout_flag: Arc::new(AtomicBool::new(false)),
            live_external_children: Arc::new(Mutex::new(Vec::new())),
        }
    }
}
```

## CLI dogfood

No CLI changes. `Engine::run`/`run_file` keep v204/v205 behavior. The CLI
binary doesn't expose v206's sandbox knobs; they're embedder-only.

## Build / packaging

No new crates, no new external deps. All additions are in `huck-engine`. CLI
crate untouched. Release build path unchanged.

## Testing & verification

### Unit tests (`crates/huck-engine/src/engine.rs::mod tests`)

**Cwd (4 tests):**

- `exec_cwd_runs_script_in_path` — `.cwd(tmpdir).capture("pwd")` returns the
  tmpdir's canonical path + `\n`.
- `exec_cwd_restores_engine_pwd` — set `PWD=before`; `.cwd(tmp).capture("cd
  /; echo $PWD")` returns `/\n` from the call; then `engine.var("PWD")` is
  `"before"`.
- `exec_cwd_chdir_failure_is_best_effort` — `.cwd("/no/such").capture("echo
  hi")` → `Output.stdout = "hi\n"`, `Output.stderr` contains `huck: cwd:`,
  `Output.exit_code = 0`.
- `exec_cwd_serializes_with_lock` — gated on `CWD_LOCK`, just structural.

**Restricted (12 tests, one per refusal + propagation):**

- `restricted_refuses_cd`
- `restricted_refuses_exec`
- `restricted_refuses_command_name_with_slash`
- `restricted_refuses_source_with_slash`
- `restricted_refuses_absolute_redirect`
- `restricted_refuses_parent_dir_redirect`
- `restricted_refuses_special_var_assignment` (each of SHELL/PATH/ENV/BASH_ENV)
- `restricted_refuses_set_plus_r`
- `restricted_off_by_default`
- `restricted_propagates_to_subshell`
- `restricted_propagates_to_function`
- `restricted_lifts_after_call`

Each verifies: exit code from the refused builtin = 1, `Output.stderr`
contains `huck: restricted:`, and the side effect (file written, cwd
changed, var set) DID NOT happen.

**Timeout (6 tests):**

- `timeout_kills_infinite_loop` — `.timeout(100ms).capture("while true; do
  :; done")` → exit 124, elapsed ≤ 500ms.
- `timeout_short_script_completes_normally` — `.timeout(5s).capture("echo
  hi")` → exit 0, stdout `"hi\n"`, returns immediately.
- `timeout_kills_sleeping_external` — `.timeout(100ms).capture("/bin/sleep
  5")` → exit 124, elapsed ≤ 500ms (SIGTERM-then-waitpid).
- `timeout_exit_code_overrides_natural_exit` — script that would naturally
  exit 0 but is timed out → 124.
- `timeout_zero_duration` — `.timeout(Duration::ZERO).capture("echo hi")` →
  exit 124, stdout empty.
- `timeout_thread_does_not_leak` — 50 back-to-back `.timeout(10s)` calls all
  completing in < 1s each (no dangling threads).

**Composition (5 tests):**

- `cwd_and_restricted` — both knobs, `pwd` works but `cd /tmp` is refused.
- `cwd_restricted_blocks_escape` — `.cwd(tmp).restricted(true).capture("echo
  hi > /tmp/x")` is refused even if the implementation could in principle
  write.
- `stdin_with_timeout_short` — input feeds correctly; finishes well within
  the timeout.
- `stdin_with_timeout_blocking_read` — empty stdin + timeout → `read` blocks
  → 124.
- `all_knobs` — `.cwd().restricted().timeout().stdin().merge_stderr().capture(...)`
  composes correctly.

### Doc example update

Append to the `Engine::exec` rustdoc:

```rust
//! // Sandboxed run: tmpdir cwd, restricted mode, 5s budget.
//! let out = e.exec(generated_script)
//!     .cwd(sandbox_dir)
//!     .restricted(true)
//!     .timeout(std::time::Duration::from_secs(5))
//!     .capture();
```

### Bash-diff harness

New `tests/scripts/engine_sandbox_diff_check.sh` driving a sibling driver
binary `engine_sandbox_diff` that takes additional argv: cwd path, restricted
flag, timeout ms. ~8 fragments where huck under restricted should match `bash
--restricted -c '…'`:

- `restricted_cd_refused` — `cd /tmp` under restricted.
- `restricted_exec_refused`
- `restricted_slash_command` — `/bin/echo hi`.
- `restricted_source_with_slash` — `. /etc/profile`.
- `restricted_redirect_absolute` — `echo hi > /tmp/x`.
- `restricted_assign_PATH` — `PATH=/tmp`.
- `unrestricted_baseline` — same fragments without `.restricted(true)`,
  expect bash-success match.
- `cwd_pwd_matches` — `.cwd(tmp)`, fragment `pwd`, bash run as
  `bash -c 'cd $tmp; pwd'`.

Some bash rbash behaviors diverge by design (e.g., bash refuses `enable -f`;
huck has no loadables — moot). Skip those with a comment.

### CLI byte-identical gate

All 128 existing harnesses (127 pre-v206 + v205's `engine_capture_diff_check.sh`)
still pass. `cargo test --workspace` count == baseline plus only the new tests.

### Workspace gates

- `cargo test --workspace --quiet` — green.
- `cargo test --workspace --doc --quiet` — doc example passes.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo build --release --workspace` — clean.

## Risks & mitigations

- **Restricted-refusal sites are scattered.** Seven enforcement points
  across builtins, executor redirects, and assignment paths. Risk: missing a
  site. Mitigate: the explicit per-site table in Semantics IS the audit
  list; each test verifies one site; the bash-diff harness cross-checks.
- **PID registry maintenance must be exhaustive.** Every successful fork
  must push; every successful waitpid must pop. Risk: an exit path that
  forgets to pop leaks pids (timer thread sends SIGTERM to a stale pid that
  may now be a different process). Mitigate: wrap the fork-success-to-waitpid
  region in an RAII guard that pops on drop.
- **`Shell.restricted` snapshot/restore must run even on panic.** Mitigate:
  RAII guard, same shape as cwd / stdin guards.
- **Cwd RAII restore order vs. stdin/timer guards.** The drop order matters:
  stdin must restore fd 0 before the cwd guard restores OS cwd (a script
  that wrote to a file via fd 0 indirection... unlikely, but ordering should
  be deterministic). The fixed order in Semantics step 8/9 sets this via
  declaration order (stdin guard declared LAST → dropped FIRST).
- **Timer thread + script panic.** If the script panics, the timer thread is
  still sleeping. Mitigate: the `TimerHandle::cancel` in step 10 doesn't run
  on panic, but the cancel channel sender drops (thread sees `Disconnected`
  and returns). Thread is reaped naturally; no leak. Documented.
- **Restricted's redirect check is syntactic.** Symlink escapes possible
  (relative path `foo` where `foo` is a symlink to `/etc/passwd`). Same as
  bash rbash. Documented as intentional.
- **Timer thread firing during stdin writer thread's write.** Both threads
  can be alive concurrently. Stdin writer doesn't touch the timeout flag.
  Timer doesn't touch the stdin pipe. They don't interact.
- **`Shell.live_external_children` lock contention.** Held only briefly at
  fork/waitpid boundaries and during timer-thread SIGTERM iteration. No
  blocking workload inside the lock.

## Out of scope

- Persistent `Engine::set_cwd` / `set_restricted` / `set_default_timeout`.
  Per-call only.
- A `Restrictions { … }` policy struct. `.restricted(true)` is a fixed
  rbash subset.
- Symlink-aware redirect-path canonicalization.
- Hard SIGKILL fallback after grace period.
- Resource limits (memory, fd, fork bomb).
- `.env(...)` per-call environment overrides.
- `.chroot(...)`.
- Custom builtin allow/deny lists.
- Full Engine snapshot/restore around sandboxed calls.
- `Result`-typed errors.
- Stable semver / crates.io publish.

## Task decomposition (for the plan)

1. Add `Shell` fields (`restricted: bool`, `timeout_flag: Arc<AtomicBool>`,
   `live_external_children: Arc<Mutex<Vec<libc::pid_t>>>`) + `Default`
   updates. Suite green (no behavior change).
2. Add `cwd_scope.rs` with `with_cwd(path, shell, f)` + unit tests.
3. Add `restricted.rs` with the seven check functions + thread them into the
   enforcement sites + per-site unit tests.
4. Widen `ExecOutcome::Interrupted` to `Interrupted(InterruptReason)`;
   update all existing sites to `InterruptReason::Sigint`. Suite green.
5. Add `timeout.rs` (timer + cancel channel) + extend `check_interrupt` to
   poll `timeout_flag` + plumb PID registry into the three fork sites + unit
   tests.
6. Extend `ExecBuilder` with `.cwd()`, `.restricted()`, `.timeout()`; thread
   through `run_with_sinks` in the fixed setup order; composition tests.
7. Bash-diff harness `engine_sandbox_diff_check.sh` + Rust driver.
8. Verify: full suite + clippy + release build + all 129 harnesses green;
   update `docs/architecture.md` with the three new knobs.
