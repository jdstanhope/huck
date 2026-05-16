# shuck Job Control Design (Sub-project A: background jobs)

**Status:** Draft (2026-05-16)

**Goal:** Add trailing-`&` background execution to shuck, with a job table, `jobs` and `wait` builtins, SIGCHLD-driven reaping, and bash-style completion notifications before the next prompt.

**Scope (v6, sub-project A):**
- In: `cmd &` and `cmd1 && cmd2 &` (whole-sequence backgrounding); job table with lowest-available ID reuse; `jobs` builtin; `wait` (no-args) builtin; SIGCHLD handler + synchronous reaping at prompt and inside `wait`; `[N] Done <cmd>` notifications before prompt; background jobs in their own process group; `/dev/null` stdin for background commands without explicit `<` redirect.
- Out (deferred to sub-project B): `fg`/`bg`, Ctrl-Z suspension, terminal control via `tcsetpgrp`, stopped-job state.
- Out (deferred to sub-project C): `disown`, `kill %N`, job specifiers (`%N`, `%+`, `%-`, `%string`), `wait %N` and `wait PID`.
- Out: `cmd1 & cmd2` (full bash list-terminator semantics — only trailing `&` is supported).

---

## 1. Syntax & AST

**Lexer changes:**
- `&` (when NOT immediately followed by another `&`) becomes `Operator::Background`. Previously this produced `LexError::BareAmpersand`; that variant is removed.
- `&&` continues to produce `Operator::And` (unchanged).

**Parser changes:** `Sequence` gains a `background: bool` flag:

```rust
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
    pub background: bool,  // NEW
}
```

- A trailing `Operator::Background` at the end of a sequence sets `background = true` and is otherwise consumed (no extra Pipeline added).
- `cmd1 && cmd2 &` parses as `Sequence { first: cmd1, rest: [(And, cmd2)], background: true }`. The whole `cmd1 && cmd2` chain is backgrounded as one job.
- `Operator::Background` in any non-terminal position is a parse error.
- New `ParseError::UnexpectedBackground` variant for the non-terminal case.

**Parse-time rejections (all errors, return `Continue(2)`):**

| Input | Result |
|-------|--------|
| `cmd &` | `Sequence { first: cmd, rest: [], background: true }` ✓ |
| `cmd1 \| cmd2 &` | `Sequence { first: Pipeline[cmd1, cmd2], rest: [], background: true }` ✓ |
| `cmd1 && cmd2 &` | `ParseError::BackgroundedMultiPipelineSequence` (see §3 — backgrounding a multi-command chain would require forking the shell, deferred) |
| `cmd1 ; cmd2 &` | `ParseError::BackgroundedMultiPipelineSequence` (same restriction) |
| `cmd1 & cmd2` | `ParseError::UnexpectedBackground` (mid-sequence `&` not supported) |
| `& cmd` | `ParseError::MissingCommand` (existing) |
| `&` alone | `ParseError::MissingCommand` (existing) |
| `cmd & &` | `ParseError::UnexpectedBackground` (second `&` after the trailing one) |

**Quoted/escaped `&`** remains literal — `echo "&"` and `echo \&` continue to produce a Word with literal `&`. Unchanged from v3.

**Inside command substitution:** `&` is allowed in `$(...)` and `` `...` `` bodies. The inner Sequence's `background: true` is parsed normally, but `execute_capturing` (used by `run_substitution`) ignores the flag — the substitution always waits for the captured command to finish (it has to, to capture stdout). Documented as "no effect"; matches bash semantically (subshell backgrounding inside a command substitution is meaningless because the parent waits for the subshell anyway).

---

## 2. Job table

A new `src/jobs.rs` module defines:

```rust
#[derive(Debug, Clone)]
pub enum JobState {
    Running,
    Done(i32),        // last stage's exit code (>= 0)
    Signaled(i32),    // last stage killed by signal N
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    pub pgid: i32,                       // == first stage's pid
    pub pids: Vec<i32>,                  // all stages, in pipeline order
    pub reaped: Vec<bool>,               // parallel with pids; true once that pid is reaped
    pub last_status: Option<i32>,        // last stage's raw waitpid status (set on reap)
    pub command: String,                 // for display
    pub state: JobState,
    pub notified: bool,                  // already printed Done message?
    pub created_at: u64,                 // monotonic counter for + / - markers
}

#[derive(Debug, Clone)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_created_at: u64,
}
```

`JobTable` API:

