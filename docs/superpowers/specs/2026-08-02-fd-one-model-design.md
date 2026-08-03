# fd routing: collapse to one real-fd model — design

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197) — *Architecture:
prevent the fd-routing bug class — sink↔real-fd reconciliation invariant + inward
OwnedFd migration.* This design pursues the **north star** that #197 itself
records as out of its incremental scope: collapsing huck's two output models into
**one real-fd model** by forking every captured execution region (bash's
comsub-as-real-pipe). #197 stays the umbrella tracking issue; this document is the
architecture + staging for the one-model end-state.

## Problem

huck keeps **two parallel models of "where output goes"**:

1. the real OS fd table — `RedirectScope`/`dup2` (~18 refs), and
2. an in-memory software sink — `StdoutSink`/`StderrSink` = `Terminal` /
   `Capture(&mut Vec<u8>)` / `Merged` (~165/169 refs), plus `err_writer` (~101)
   and the `redirs_merge_err_into_out` reconciliation predicate.

Every **Class-B routing bug** (output sent to the wrong destination — leaked, lost,
or mis-ordered) has been a spot where the two models *disagreed*: a real
`2>&1`/`>&2` dup was applied while a capture sink was active, but the corresponding
software sink was never reconciled. The fixed cluster (#144, #176, #191) and the
open members (#195, #353, and the capture side of #77/#30) are all this shape. A
`debug_assert` reconciliation invariant (the incremental #197 scope) turns the
*next* such bug into a test failure; collapsing to **one model** removes the class
**by construction** — there is nothing left to reconcile.

### Where the two models actually diverge

The software `Merged` variant is gated on `StdoutSink::Capture` (executor.rs:1332):
it exists **only inside a captured region**. At the top level (Terminal sink) a
builtin's `2>&1 >file` already goes through the *real* fd table (`RedirectScope`
`dup2` + save/restore) exactly like an external command. So:

- **Top level** — one model already (real fds).
- **Inside `$(…)` / backticks / the embedder capture** — the software
  `Capture`/`Merged` sink, because `run_substitution` (expand.rs:2243) **clones the
  shell and runs the body in-process**, collecting output into a `Vec<u8>` rather
  than forking a subshell with a real pipe. External commands *within* a capture
  already get a real pipe (executor.rs:675); the sink exists to route **in-process**
  writers (builtins + the interpreter driver) into the capture buffer.

The in-process, no-fork capture is the entire reason the second model exists, and
the entire home of the Class-B bug family.

## End-state architecture

**One invariant:** the real OS fd table is the *only* model of where output goes.
Nothing in the interpreter consults an in-memory sink. A builtin writes to fd 1/2
(as currently redirected) exactly like an external command. `StdoutSink`,
`StderrSink`, `Merged`, `err_writer`, `redirs_merge_err_into_out`, and the
reconciliation code are **deleted**.

**Captured execution regions fork.** `$(…)`, backticks, and every
`execute_capturing` region become:

1. create a pipe;
2. `fork_and_run_in_subshell` (the existing, battle-tested fork path — pgid, fd-close
   discipline, `exec_guard::assert_single_threaded_fork`, child job-signal reset)
   with the child's fd 1 dup2'd to the pipe **write**-end (and fd 2 → the pipe too
   under an enclosing `2>&1`);
3. the child runs the body as an ordinary real-fd subshell (Terminal-equivalent:
   writes go to real fd 1/2, which *is* the pipe);
4. the parent closes the write-end, drains the read-end **to EOF**, **then**
   `waitpid`s for `$?`.

The child is a real separate process, so parent-drains-while-child-writes cannot
deadlock regardless of output size — which is precisely what the in-memory `Vec`
was working around. Subshell isolation (state changes discarded, traps reset, `$$`
unchanged, `$?` from `waitpid`) falls out of the fork for free and is *more*
bash-faithful than today's clone-the-shell approach. The comsub child is a
**transient foreground** child: waited directly, never entered in the `JobTable`
and never setting `$!` (avoids the #175 non-interactive job-table leak class).

**`$(<file)` is a file read, not a capture.** Recognized at expansion time and
slurped directly into a string — no fork, no executor, no pipe. It is the one
hot-path case that stays fork-free, legitimately: there is no command to run. It is
not a captured *execution* region and never needs the sink.

**Process substitution is already one-model.** `<(…)`/`>(…)` already back a real fd
via a fork; they simply stop being special citizens — the same "fork + real fd"
shape.

**The embedder boundary (`ExecBuilder::capture` / `run_with_sinks`) uses a temp
file.** It *is* the top-level process, so it cannot fork itself, and huck's
single-threaded fork-safety rules out a concurrent drain thread. To let the sink
type be deleted *entirely*, this boundary redirects the process's fd 1/2 to a temp
file (both to one file under `merge`), runs the shell, and reads the file back —
all-real-fd, no thread, no deadlock. The cost is temp-file I/O on the embedder
capture API only (never on shell-internal comsub, which uses pipes via fork). This
is the one deliberate trade: we pay temp-file I/O at the embedder boundary to
achieve full deletion of the sink type rather than keeping a thin boundary-only
sink.

## Staging

Each stage is an independently shippable iteration under the #197 umbrella, lands
its own PR, and keeps every suite + the bash test harness green before the next
begins. The "large change" is a sequence of verified small ones.

### Stage 0 — Safety net (this iteration; **no behavior change**)

Add bash-diff harnesses that pin the **current, correct** behavior of every
combination Stage 1 will move, so Stage 1 either keeps them green or has a precise,
pre-agreed target to change. Any case that is **already** diverging is recorded (it
becomes a Stage-1 target, not a Stage-0 failure). Coverage:

- `$(cmd 2>&1)`, `$(cmd >file 2>&1)`, `$(cmd 2>&1 >file)`, `$(builtin 2>&1)` —
  ordering-dependent stderr routing under capture (the #195/#353 shapes).
- **Large output**: a `$(…)` emitting well over one pipe buffer (>64 KB) — the
  deadlock case the fork must handle.
- **Nesting**: `$( $(…) )`, `$(…)` inside a pipeline stage, `$(…)` inside an
  already-forked subshell, `$(…)` as the in-process lastpipe last stage.
- **Subshell semantics**: `$?` after `$(exit N)`, `last_cmd_sub_status`,
  trap-reset inside `$(…)`, a variable/`cd` mutated inside `$(…)` not leaking to the
  parent, `$(sleep … &)` background-job-inside-comsub behavior, `$(cat)` reading the
  shell's stdin.
- **Builtin vs external** producers under capture, and `$(<file)` (must stay a plain
  file read).

Run the existing differential audits (`tools/redirect_audit.sh` + pipeline/bg
variants, `tools/soak/`) and confirm green. Deliverable: new
`*_diff_check.sh` harness(es) wired into `run_diff_checks.sh`, all green on the
current tree. **No engine code changes in this iteration.**

### Stage 1 — Fork `$(…)` and backticks (the prize) — ✅ LANDED (2026-08-02)

`run_substitution` stops cloning-and-capturing in-process; it forks (pipe + child
real-fd subshell + drain-to-EOF + `waitpid`) as above, and `$(<file)` is split out
first as a direct file read. **What dissolves by construction:** #195, #353, and the
capture side of #77/#30. Highest value, highest risk → most verification.

**Landed as `executor::capture_via_fork` + a `run_substitution` rewire +
`try_read_file_substitution` (`$(<file)` pre-fork read).** #353 and #195 closed
(their Stage-0 pins became real `check`s vs bash). Verification: engine lib
1980/1980, full sweep 245/245, `redirect_audit` 157/157 agree 0 diverge, 15
comsub/subshell/pipeline/procsub/jobs integration bins green, and **perf ~0.9 ms per
`$(…)` — slightly faster than bash** on a 1000-comsub loop (fork-per-comsub is a
non-issue). One surprise: `$(trap)` now lists nothing like a plain `( trap )`
subshell — a **general subshell-trap-display gap** (#389), orthogonal to the fork
(the fork correctly makes comsub a subshell), re-pinned not fixed. `execute_capturing`
is retained (`#[allow(dead_code)]`) for the executor unit tests until Stage 3.

#### Stage-1 targets (pinned by Stage 0)

The Stage 0 harnesses (Task 5) surveyed the capture combinations. Every case
matched bash on the current tree **except** these, pinned to current huck as
`check_pin` — the precise, pre-agreed change-set Stage 1 must flip green:

- **#353** — `comsub_capture_matrix`, case `readonly-arith-2>&1`
  (`x=$(readonly r=1; (( r++ )) 2>&1)`): bash captures the readonly-var error into
  `x` (out `<…readonly variable>`, rc 0); huck leaks it onto its own real stdout
  *ahead* of the capture and captures nothing. **Forking fixes it by construction**
  (the child's stderr→`2>&1`→pipe is real).
- **#195** — the still-diverging shape is the **compound-group**
  `{ …; } 2>&1 >file` inside `$()`, already pinned in the existing
  `comsub_merge_stderr_diff_check.sh`. Note: Stage 0 found the *bare-simple-command*
  `>file 2>&1` / `2>&1 >file` orderings **already match** bash — so Stage 1's #195
  scope is specifically the compound-group ordering.

Two further pins are **orthogonal** to the fork (documented so Stage 1 leaves them
alone, not fork targets):

- **#387** — brace expansion capped at 65536 elements (a parse-time error where bash
  expands); surfaced by a `{1..70000}` test case, unrelated to comsub plumbing.
- **comsub `trap` listing** — a comsub's `trap` (no args) output omits bash's default
  `trap -- '' SIGTSTP` row; a subshell trap-listing detail, not fd routing.

### Stage 2 — Temp-file the embedder boundary; drop streaming — ✅ LANDED (2026-08-02)

**Landed:** production `capture()` is sink-free (redirects real fd 1/2 to a temp
file, runs with `Terminal` sinks, reads back — commit `f532237`), and the entire
streaming-callbacks feature is removed (`f8ace1e`: deleted `callbacks_thread_local`,
`line_buf`, `engine_stream_diff` example, `streaming_fd_serial`/`tee_inherit` test
binaries; trimmed `on_stdout_line`/`on_stderr_line`, `Callbacks`, tee,
`run_with_sinks_tee`). Because temp-file capture redirects PROCESS-GLOBAL fd 1/2 (it
collides with libtest's fd-1 reporter in the parallel `--lib` binary — same class as
Stage 1's fork guard), the lib test build keeps an in-memory `#[cfg(test)]`
`capture()`; the production temp-file path is covered by the single-`#[test]`
`capture_tempfile_serial` binary. Verified: sweep 245/245, redirect_audit 157/157
0-diverge, engine lib 1961/1961 under `--test-threads 4`, bash-suite PASS-set
unchanged. **Stage-3 residuals** (the only remaining `Capture`/`Merged` users):
the `#[cfg(test)]` in-memory `capture()`, `run()`'s `merge_stderr` via
`StderrSink::Merged`, and `execute_capturing` (comsub-in-tests) — Stage 3 converts
`run()` merge to a real dup2, migrates the in-memory capture unit tests to a serial
integration binary, and deletes the sink types.

Convert `ExecBuilder::capture` (and any non-streaming `run_with_sinks`) to redirect
the process's fd 1/2 to a temp file (one file under `merge`), run, read back. Handle
cleanup on panic and `TMPDIR`.

**Decision (user, no API users exist):** the `run()` **streaming-callbacks** path
(`push_stdout`/`push_stderr`, tee-to-saved-fd, `Callbacks`, `callbacks_thread_local`,
the `LineDispatchWriter` callback firing) is **removed**, not re-architected. It was
the only remaining user of the software `Capture`/`Merged` sink at the embedder
boundary that a temp file cannot serve (streaming needs live, in-process
observation; a pipe + drain thread would break the single-threaded-fork invariant,
#184). Streaming is a nice-to-have to **revisit after the split is fully fixed** —
and its whole reason for being in-process was **thread affinity**: callbacks must
fire on the same thread that created the `Engine`, never a drain thread. Any future
re-introduction must preserve that. `run()`'s existing no-callback **fast path** (fd
1/2 inherit directly, already sink-free) is unchanged.

After Stage 2, the only remaining `Capture`-sink user is `execute_capturing`
(the in-process comsub capture kept for the lib unit tests + the `#[cfg(test)]`
`run_substitution` path from Stage 1) — which Stage 3 removes.

### Stage 3 — Delete the software sink — ✅ LANDED (2026-08-02) — ARC COMPLETE

Delivered in three commits (Full, per the user — no dead code left in production):
1. **`run()` merge → real `dup2`** (removes the last live production `Merged`; production is now strictly single-model).
2. **`#[cfg(test)]` thread-local capture** (`capture_test_hook`, hooked at `FdWriter`/`CaptureStderr` write sites) replaces the `Capture` sink for the in-process unit tests (parallel-safe); **`Capture`/`Merged`/`LineDispatchWriter` + the external-under-capture pipe path deleted**; external-output-capturing tests migrated to the serial `capture_tempfile_serial` binary or dropped as redundant with `comsub_merge_stderr_diff_check.sh`. Both sink enums become single-variant `{ Terminal }`.
3. **`StdoutSink`/`StderrSink` types deleted** and the vestigial `&mut StdoutSink`/`&mut StderrSink` parameter stripped from ~63 functions; `err_writer()` zero-arg; net −639 lines.

**The interpreter now consults exactly ONE model of where output goes — the real fd
table.** Verified at each step: build warning-clean, `git grep StdoutSink/StderrSink`
empty, engine lib 1953/1953 under `--test-threads 4`, sweep 245/245, redirect_audit
157/157 (0 diverge), bash-suite PASS-set unchanged (26 categories, 0 lost/gained).

### Class-A — inward OwnedFd migration — ✅ LANDED (2026-08-02)

The resource-safety half of #197 (plan:
`docs/superpowers/plans/2026-08-02-owned-fd-migration.md`). Interior OWNED fds now
have `OwnedFd`/`File` RAII owners so they cannot leak or double-close: `RedirectScope`
saved fds (`own_dup`), `make_pipe_owned`, `capture_via_fork`, procsub, stdin_pipe,
heredoc pipe/file, `exec_builder`'s `FdRestore`/`Fd2Restore`, and `wait_loop`'s
`sigchld_fd`/`kq`. Interior manual `libc::close` count **97 → 54** (~43 retired). The
residual raw closes are either **genuinely borrowed** (fd numbers 0/1/2, dup2 targets,
child-side post-fork closes) or **owned-but-entangled and documented as left raw**:
the pipeline inter-stage wiring (double-tracked by fd *number* with alternating
ownership — an `OwnedFd` there would create two owners → double-close on a hot path)
and coproc/procsub records (stored in `Clone` structs; `OwnedFd` isn't `Clone`).
Verified behavior-neutral: sweep 245/245, redirect_audit 0-diverge, engine lib 1953
under `--test-threads 4`, and the **`tools/soak` harness PASS — the resting fd floor is
flat (0 delta over ~925→1250 iterations), i.e. no leak.** (The macOS `kq` conversion
mirrors the Linux one but is unverified on the Linux CI box.)

**Follow-on cleanup (tracked for a short next step, per the user):** `FdWriter::new`
is now `#[cfg(test)]`; the in-memory compound `{…} 2>&1`-under-capture software merge
was dropped as dead (production's fork + real-dup2 path and the diff-harness cover it);
one `alias_tests` not-found stderr-text assertion was relaxed (a forked child's fd 2
is uncapturable in-process — outcome + stdout still cover it). #197's inward `OwnedFd`
migration (Class-A resource-safety) can now proceed independently.

## Risks & verification

- **Deadlock (full pipe, >64 KB).** Parent closes write-end → drains to EOF →
  `waitpid`, in that order. Stage-0 harness locks it in.
- **Perf.** Fork per `$(…)` regresses comsub-heavy loops; accepted (correctness
  first). `$(<file)` stays fork-free. Measured as info (`for i in {1..1000}; do
  x=$(echo hi); done`), not a gate.
- **Real-subshell semantics** may shift edges vs. today's clone (bg-job-in-comsub,
  trap reset, `$?` from `waitpid`). Stage-0 harnesses pin bash's actual behavior so
  each is a deliberate decision.
- **JobTable/`$!`/pid-registry** hygiene — transient-foreground child, guarded by
  `tools/soak`.
- **`exec_guard`/single-thread** — the child forks-without-exec and runs the
  interpreter; already asserted; unit tests run `--test-threads 1`.
- **macOS** — more forking; CI is linux (`ubuntu-24.04`), so the harness verifies
  linux; macOS remains a documented caveat (#96/#97).
- **Embedder temp-file** — cleanup on panic/signal, `TMPDIR`, no fd leak (Stage 2).

Verification net, constant across stages: differential audits + full per-crate
suites + the bash test harness + the Stage-0 harnesses; **any harness red is a hard
blocker.**

## Success criteria

The interpreter consults exactly one model of "where output goes" (the real fd
table); `StdoutSink`/`StderrSink`/`Merged` and the reconciliation code are deleted;
`$(…)` and backticks fork a real subshell over a pipe; `$(<file)` is a file read;
the embedder capture uses a temp file; and the member routing divergences
(#195, #353, capture side of #77/#30) are closed by construction with the full
suites + bash harness green.

## Non-goals

- Not the inward `OwnedFd` / Class-A resource-safety migration (that is #197's other
  half; it proceeds independently, easier once the sink is gone).
- Not a change to top-level (non-captured) redirect handling, which is already
  one-model.
- No fork-free fast-path for general comsub beyond `$(<file)`.
