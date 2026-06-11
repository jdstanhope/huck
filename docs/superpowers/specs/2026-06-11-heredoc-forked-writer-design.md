# huck v134 — fork a writer for heredoc/herestring bodies (M-120 deadlock) Design

**Status:** approved design, ready for implementation plan.
**Implements:** fix M-120 — a heredoc/herestring body that exceeds the OS pipe
buffer (~64 KB), OR feeds a consumer that backpressures, deadlocks because the
parent does a BLOCKING `write()` of the whole body before the consumer drains.
This is the FINAL `nvm ls-remote` blocker (verified: with the fix, `nvm ls-remote`
completes end-to-end, 888 lines, byte-identical to bash).
**Branch (impl):** `v134-heredoc-forked-writer`.

## Background — measured root cause (full `nvm ls-remote` trace)

A deep trace of `nvm ls-remote` (isolated worktree, real network) found ONE root
cause behind the remaining hang: huck delivers a heredoc/herestring body to a
child's stdin by having the PARENT `write()` the whole body to a pipe, then run
the consumer. When the body exceeds the ~64 KB pipe buffer (or the consumer
backpressures before draining), the parent's `write()` blocks and the consumer is
never reached → deadlock. It surfaces at EVERY heredoc-stdin site:

| shape | site | huck | bash |
|---|---|---|---|
| compound `{ cmd; } << big` | `with_redirect_scope` → `write_pipe_for_stdin` (executor.rs:2543) | HANG | ✅ |
| pipeline `cmd << big \| c2` | `run_multi_stage` (executor.rs:4072) | HANG | ✅ |
| captured single `r=$(cat << big)` | `run_subprocess` (executor.rs:3359) | HANG | ✅ |
| background `{…} << big &` | `run_background_sequence` (executor.rs:2039) | HANG-class | ✅ |
| herestring `<<< big` on a compound | same paths | HANG | ✅ |

nvm hits the compound case: `nvm.sh:1631` feeds the ~200 KB `index.tab`
(`$VERSION_LIST`) into `{ command awk '…' | while read …; done; } << EOF`.
`write_pipe_for_stdin`'s OWN comment predicts this: *"Write may not complete if
bytes exceed pipe buffer; … A future enhancement could fork a writer if needed."*

**Key constraint discovered by prototyping:** a writer THREAD does NOT work — an
in-process pipeline stage (e.g. `command awk`, which `classify_stage` marks
InProcess) forks-WITHOUT-exec, so the forked child inherits a COPY of the writer
thread's still-open write fd (CLOEXEC only fires on exec) → the heredoc reader
never sees EOF. The fix must **fork a writer PROCESS** (the parent closes the write
end immediately, so no later in-process fork inherits it). This is what bash does.

## Architecture — one forked-writer helper, used at every heredoc-stdin site

### Component 1 — `spawn_heredoc_writer` (src/executor.rs)
Replaces `write_pipe_for_stdin`:
```rust
/// Feed `bytes` (a heredoc/herestring body) to a child's stdin without the
/// parent ever blocking on a full pipe. Forks a writer process that owns the
/// write end and writes the body, then `_exit`s; the parent closes the write end
/// immediately (so no later in-process forked stage inherits it). Returns the
/// READ end (→ the consumer's stdin) and the writer PID (to reap).
fn spawn_heredoc_writer(bytes: &[u8]) -> Result<(RawFd, libc::pid_t), io::Error> {
    // pipe(); fork();
    //   child: close r; write_all(w, bytes) (loop on partial / EINTR);
    //          close w; libc::_exit(0);
    //   parent: close w; return (r, pid).
}
```
Notes: the child must use async-signal-safe `libc::write` in a loop (no Rust
buffered I/O between fork and `_exit`); ignore `SIGPIPE` outcome (a consumer that
exits early closes `r` → the writer gets EPIPE → it just `_exit`s; bytes already
consumed). The body bytes are captured BEFORE the fork (already expanded).

### Component 2 — wire it at the 4 sites (replace the blocking write)
1. **`with_redirect_scope`** (executor.rs:566 Heredoc / :573 HereString arms):
   call `spawn_heredoc_writer` instead of `write_pipe_for_stdin`; dup2 the read
   end onto stdin in the scope (as today); record the writer pid; after the inner
   body returns, `waitpid(writer_pid, 0)` (tolerate `ECHILD`).