```rust
impl JobTable {
    pub fn new() -> Self;
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32;
    pub fn iter(&self) -> impl Iterator<Item = &Job>;
    pub fn has_running(&self) -> bool;
    /// Marks `pid` as reaped with the given raw waitpid status. If the pid is
    /// the LAST stage of its job, also stores `last_status`. When all pids of
    /// the job are reaped, transitions state from Running to Done(N) or
    /// Signaled(N) using `last_status`.
    pub fn reap(&mut self, pid: i32, raw_status: i32);
    /// Returns jobs that are non-Running AND not yet notified, in id order.
    /// Marks them notified (so the next call returns nothing for the same jobs).
    pub fn drain_notifications(&mut self) -> Vec<Job>;
    /// Removes all notified, non-Running jobs from the table.
    pub fn remove_notified(&mut self);
    /// Adds a synthetic Done(0) job for a pure-builtin pipeline.
    pub fn add_synthetic_done(&mut self, command: String, exit: i32) -> u32;
}
```

**ID allocation (`add`):** scan IDs 1, 2, 3... and return the first not currently in `jobs`. This matches bash's reuse semantics.

**`+` and `-` markers:** computed at display time. Sort jobs by `created_at` descending; the first is `+`, the second is `-`, others are blank.

**State decoding:** `reap()` uses the libc macros `WIFEXITED(raw)`, `WEXITSTATUS(raw)`, `WIFSIGNALED(raw)`, `WTERMSIG(raw)` to decode the raw status. If exited normally, store `Done(WEXITSTATUS)`; if signaled, store `Signaled(WTERMSIG)`. The job's overall state transitions when all pids in the job have been reaped.

**`Shell` integration:** `Shell` gains `pub jobs: JobTable`. Since `Shell: Clone` (from v5), `JobTable` derives `Clone`. **Subshell isolation**: `run_substitution` clones the Shell; any jobs added to the clone vanish when the substitution ends. (We do not reap them; they continue running as orphans, but the parent's job table doesn't see them. Matches bash semantics for jobs registered inside `$(...)`.)

**Capturing the command string for display:** the REPL has the user's input line as a `String`. `process_line` passes the line through `executor::execute` via a new `source: &str` parameter. When the executor takes the background path, it stores the trimmed source (with trailing `&` and whitespace stripped) as the job's `command`. The notifier re-adds a trailing `&` per the format in §4. This avoids needing a Sequence-to-text renderer.

---

## 3. Process groups & spawning

`execute_inner` (in `src/executor.rs`) checks `seq.background`. If true, calls a new `run_background_sequence` path; otherwise the existing foreground path.

**Foreground (`seq.background == false`):** unchanged. Children inherit the shell's process group. Shell waits in the existing wait loop.

**Background (`seq.background == true`):** the whole sequence — including any `&&`/`||`/`;` chain — runs in a forked-and-detached form. But we don't actually fork the shell; instead, we serialize the sequence's commands but use process groups to detach the spawned children. Concretely:

