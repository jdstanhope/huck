# v298 — builtin write-error + InProcess pipeline-stage redirect order (batch B) — Design

**Issues:** closes [#137](https://github.com/jdstanhope/huck/issues/137),
[#144](https://github.com/jdstanhope/huck/issues/144); **advances (does not close)**
[#147](https://github.com/jdstanhope/huck/issues/147) — the #144 fix retires the InProcess-stage
slot pre-wiring, one of #147's cleanup targets.

Two independent fixes in the builtin-output / pipeline-stage area (`crates/huck-engine/src/executor.rs`,
`builtins.rs`), grouped because both were filed as the "H7 software-sink family." Investigation
reframed #144: it is not a software-sink routing gap but a **source-order fd bug** for InProcess
pipeline stages (the InProcess analog of the v293 external-stage flip, a sibling of #50).

## Section 1 — #137: builtin stdout write error is swallowed (rc 0 vs bash rc 1)

**Behavior:**
```
bash -c 'exec >&-; echo end'   # bash: line 1: echo: write error: Bad file descriptor   (rc 1)
huck -c 'exec >&-; echo end'   # (nothing)                                               (rc 0)
```
Confirmed identically for `printf "end\n"`, `echo -n end`, and `echo hi >&-`.

**Mechanism.** `builtin_echo` (`builtins.rs:679/683`) and `builtin_printf` (`builtins.rs:4143`)
DO check their `out.write_all(...)` result and return `Continue(1)` on `Err`. But `out` for the
Terminal sink is Rust's line-buffered `io::stdout()`: a `write_all` that does not force a flush
(no trailing newline, or a full-line write whose buffer copy succeeds) returns `Ok` **without the
`write(2)` syscall**. The real OS error (EBADF on a closed fd, ENOSPC on a full disk) only surfaces
at the next `flush()`, and every flush site does `let _ = ...flush()` — discarding it. The
authoritative site is the `run_builtin_with_redirects` epilogue flush (`executor.rs:1531`, run
immediately after the builtin returns, while its own redirect scope is still installed so fd 1 is
the redirected target).

**Fix.** At the `run_builtin_with_redirects` epilogue, capture the stdout flush `Result` instead of
discarding it. On `Err(e)`, if the builtin's own outcome status is still 0 (it did not already
report the error itself — the **double-emit guard**), emit `"<name>: write error: <bash_io_error(e)>"`
to the stage's error writer and override the outcome to `Continue(1)`. `<name>` is the builtin
program name (`echo`/`printf`/…) that `run_builtin_with_redirects` already holds. Use
`bash_io_error(&e)` so the suffix matches bash (`Bad file descriptor`, not Rust's
`… (os error 9)`); this also fixes printf's pre-existing raw-`{e}` inconsistency at its own
write-check site if that path ever fires.

**Scope.** The flush-driven check is builtin-agnostic and catches the write failure uniformly for
**non-pipe** destinations (closed fd, file/disk errors). Pipe destinations are unchanged: a builtin
writing to a broken downstream pipe dies by SIGPIPE (exit 141) before any `io::Error`, matching
bash (huck resets `SIGPIPE` to `SIG_DFL`; see `tests/sigpipe_integration.rs`). The forked
pipeline-stage flush (`fork_and_run_in_subshell`, `executor.rs:7979`, before `_exit`) is a
secondary site; the repros are all bare-builtin (single command), so the primary fix is the
`run_builtin_with_redirects` epilogue. If a pipeline-stage builtin with a closed stdout is easily
reachable and diverges, extend the fix there too (set `status` before `_exit`); otherwise note it.

## Section 2 — #144: InProcess pipeline-stage redirects applied out of source order

**Behavior:**
```
printf '%d\n' abc 2>&1 >/tmp/b | cat
#   bash: error -> pipe (cat prints "printf: abc: invalid number"), "0" -> file
#   huck: error -> FILE (not the terminal the issue guessed), "0" -> file
printf '%d\n' abc >/tmp/b 2>&1 | cat
#   bash & huck agree: error AND "0" -> file
```
Only the `2>&1 >f` order diverges; `>f 2>&1` already matches.

**Mechanism.** For an InProcess stage the stage loop computes `explicit_stdout` from the stage's
own `slot_stdout()` (`executor.rs:6347`, gated `!stage_is_external`) and wires it into the child's
**base** fd 1 (`executor.rs:6532`). The forked child then re-applies the *same* stage redirects in
source order via `run_command` → `run_builtin_with_redirects` → `apply_redirects`
(`executor.rs:1355`). Because the base fd 1 is **already the file** when the child applies `2>&1`,
`2>&1` binds stderr to the file instead of the source-order pipe. External stages do not have this
bug: they get a pipe-only base and replay an ordered `ChildRedirPlan` (v293/#50). Compound stages
`{ …; } 2>&1 >f | cat` already work because their forked body applies its own redirects in order.

**Minimal fix.** Give InProcess pipeline stages a **pipeline base** (fd 1 = inter-stage pipe /
capture / inherit, exactly like external stages) and rely on the child's existing source-order
re-application — i.e. stop pre-wiring the stage's own `explicit_stdout`/`explicit_stderr`/dup-target
into the base for InProcess stages. Then `2>&1 >f` applies in source order in the child: `2>&1`
(stderr → the pipe base), `>f` (stdout → file) — matching bash. This removes the redundant
double-application and the source-order corruption in one move.

**#147 overlap (noted, not closed).** The pre-wiring being removed is the InProcess-stage
`explicit_stdout`/`explicit_stderr`/dup-target slot machinery that #147 wants to retire. v298 makes
the minimal change needed for #144 correctness and comments the #147 progress; the broader slot
deletion (`slots_for_simple_path`, `RedirectSlot`, the spawner's legacy `else`, the single-command
`run_subprocess` path) stays with #147.

**Risk — verify before removing the pre-wire.** Every InProcess stage TYPE must actually re-apply
its own redirects in the forked child, or removing the pre-wire drops the redirect entirely:
- **builtin** (`Simple(Exec)` builtin) → `run_command` → `run_builtin_with_redirects` applies
  `exec.redirects` ✓ (the #144 case).
- **function** → `run_command` → `with_redirect_scope` applies redirects ✓ (verify).
- **compound** (`{…}`, `(…)`, `if/for/while` with a trailing redirect) → verify the child applies
  the compound's own redirects (the issue says these already route correctly).
- **assign-only** (`x=1 >f` as a stage) → verify.
The plan MUST confirm each type re-applies (a `_diff_check.sh` case per type) before deleting the
pre-wire; if any type relies on the pre-wire, keep the pre-wire for that type and narrow the fix to
the builtin case.

## Error handling

- #137: only the message text + exit status change; no fd/behavior change. The double-emit guard
  prevents two diagnostics when the builtin already reported.
- #144: only the fd wiring order changes; redirect success/failure and which fd ends up where must
  match bash. No `{var}` numbering change (v297) — the dup-target pre-resolution needed for `{var}`
  visibility must be preserved where it is still required; the plan verifies the `{var}` cases
  (`{v}>f 2>&$v | cat`) still pass after the base change.

## Testing

- **New `tests/scripts/builtin_write_error_diff_check.sh`** (#137): `exec >&-; echo end`,
  `exec >&-; printf 'end\n'`, `exec >&-; echo -n end`, `echo hi >&-`, and a full-disk-style file
  case if cheaply reproducible; assert stdout+stderr+rc byte-identical to bash (with the standard
  shell-name/`line N:` prefix normalization).
- **New `tests/scripts/builtin_stage_stderr_diff_check.sh`** (#144): the `2>&1 >f | cat` matrix —
  `printf '%d\n' abc 2>&1 >f | cat`, `printf '%d\n' abc >f 2>&1 | cat`, `echo hi 2>&1 >f | cat`,
  plus one case per InProcess stage type (function, compound, assign-only) to prove source-order
  re-application, plus the `{var}` visibility guard `true {v}>f 2>&$v | cat`. Compare the pipe
  output, the file contents, and rc, byte-identical to bash.
- **Regression:** `redirect_audit.sh` should drop the closed-stdout DIVERGE cases (v297 review
  counted these among its remaining 8); report before/after. `builtin_stdout_dup_diff_check.sh`,
  `builtin_pipe_flush_diff_check.sh`, `pipeline_redirect_audit.sh`, `redirect_diag_diff_check.sh`
  (v297), `fd_torture`, `sigpipe_diff_check.sh`, engine lib (~1806), and the full sweep (its prior
  baseline plus the two new harnesses, 0 failed on both binaries) must stay green.

## Risk / sequencing notes for the plan

- #137 is small and self-contained — a natural first task with its own gate.
- #144 touches the shared stage-loop stdio-construction code the fd-plumbing arc has been reworking
  (v293–v297); it is the riskier task. Gate it hard with the per-stage-type matrix + the full
  pipeline/redirect audits, and verify the `{var}` (v297) and per-stage-failure (v296) behaviors are
  untouched. If removing the pre-wire regresses a stage type, narrow to the builtin case and file a
  follow-up for the rest.
- Keep [[huck-recent-bug-cluster]] in mind: this is the same per-path-duplicated pipeline fd
  machinery; the whole-branch review must check for a missed sibling path (fg vs bg pipeline, capture
  vs inherit).
