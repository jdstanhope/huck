# v289 — fd-plumbing remediation, Phase 0 (exit SIGHUP policy + bg-pipeline stdin + fd-torture net)

**Issues:** [#128](https://github.com/jdstanhope/huck/issues/128) (exit-time SIGHUP)
+ [#129](https://github.com/jdstanhope/huck/issues/129) (bg-pipeline async stdin).
Both `divergence` + `bug`.

**Context:** This is **Phase 0** of the phased fd/redirect/process-plumbing
remediation from the architectural review
(`docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md`). Phase 0 is
the "quick wins + safety net" step: two targeted policy fixes the review
reclassified as *not* structural, plus the `fd_torture` harness that will be the
objective regression net for the structural Phases 1–5. No architectural change
here — those begin at Phase 1 (`ChildFd`/`OwnedFd` module).

## Problem

### #128 — jobs SIGHUP'd on non-interactive exit
`hangup_jobs` (`shell_state.rs:3046`) is called unconditionally on every clean
exit (`shell.rs:281` for `run_program`; `repl.rs:284` for the interactive REPL),
and `should_hangup` only checks live-and-not-nohup — never interactivity. So a
backgrounded command in a non-interactive shell is killed at exit:

```
huck -c 'sleep 0.5 && echo alive > f & exit'   # child SIGHUP'd -> f never written
bash -c 'sleep 0.5 && echo alive > f & exit'   # child survives -> f = "alive"
```

bash sends SIGHUP to jobs at exit only for an interactive shell with the
`huponexit` shopt set (default off). huck defines the `huponexit` shopt
(`shell_state.rs:455`, default false) but **never reads it**. This also blocks
deterministically testing Path A async stdin (#129): a bare trailing-`&` child
must survive `huck -c` exit to be observed.

### #129 — bg-pipeline stage-0 stdin ignores the async rule
`run_background_sequence` opens `/dev/null` unconditionally for stage-0 stdin
(`executor.rs` ~3189–3208, used ~3473), ignoring both the interactive gate and
the bare-multi-stage-pipeline inherit rule. `run_background_subshell` already
gets this right via the `async_default_stdin` helper (v287). This is the review's
**H1** — the same rule implemented in two places, only one fixed.

## Design

### Section 1 — #128: gate the exit-time SIGHUP

Add a shell-level guard at the top of `hangup_jobs` (`shell_state.rs:3046`) and
return early when it does not hold:

```rust
pub fn hangup_jobs(&mut self) {
    // bash: SIGHUP jobs at exit only for an interactive shell with `huponexit`
    // set (default off). Non-interactive shells (and interactive shells without
    // huponexit) leave background jobs running. This also wires the huponexit
    // shopt, which was defined but never read.
    if !(self.is_interactive && self.shopt_options.get("huponexit").unwrap_or(false)) {
        return;
    }
    // ... existing per-job SIGCONT-then-SIGHUP loop, unchanged ...
}
```

Both callers (`shell.rs:281`, `repl.rs:284`) are clean-exit paths and inherit the
correct behavior from the single guard:
- **Non-interactive** (`huck -c`, scripts): `is_interactive == false` → early
  return → background children survive exit, matching bash. (This is the #128 fix
  and is harness-testable.)
- **Interactive**: gated on `huponexit`; default off → default is also "don't
  hangup," matching bash's default.

The per-job `should_hangup` predicate and the SIGCONT→SIGHUP loop are unchanged.

**Deliberate simplification (documented, out of scope):** bash scopes `huponexit`
to interactive *login* shells. We gate on `is_interactive && huponexit` and do
**not** add the `login_shell` distinction. With `huponexit` off by default the
observable default is already correct; the login-only case is reachable only by
an interactive login shell that explicitly enabled `huponexit` — untestable in
the diff harnesses (needs a tty) and pure YAGNI.

### Section 2 — #129: route bg-pipeline stage-0 stdin through `async_default_stdin`

Replace `run_background_sequence`'s unconditional `/dev/null` open for stage-0
stdin with a call to the existing `async_default_stdin(inherit, shell, sink,
err_sink)` helper, where `inherit = pipeline.commands.len() > 1` (a bare
multi-stage pipeline's stage 0 inherits the shell's stdin; a single-stage async
command defaults to `/dev/null` when non-interactive; an interactive shell always
inherits — the same rule `run_background_subshell` uses).

The helper returns `AsyncStdin::{Inherit, DevNull(fd)}`. `run_background_sequence`
must thread the resulting fd into its existing stage-0 stdin wiring and
`parent_held` bookkeeping, closing the `DevNull` fd on exactly the paths the
current unconditional `/dev/null` fd is closed today — i.e. reuse the existing
lifecycle, made conditional. `Inherit` means stage 0 uses `STDIN_FILENO` and no
fd is opened.

This makes both background paths share the single helper. (Phase 4 later merges
`run_multi_stage` + `run_background_sequence`, making it one site permanently.)

### Section 3 — `fd_torture_diff_check.sh` (the regression net)

A concentrated `tests/scripts/fd_torture_diff_check.sh` byte-diff harness over the
fd/redirect/pipeline/background surface the structural phases will churn,
restricted to cases huck **currently handles correctly** so it is green on day
one and its role is purely to catch *regressions* during Phases 1–5.

Coverage matrix (each fragment through bash and huck, byte-identical):
- **Freed std fds × pipelines** — `exec <&-; cat | cat`, `exec <&-; cat | cat | cat`,
  `exec 2>&-; ls /nope | cat`, `exec >&-; echo hi | cat` (post-v288-correct forms).
- **Per-stage redirects on non-last stages** — `echo hi > f | cat`, `cmd 2>f | cat`,
  fd>2: `exec 3>f; echo x >&3; cat f`.
- **Background pipelines/redirects** (observable post-#128) — `echo hi > f & wait; cat f`,
  `printf 'a\nb\n' | cat & wait`.
- **Heredoc/here-string into a stage** — `cat <<EOF | cat` / `cat <<< "s" | cat`.
- **Subshell/group redirects** — `(echo hi) 2>&1 | cat`, `{ echo hi; } > f; cat f`.
- **dup/close/merge** — `echo hi 1>&2 2>/dev/null`, `echo hi 2>&1 | cat`, `echo hi &> f; cat f`.

Mechanics follow the house style: `norm()` strips the `bash:`/`huck:` prog-name +
`line N:` prefix; both shells wrapped in `timeout` so a regression that
reintroduces a hang FAILs rather than wedging; external commands used where
builtin wording would otherwise diverge; a `mktemp -d` work dir with a cleanup
trap. Auto-discovered by `run_diff_checks.sh` (globs `tests/scripts/*_diff_check.sh`).

**Deliberately excluded (currently divergent — would be red now):**
- `exec <&-; cat < file | cat` — the redirect-CLOEXEC-on-freed-fd case (**#132**).
- Stage redirect source-ordering (**#50**).
These cases are added by the phase that fixes them (Phase 1 / Phase 3), flipping
them green at the right time instead of sitting red.

## Testing

- **#128**: a dedicated functional check *inside* `fd_torture` (not the byte-diff
  `check()` pattern — it needs post-exit polling). A backgrounded command writes a
  file after a short sleep while the shell exits immediately; assert huck (like
  bash, non-interactive) leaves the child running so the file appears, by polling
  for it under a `timeout`. Compare huck's outcome to bash's (both: file written).
- **#129**: covered by `fd_torture` background cases once #128 lands — a bare
  `cmd &` (single stage) non-interactive gets `/dev/null` (reads nothing / EOF),
  and a bare multi-stage `a | b &` stage 0 inherits — compared byte-identically to
  bash. (Interactive gating is verified manually via a `script` pty, recorded in
  the plan; not automated.)
- **`fd_torture`**: green (all cases pass) on the v289 binary.
- **Regression:** full `run_diff_checks.sh` sweep green; `cargo test -p huck-engine`
  / `-p huck-syntax` (per-crate, single-threaded). The existing
  `should_hangup` unit tests (`shell_state/tests.rs:629–646`) stay valid (the
  predicate is unchanged); add a unit test for the new shell-level guard if
  practical.

## Non-goals

- Any structural change (`ChildFd`/`OwnedFd`, merging the pipeline functions,
  retiring `RedirectSlot`) — those are Phases 1–5.
- The `login_shell` refinement of `huponexit`.
- #132 / #50 (excluded from `fd_torture` until their fixing phase).