For a backgrounded sequence consisting of a single pipeline (the common case `cmd &` or `cmd | cmd2 &`):
- First stage: `Command::process_group(0)` — child becomes leader of a new pg (pgid == own pid).
- Subsequent stages: spawn with `Command::process_group(first_stage_pid)` (we know it because we've already spawned stage 0 to wire up the stdin pipe for stage 1).
- Stdin for the first stage: if `cmd.stdin` is None, redirect to `/dev/null` (`File::open("/dev/null")?`). Otherwise honor the explicit `<` file.
- Stdout/stderr: same as foreground (explicit `>` honored; otherwise inherit terminal). **Note:** background stdout to the terminal CAN interleave with the shell's prompt output. That's bash behavior too; the user is expected to redirect if they don't want that.
- After spawning all stages, the executor does **not** wait. It builds a `Job` with the pgid, pids list, and command-string, calls `shell.jobs.add(...)`, and prints `[N] <last_pid>` to stderr.
- Returns `ExecOutcome::Continue(0)` — `$?` after `cmd &` is 0.

For a backgrounded sequence with multiple `&&`/`||`/`;`-connected pipelines (`cmd1 && cmd2 &`): we currently can't background a full sequence-with-branching without forking the shell. Two options:

**Option 1 (taken):** Restrict `&` to backgrounding a single pipeline. If `seq.background == true` AND `seq.rest.is_empty() == false`, the parser rejects with a new `ParseError::BackgroundedMultiPipelineSequence` ("background `&` only supported on a single pipeline; use a subshell for `cmd1 && cmd2 &`"). This is a real restriction vs bash.

**Option 2:** Fork the shell process for `&` so the child runs the whole sequence and exits. More bash-faithful but adds fork+exec gymnastics and requires duplicating signal/REPL setup.

We go with **Option 1** for sub-project A. `cmd1 && cmd2 &` is rejected at parse time with a clear error. (Sub-project B or C can add a `run_in_subshell` primitive later.)

**Revised parser rule:** `Operator::Background` is only accepted as a terminator when `seq.rest.is_empty()` AND we're parsing a single pipeline. If there's been any `&&`/`||`/`;` connector, hitting `Background` at the end produces `ParseError::BackgroundedMultiPipelineSequence`.

**Pure-builtin pipeline with `&`:** if every stage of the pipeline is a builtin, run synchronously in the parent shell (so side effects like `cd` propagate). Register a synthetic `Done(exit_code)` job via `JobTable::add_synthetic_done`. Print `[N] Done <cmd>` to stderr immediately. Documented as bash divergence.

**Pipeline-stage process_group sequencing:** The first stage is spawned with `process_group(0)`. We then read its pid. Subsequent stages are configured with `process_group(first_pid)` before spawn. This uses `std::os::unix::process::CommandExt::process_group` (stable since Rust 1.64).

---

## 4. SIGCHLD handler + reaping

**Handler installation:** mirror the existing SIGINT install pattern in `src/shell.rs::install_sigint_handler`. Add a parallel `install_sigchld_handler` that uses `signal_hook::flag::register(SIGCHLD, flag)` to set an `Arc<AtomicBool>`. The flag is stored on the `Shell` struct (or wrapped in `Arc` and shared) so the reap routine can check + reset it.

`Shell` gains: `pub sigchld_flag: Arc<AtomicBool>`. Initialized to `false` by `Shell::new`. The handler is installed by `shell::run` once at startup.

**Reap routine** (in `src/jobs.rs`):

```rust
pub fn reap_completed(shell: &mut Shell) {
    shell.sigchld_flag.store(false, Ordering::Relaxed);
    loop {
        let mut raw_status: libc::c_int = 0;
        let pid = unsafe { libc::waitpid(-1, &mut raw_status, libc::WNOHANG) };
        if pid <= 0 {
            // 0 → no children changed state; -1 → no children at all (ECHILD)
            break;
        }
        shell.jobs.reap(pid as i32, raw_status);
    }
}
```

Called from:
1. **REPL loop** in `src/shell.rs::run` — right before each `editor.readline(PROMPT)` call. After the reap, drain notifications and print them to stderr; remove notified jobs from the table.
2. **`wait` builtin** — see Section 5.

**Why also call at every prompt** (not just when the SIGCHLD flag is set): defensive. If SIGCHLD was missed due to a race between the handler and the flag-clear, the prompt-time call catches up. The cost is one `waitpid(WNOHANG)` per prompt, which is cheap.

**Notification format** (printed to stderr, before the next prompt):
- `[1]+ Done                 sleep 5 &`
- `[2]- Exit 1               false &`
- `[3]  Killed (signal 15)   sleep 100 &`

Width is loose — the format string is `"[{id}]{flag} {state:20} {cmd} &\n"` where:
- `flag` is `+` for most-recent, `-` for previous, ` ` (space) for others.
- `state` rendered as `Done` (exit 0), `Exit N` (other exit), or `Killed (signal N)`.
- `cmd` is the stored command string. We always show a trailing `&` to indicate background origin.

The drain happens AFTER reap; notified jobs are removed from the table in the same pass.

---

## 5. Builtins

Two new builtins, added to `src/builtins.rs::is_builtin` and `run_builtin` dispatch.

### `jobs`

Synopsis: `jobs` (no flags in v6).

Behavior:
- For each job in the table, in ID-ascending order, print one line in the notification format from Section 4.
- The state column reflects the live state (Running or terminal). Notification flag is ignored — `jobs` shows everything currently tracked, including jobs that have completed but haven't been notified yet.
- Returns 0 always.
- Does NOT remove notified jobs (that's the prompt path's job).

Note: `jobs` writes to the `out` sink passed to `run_builtin` (so it composes with pipelines and `> file` redirects).

### `wait`

Synopsis: `wait` (no args in v6).

Behavior:
- If `shell.jobs.has_running()` is false, return 0 immediately.
- Otherwise: in a loop, call blocking `libc::waitpid(-1, &mut status, 0)`. For each returned pid, call `shell.jobs.reap(pid, status)`. Stop when `has_running()` is false OR `waitpid` returns -1 with ECHILD.
- After the loop completes, also drain notifications and print them (so the user sees Done lines as part of the `wait` output, not deferred to the next prompt). Remove notified.
- Returns 0 always (POSIX).
- Takes a `shell: &mut Shell` parameter like other builtins; signature: `builtin_wait(args, out, shell) -> ExecOutcome`. If `args` is non-empty, prints `shuck: wait: not yet implemented for arguments` to stderr and returns `Continue(2)`.

---

## 6. Edge cases & error handling

| Case | Behavior |
|------|----------|
| `cmd &` with `cmd` an external | Foreground-equivalent spawn but in own pg; not waited for; registered as Running job. |
| `cmd1 \| cmd2 &` | Both in same new pg; pipeline still wired; registered as one job with two pids. |
| `cmd1 && cmd2 &` | Parser error `BackgroundedMultiPipelineSequence`. (Restriction.) |
| `cd / &` (pure builtin) | Runs synchronously; cd takes effect in parent; synthetic Done(0) job added; `[N] Done` printed. |
| `echo hi &` | Synchronous; prints `hi` to terminal; synthetic Done(0) job; notification. |
| `exit &` | Synchronous; shell exits. (Documented bash divergence.) |
| `cmd & ` (multiple `&`) | Lexer produces two Background ops; parser rejects second one as UnexpectedBackground. |
| `wait` with no jobs | Returns 0 immediately. |
| `wait` while job has multiple pids | Blocking waitpid loops; updates job table on each reap; only returns when all pids of all jobs are reaped. |
| SIGCHLD missed (rare race) | Caught by the prompt-time `reap_completed` call regardless. |
| Job with explicit `>file` redirect | `&` is fine; redirect is honored; only stdin defaults to `/dev/null`. |
| Job inside `$(...)` | Inner Sequence's `background: true` flag is ignored by `execute_capturing` — the substitution always waits. |
| Background command's `$?` | After `cmd &`, parent's `$?` is 0 (the spawn itself succeeded). After `wait`, `$?` is 0 (POSIX). |

**Error messages:**
- `shuck: syntax error: '&' not allowed here` — for `UnexpectedBackground` (mid-sequence).
- `shuck: syntax error: '&' on multi-command sequence not supported; use a single pipeline` — for `BackgroundedMultiPipelineSequence`.
- `shuck: wait: not yet implemented for arguments` — `wait` with args.

---

## 7. Testing

Following the v1–v5 pattern (unit tests per module + smoke tests).

**Lexer tests** (`src/lexer.rs`):
- `tokenize_single_ampersand_is_background_op` — `&` → `Operator::Background`.
- `tokenize_double_ampersand_is_still_and` — `&&` → `Operator::And` (unchanged).
- `tokenize_ampersand_then_ampersand_with_space` — `& &` → two Background ops.
- `tokenize_quoted_ampersand_is_literal` — `"&"` → Literal `&` (regression check).
- `tokenize_escaped_ampersand_is_literal` — `\&` → Literal `&` (regression check).

**Parser tests** (`src/command.rs`):
- `parse_command_with_background` — `cmd &` → `background: true`.
- `parse_background_alone_is_missing_command` — `&` → `MissingCommand`.
- `parse_background_mid_sequence_is_error` — `cmd1 & cmd2` → `UnexpectedBackground`.
- `parse_background_after_andor_is_unsupported` — `cmd1 && cmd2 &` → `BackgroundedMultiPipelineSequence`.
- `parse_background_after_semi_is_unsupported` — `cmd1 ; cmd2 &` → `BackgroundedMultiPipelineSequence`.
- `parse_pipeline_backgrounded` — `cmd1 | cmd2 &` → single pipeline of 2 stages with `background: true`.
- `parse_two_backgrounds_is_error` — `cmd & &` → `UnexpectedBackground`.
- Update all existing parser tests' fixtures to include `background: false`.

**Job-table tests** (`src/jobs.rs`):
- `add_allocates_id_one_first`.
- `add_after_remove_reuses_lowest`.
- `reap_marks_pid_done`.
- `reap_transitions_to_done_when_all_pids_reaped`.
- `reap_with_signal_transitions_to_signaled`.
- `drain_notifications_returns_completed_unnotified` then marks them.
- `remove_notified_drops_completed_only`.
- `has_running_tracks_state`.
- `add_synthetic_done_immediate`.

**Builtin tests** (`src/builtins.rs`):
- `jobs_with_empty_table_prints_nothing` (returns 0, output empty).
- `jobs_lists_running_and_done` with synthetic entries.
- `wait_with_no_jobs_returns_immediately`.
- `wait_with_args_errors`.
- (Tests that involve real `waitpid` are reserved for smoke tests.)

**Executor tests:**
- `background_pipeline_records_job_and_returns_zero` — runs `/bin/true` (a quick external) with `&`, asserts the job table has one Running or recently-Done entry and the exec outcome is `Continue(0)`. Then calls `reap_completed` and verifies the job transitions. Skip if `/bin/true` is missing.
- These are inherently process-dependent; keep small and tolerant.

**Smoke tests** (final plan task, manual verification):
```
sleep 0.2 &
jobs                              → [1]+ Running   sleep 0.2 &
wait
jobs                              → (empty; the Done line was printed by wait)

sleep 0.1 & sleep 0.2 &           (error: only one trailing &; use two lines)
sleep 0.1 &
sleep 0.2 &
jobs                              → two entries
wait

false &
wait
echo $?                           → 0

echo hi &                         → hi
                                  → [N] Done    echo hi &

cmd1 && cmd2 &                    → shuck: syntax error: '&' on multi-command sequence not supported; use a single pipeline

cmd1 & cmd2                       → shuck: syntax error: '&' not allowed here

cd / &                            (synchronous; cwd changes; [N] Done printed)
pwd                               → /

sleep 100 &
kill <pid_from_jobs_-l>           (out of scope for v6 — verify manually that the job is killed and notified)
```

---

## 8. File summary

| File | Change |
|------|--------|
| `Cargo.toml` | Add `libc = "0.2"` (direct dependency for `waitpid`, status macros, `WNOHANG`). Add `SIGCHLD` use from `signal_hook::consts` (existing crate). |
| `src/lexer.rs` | `&` (not followed by `&`) → `Operator::Background`. Remove `LexError::BareAmpersand`. Update tests. |
| `src/command.rs` | Add `background: bool` to `Sequence`. Add `ParseError::UnexpectedBackground` and `ParseError::BackgroundedMultiPipelineSequence`. Parser logic: trailing `Background` op on a single-pipeline Sequence → set flag and consume; on multi-pipeline → error; mid-sequence → error. Update all existing test fixtures (`Sequence { ..., background: false }`). |
| `src/shell.rs` | `lex_error_message` loses `BareAmpersand` arm. `parse_error_message` gains the two new arms. Install SIGCHLD handler at startup. REPL calls `jobs::reap_completed(&mut shell)` then drains notifications before each `readline`. Pass the SIGCHLD `Arc<AtomicBool>` through `Shell::new`'s constructor — or store it as a module static, mirroring SIGINT. |
| `src/shell_state.rs` | `Shell` gains `pub jobs: JobTable` and `pub sigchld_flag: Arc<AtomicBool>`. `Shell::new` initializes both. `Clone` propagates (job_table and atomic-flag-handle clone cheaply). |
| `src/jobs.rs` | **New.** `Job`, `JobState`, `JobTable`, `reap_completed`, helper for status decoding. Unit tests. |
| `src/executor.rs` | `execute` and `execute_inner` gain a new `source: &str` parameter (the original input line, used for the job's display command). `execute_inner` branches on `seq.background`. New `run_background_sequence` for the background path: special-cases pure-builtin pipelines (synchronous + synthetic Done), and otherwise spawns the pipeline with `process_group` and `/dev/null` stdin. Builds Job, registers in `shell.jobs`, prints `[N] PID` to stderr, returns `Continue(0)`. `execute_capturing` (from v5) passes an empty `source` since substitutions ignore the background flag. |
| `src/builtins.rs` | New `builtin_jobs` and `builtin_wait`. `is_builtin` and `run_builtin` dispatch updated. New tests. |
| `src/main.rs` | Add `mod jobs;`. |

Estimate: ~700 LoC implementation + ~300 LoC tests across 8 files.

---

## 9. Out of scope / future work

- **Sub-project B**: `fg`/`bg`/Ctrl-Z, terminal control via `tcsetpgrp`, `Stopped` job state.
- **Sub-project C**: `disown`, `kill %N`, job specifiers, `wait %N` and `wait PID`.
- **Background multi-pipeline sequences** (`cmd1 && cmd2 &`): requires forking the shell for proper subshell semantics; deferred.
- **`set -m` / `set +m`**: job control on/off switch. Not relevant — shuck is always interactive.
- **Suspend/resume signaling** of background jobs (sending SIGSTOP/SIGCONT manually): not until kill in sub-project C.
- **Process accounting** (`time`, `times` builtin): out of scope.
