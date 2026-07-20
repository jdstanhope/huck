# v318 — procsub `$!` + assignment-RHS fd lifetime (flip the `procsub` category)

**Issue:** [#218](https://github.com/jdstanhope/huck/issues/218). The `procsub`
bash-suite category is a 2-divergence near-miss; fixing both flips it to PASS.

**Goal:** (1) a process substitution sets `$!` to its child PID and that child
stays waitable (`wait "$!"` returns the child's exit status); (2) `f=<(…)`
process-substitution assignment works (see the correction below).

> **CORRECTION (v318 implementation, verified against bash 5.2.21):** Divergence 2's
> premise below — that bash keeps an assignment-RHS procsub's fd open "until the
> scope" so a later `cat $f` works — is **WRONG**. Measured: `f=<(echo hi); cat "$f"`
> errors identically in BOTH shells (`/dev/fd/N: No such file`); bash closes the fd
> at the assignment command's end, exactly like every other command. The
> `procsub.tests` `eval f=<(echo test4) "; cat $f"` case works because the procsub
> is realized in **`eval`'s own argv expansion**, not via assignment-RHS lifetime.
> So the `procsub_deferred` "defer to scope" design (below) was built and reverted;
> the real bug was that `f=<(…)` didn't parse/expand as a procsub at all — the
> lexer split `<(` off the assignment value, and `expand_assignment` no-op'd
> `WordPart::ProcessSub`. The shipped fix glues `<(…)`/`>(…)` onto the assignment
> value in the lexer (guarded so `x=<foo` stays a redirect), realizes it in
> `expand_assignment`, and drains it per-command like any other procsub (same
> point bash closes it). Fix 1 (`$!`) is unchanged and correct. The
> `procsub_deferred` / `process_line_in_sinks` sections below are superseded.

---

## The two divergences (measured against bash 5.2.21)

`<` = huck, `>` = bash in the runner diff. Everything else in `procsub.tests`
already passes — huck's per-command procsub cleanup is correct for
*consuming-command* forms (`cat <(…)`, `3< <(…)`, etc.).

**Divergence 1 — `$!` from a process substitution** (`procsub1.sub`):
```
cat <(exit 123) >/dev/null
wait "$!"; echo $?
# bash: 123
# huck: wait: `': not a pid or valid job spec   (rc 1)   — $! is empty
```
`procsub::realize` forks the child (`fork_and_run_in_subshell` returns its pid)
but never sets `shell.last_bg_pid`, so `$!` expands empty. And `procsub::cleanup`
reaps the child with a discarded `waitpid`, so even with `$!` set, `wait "$!"`
would find no child and no saved status.

**Divergence 2 — assignment-RHS fd lifetime**:
```
eval f=<(echo test4) "; cat $f"
# bash: test4
# huck: /dev/fd/3: Permission denied
```
`f=<(…)` captures the `/dev/fd/N` path string; the fd is the parent read end
(`realize_via_devfd`). But the enclosing `run_exec_single` drains
`procsub_pending[base..]` when the assignment command completes
(`drain_procsubs` → `cleanup` closes the fd), so the later `cat $f` opens a dead
fd → `Permission denied`. Consuming-command forms work because the command uses
the fd *before* the drain.

---

## Design

### Fix 1 — `$!` + waitable procsub child (reuse the v306 saved-status ring)

- **`realize`** (both `realize_via_devfd` and `realize_via_fifo`): after the fork
  succeeds, set `shell.last_bg_pid = Some(pid)`. bash sets `$!` to the most
  recent process substitution's child PID.
- **`cleanup` → save the status**: `cleanup(ps: ProcSub)` currently `waitpid`s
  and discards the status. Change it to RETURN the reaped `(pid, exit_code)`
  (decode via the existing wait-status decoder), and have `drain_procsubs`
  (which holds `&mut shell`) record it in the v306 ring:
  `shell.jobs.record_terminal_status(pid, code)`. Then `wait "$!"` (builtins.rs
  wait, already consulting `shell.jobs.saved_status(pid)` at ~4519) resolves to
  `123`. Ordering in `procsub1.sub` is satisfied: `cat <(exit 123)` completes →
  its procsub is drained → status 123 saved to the ring → `wait "$!"` finds it.
  - `cleanup` is a free function with no `Shell`; keep it free, return
    `Option<(i32, i32)>` (pid, code) — `None` when `pid <= 0` / already reaped —
    and let the two callers (`drain_procsubs`, `drain_procsubs_nonblocking`)
    record it. `drain_procsubs_nonblocking` (background path) closes fds without
    blocking-reaping today; it should NOT start blocking — record only when a
    non-blocking `waitpid(WNOHANG)` actually reaps (leave its current behavior,
    just thread the ring where it already reaps). The `$!`/`wait` case goes
    through the blocking `drain_procsubs` path (foreground), which is what
    `procsub1.sub` exercises.

### Fix 2 — defer assignment-RHS procsub cleanup to the scope boundary

A procsub realized while evaluating a **standalone assignment** RHS escapes into
a variable, so it must outlive the assignment command. Add a deferred list and a
scope-boundary drain:

- **`Shell::procsub_deferred: Vec<ProcSub>`** (new) — procsubs whose cleanup is
  deferred past the creating command.
- **Move, don't drain, for a standalone assignment.** The standalone assignment
  path is `run_assignment_list` (dispatched at `executor.rs:3839`,
  `ExecOutcome::Continue(run_assignment_list(...))`), wrapped by
  `run_exec_single` which snapshots `procsub_base` and drains at command end.
  After `run_assignment_list` returns, MOVE `procsub_pending[procsub_base..]`
  into `procsub_deferred` (drain the enclosing snapshot to zero without closing).
  Do this ONLY for the standalone-assignment dispatch — consuming commands keep
  the current per-command drain. (Simplest: in the `run_assignment_list`
  dispatch arm, record the base before, and after it returns splice the tail
  from `procsub_pending` into `procsub_deferred`.)
- **Drain `procsub_deferred` at the input-unit / function boundary.**
  `process_line_in_sinks` (shell.rs) runs one top-level input unit as a single
  sequence — for `eval "f=<(…); cat $f"`, the whole list is one
  `process_line_in_sinks` call, so the deferred fd lives across the assignment
  AND the `cat`. Snapshot `procsub_deferred.len()` at `process_line_in_sinks`
  entry, and drain `procsub_deferred[base..]` (close fd + reap, saving status to
  the ring per Fix 1) on every exit path. Also drain a function's deferred
  procsubs at function return (the local-scope unwind point), so a
  `f(){ x=<(…); }` procsub does not outlive the function.

**Deferral target rationale:** the test's only assignment-RHS case is inside one
`eval` (a single `process_line_in_sinks` unit), so draining at that boundary is
sufficient and matches bash's "the fd lives until the scope" model. Two SEPARATE
top-level statements (`f=<(…)` on one line, `cat $f` on the next) would still
close the fd at the first statement's input-unit boundary — bash keeps it until
script exit — but that form is NOT in `procsub.tests` and is a documented
non-goal (huck processes top-level input per-unit; cross-unit procsub tracking
is out of scope).

### Non-goals

- bash's exact lazy `/dev/fd` reuse under `ulimit -n` (the `procsub.tests`
  fd-exhaustion loop uses *consuming* `3< <(…)` procsubs, drained per-command —
  already passing; the deferred list only holds rare assignment-RHS procsubs).
- The separate-top-level-statement `f=<(…)` / `cat $f` form (above).

---

## Testing

- **New `tests/scripts/procsub_lifetime_diff_check.sh`** (byte-diff huck vs bash,
  normalize the shell-name prefix, compare stdout + stderr + rc — capture rc
  without a pipe):
  - `$!` from a procsub: `cat <(exit 123) >/dev/null; wait "$!"; echo $?` → `123`.
  - `$!` set at all: `cat <(:) >/dev/null; [ -n "$!" ] && echo set`.
  - assignment-RHS lifetime: `eval f=<(echo test4) "; cat \$f"` → `test4`; also
    the plain `f=<(echo hi); cat "$f"` form inside a single `-c` string.
  - control (still works): `cat <(echo a) <(echo b)`; `f2(){ cat "$1"; }; f2 <(echo x)`.
- **Flip check:** re-sweep `procsub` — expect **PASS** (0-diff). This is the
  #218 payoff.
- **Regression guards:** the existing procsub/process-substitution coverage —
  `/usr/bin/grep -rln 'procsub\|<(' tests/*.rs tests/scripts/*.sh` — stays green
  at `--test-threads 2` (the deferred-list change must not leak fds or reap the
  wrong child); full `run_diff_checks.sh` green; `$!` for a real background job
  (`sleep 0 & echo $!`) unchanged (the procsub `last_bg_pid` set must not clobber
  a subsequent real `&` — bash's `$!` is "most recent", so the last writer wins,
  which is the natural behavior).

## Rejected alternatives

- **Keep assignment-RHS procsubs in `procsub_pending` and change the per-command
  drain to skip them.** Muddier — the per-command drain would need to know which
  entries are "assignment-owned"; a separate `procsub_deferred` list is explicit.
- **Drain deferred procsubs per top-level statement** (not per input-unit).
  Would close the fd before `cat $f` in the eval case. Rejected.
- **Don't reap procsub children in cleanup; let `wait` reap them.** Risks zombie
  accumulation and reorders bash's reap timing; saving the status to the v306
  ring (already the mechanism for `wait $pid` after auto-reap) is the
  established pattern.