2. **`run_multi_stage`** (executor.rs:~4072, the `write_file.write_all(&bytes)`):
   for a stage with a heredoc stdin, use `spawn_heredoc_writer` to get the read fd
   (→ that stage's stdin) instead of make_pipe + parent write; collect the writer
   pids; after `wait_pipeline_raw`, `waitpid` each (tolerate `ECHILD`).
3. **`run_subprocess`** (executor.rs:~3359, `child_stdin.write_all(&bytes)` /
   `pending_stdin_bytes`): set the child's stdin to the writer's read fd
   (`Stdio::from(OwnedFd::from_raw_fd(r))`) instead of `Stdio::piped()` + a
   blocking parent write; after `child.wait()`, `waitpid(writer_pid)`.
4. **`run_background_sequence`** (executor.rs:~2039, the deferred `heredoc_pairs`
   write loop): use `spawn_heredoc_writer` per heredoc stage; the writer pids are
   NOT waited synchronously (the pipeline is backgrounded) — they are reaped by
   the existing SIGCHLD reaper (they are internal forks, never registered as jobs).

### Component 3 — reaping discipline (must-not-disturb job state)
- Writer pids are internal forks: they MUST NOT be added to the job table, set
  `$!`, or appear in `$PIPESTATUS`/affect `$?`. They are reaped where listed
  above; `waitpid` returning `ECHILD` (already reaped by a global handler) is
  tolerated, not an error.
- Reaping a writer must not race a pipeline stage with the same-looking pid: huck
  reaps stage pids explicitly via `wait_pipeline_raw`; writer pids are separate
  and reaped separately. Confirm no pid is double-waited.

## Scope & must-not-regress
- **Small heredocs unchanged** (the common case): a small body still arrives; the
  only difference is it comes from a forked writer that exits immediately. The
  REGRESSION to guard (found in prototyping): in `run_multi_stage`, the write fd
  must not leak into later in-process stages — with a forked writer the PARENT
  closes the write end at once, so this is structurally avoided (no
  `parent_held`/CLOEXEC juggling needed).
- **`$!` / `$PIPESTATUS` / `$?`** after a heredoc command must match bash (writer
  pids invisible). Explicit tests.
- **No new zombies**: every foreground writer is waitpid'd; background writers go
  through the existing reaper. A test asserts no lingering processes after a large
  heredoc command (best-effort: the command returns and the shell continues).
- **Untouched:** non-heredoc redirects, `<file`, pipelines without heredocs, the
  capture-drain fix (v133), eval/source sink (v132).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Add `spawn_heredoc_writer`; replace `write_pipe_for_stdin` and the three other blocking heredoc writes (`run_multi_stage`, `run_subprocess`, `run_background_sequence`); add writer-pid reaping at each foreground site. |
| `tests/heredoc_forked_writer_integration.rs` (NEW) | Per-shape large-body tests (compound/pipeline/captured-single/herestring) + small-body no-regress + `$!`/`$PIPESTATUS` unaffected, all watchdog-guarded. |
| `tests/scripts/heredoc_pipeline_diff_check.sh` (NEW) | bash-diff harness over all shapes + sizes. |
| `tests/nvm_ls_remote_pty.rs` (NEW, network-gated) | PTY payoff: `nvm ls-remote` completes and lists versions; skips cleanly if no nvm/network. |
| `docs/bash-divergences.md` | DELETE M-120 (Tier-1 2→1) on completion; ADD a `[deferred]` perf entry for the `read`-builtin per-byte-syscall slowness (off the nvm hang path). |

## Testing

1. **Integration `#[test]`s** (`tests/heredoc_forked_writer_integration.rs`), each
   wrapped in a watchdog (kill-on-timeout → a regression FAILS as a timeout, reuse
   v133's `run_guarded` pattern), asserting exact bytes/length:
   - compound: `V=$(printf 'x%.0s' $(seq 1 200000)); { wc -c; } << EOF`\n`$V`\n`EOF` → `200001`
   - pipeline: `cat << EOF | wc -c`\n`$V`\n`EOF` → `200001`
   - captured single: `r=$(cat << EOF`\n`$V`\n`EOF`\n`); echo ${#r}` → `200000`
   - awk|while compound (the nvm shape): `{ command awk '{print}' | wc -l; } << EOF`\n`$V`\n`EOF` → `1`
   - herestring on a compound: `{ wc -c; } <<< "$V"` → `200001`
   - small no-regress: `cat << EOF | wc -c`\n`hi`\n`EOF` → `3`; `{ cat; } << EOF`\n`yo`\n`EOF` → `yo`
   - `$!` unaffected: `sleep 0.1 & p=$!; cat << EOF >/dev/null`\n`big`\n`EOF`\n`echo $p $!` → the two pids equal (the heredoc writer did not change `$!`); compare shape to bash.
   - `$PIPESTATUS` unaffected: `false << EOF | true`\n`x`\n`EOF`\n`echo "${PIPESTATUS[*]}"` matches bash.
2. **Bash-diff harness** `tests/scripts/heredoc_pipeline_diff_check.sh` — all shapes
   at small AND >64 KB, `timeout`-guarded, byte-identical bash↔huck.
3. **`nvm ls-remote` PTY payoff (REQUIRED — verify before claiming):** a
   network-gated PTY test (and a manual run in the report) that sources
   `~/.nvm/nvm.sh` (NOT `~/.bashrc`) and runs `nvm ls-remote`, asserting it
   COMPLETES and prints version lines (e.g. `v18.`/`iojs`/an LTS alias). Skip
   cleanly if nvm/network absent; the per-shape synthetics are the network-free
   proof.
4. **Full regression:** entire unit + integration suite + ALL harnesses green —
   ESPECIALLY heredoc/herestring, pipeline, job-control, `$PIPESTATUS`, and the PTY
   suites; clippy clean.

## Edge cases & notes
- **Empty heredoc body:** the writer writes 0 bytes and `_exit`s → immediate EOF.
  Handle `bytes.is_empty()` (still fork, or just make a closed pipe) — pick the
  simplest that yields immediate EOF.
- **Consumer exits before reading all input** (`head -c1 << big`): the writer gets
  EPIPE on `write` → it `_exit`s; this is correct (bash same). The writer loop must
  treat EPIPE as "stop, exit 0", not an error print.
- **Writer + `set -e`/traps:** the writer child runs no shell logic — it only
  writes bytes then `_exit`s; it must NOT run EXIT traps or atexit (`_exit`, not
  `exit`). Mirrors `fork_and_run_in_subshell`'s `_exit`.
- **`read`-builtin per-byte slowness** (`builtins.rs:2027`, one `libc::read` per
  byte) is a SEPARATE pre-existing PERF issue (not a hang; off the nvm path — nvm
  feeds awk, not `read`, the big body). Logged as a deferred perf divergence, NOT
  fixed here.
- The investigation confirmed the prototype makes `nvm ls-remote` complete
  end-to-end byte-identical to bash — this design generalizes that prototype
  (forked writer everywhere + clean reaping).
