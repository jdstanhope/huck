# fd redirect application — Approach B (interleaved in-process, batched child)

**Issue:** [#139](https://github.com/jdstanhope/huck/issues/139) (redirected to
Approach B). **Supersedes:** the Approach-C spec/plan
(`2026-07-14-fd-plumbing-phase3a*`). **Design basis:** the empirical
[fd-management design evaluation](../reviews/2026-07-14-fd-management-design-evaluation.md).

## Why (one paragraph)

The Approach-C consolidation (one *batch* `lower_redirects` → two appliers) was
implemented and reviewed. A systematic differential audit vs bash 5.2, run
against both `main` and the C branch, showed batch-lower-then-apply **introduces
6 in-process `{var}` regressions** (side-effect interleaving) while **fixing 8
external-path divergences**; `OwnedFd` caused zero divergences. The fix is
**Approach B**: share the per-redirect *resolution*, but let the two *application*
models differ — in-process **interleaves** (resolve→apply against the real fd
table, per bash), the child **batches** (build a plan, replay post-fork).

## Goals / non-goals

**Goals**
1. Fix the 6 in-process `{var}` regressions the C branch introduced: `$v`
   visible to a later redirect, `{var}` persistence across a later redirect's
   failure, and `{var}` fd numbering (`3>a {v}>x` → `$v=10`).
2. Retain the 8 external-path fixes the C branch gained (invalid-dup detection,
   no-truncation, dup ordering — via the child's `fd_state` validation).
3. Keep `OwnedFd` RAII. Keep the consolidation win by sharing the per-redirect
   *resolution* in one `lower_one_redirect`.
4. **Acceptance gate:** `tools/redirect_audit.sh` on this branch must show
   **zero regressions vs `main`** and **zero of the 6 in-process `{var}`
   divergences**, i.e. exactly the 16 persistent (orthogonal) divergences and no
   more. `fd_torture_diff_check.sh`, `named_fd_integration`, and the full
   `run_diff_checks.sh` sweep stay green on both binaries.

**Non-goals (pre-existing, orthogonal — file/keep as issues, do NOT fix here)**
- **#137** — writes to a closed fd not detected (software-sink write path).
- **#140** — `{var}` error-message wording + external `$v`-visibility.
- **#141** — external `{var}` fd *number* under child batch (needs a virtual
  allocator). Pre-existing on `main`; not a v292 regression.
- Phase 3b/4/5 (retire `RedirectSlot`, merge the pipeline functions, procsub
  pgroup) are untouched.

## Design

### The shared resolver

```rust
/// Resolve ONE redirection into its neutral, ordered ops. No validation, no
/// `$var` assignment, no application — just: open the file (OwnedFd), spawn the
/// heredoc writer, resolve a dup WORD to a fd NUMBER, allocate a `{var}` high fd.
/// Shared by both the in-process (interleaved) and child (batch) appliers.
fn lower_one_redirect(
    redir: &Redirection,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<Vec<PlanOp>, i32>;
```

`PlanOp` and its `OwnedFd`-owning variants (`InstallOwned`, `InstallDup`,
`Close`, `NamedFd`) are exactly as built in the C branch (reused). A `{var}`
resolves to `NamedFd { high, name }` (+ a trailing `Close` for a move); a File to
`InstallOwned`; a dup/move to `InstallDup` (+ `Close` for the move, minus the
degenerate `N>&N-`); heredoc/here-string to `InstallOwned`; `Close` to `Close`.
This is the C branch's `lower_redirects` loop body + `lower_named_fd`, factored to
a single redirection.

Ordering guarantee that makes interleaving correct: `lower_one_redirect` performs
its side effects (open file, `dup_to_high_fd` for `{var}`) when it is *called*.
The in-process applier calls it immediately before applying each redirect, so a
File's temp is opened-then-closed before the next redirect resolves, and a
`{var}`'s `dup_to_high_fd` runs after earlier temps are closed — reproducing
bash's numbering.

### Applier 1 — in-process, INTERLEAVED (the fix)

`RedirectScope` gains an interleaved driver replacing the C branch's
`apply_plan(whole_batch)`:

```rust
impl RedirectScope {
    /// Resolve-then-apply each redirection in source order against the shell's
    /// real fds (save/restore). Because it is interleaved, a `{var}`'s $v
    /// assignment + fd allocation and each redirect's fd-table mutation are
    /// visible to the NEXT redirect — matching bash and the pre-C `apply`.
    fn apply_redirects(
        &mut self,
        redirs: &[Redirection],
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        for redir in redirs {
            let ops = lower_one_redirect(redir, shell, sink, err_sink)
                .map_err(ExecOutcome::Continue)?;
            for op in ops {
                self.apply_one(op, shell, sink, err_sink)?;   // per-op, vs REAL fds
            }
        }
        Ok(())
    }
}
```

`apply_one` is the per-op arm of the C branch's `apply_plan`, with dup validation
against the **real** fd table (`validate_fd_open`, not `fd_state` — the real fds
already reflect earlier applies, so this is exact and needs no simulation):
- `InstallOwned { target, source }`: `#135` `raw == target` in-place CLOEXEC-clear
  + `saved.push((target,-1))`; else `redirect(raw, target)` (dup2+save) then
  `drop(source)` (close temp immediately).
- `InstallDup { target, source }`: `validate_fd_open(source)`, then
  `redirect(source, target)`.
- `Close { target }`: `close_target(target)`.
- `NamedFd { high, name }`: `let fd = high.into_raw_fd(); shell.set(&name, fd.to_string());`
  (assign `$v` **now**, before the next redirect resolves; persist — not in `saved`).

Crucially, because `lower_one_redirect` for the next redirect runs *after*
`apply_one` of this one, `$v` is set (fixes `2>&$v`), the fd persists even if a
later redirect fails (fixes `{v}>f 2>&9` — this redirect already applied and is
not rolled back by the `?` on the next), and `dup_to_high_fd` sees the freed
temp fds (fixes `3>a {v}>x` numbering). The three in-process call sites
(`with_redirect_scope` ×2, `apply_redirects_permanently`) call `apply_redirects`.

On a mid-list error, the scope's existing `Drop` rolls back the `saved` entries
(temporary semantics) exactly as before; an already-applied `{var}` (not in
`saved`) persists — matching bash and pre-C. `reap_heredoc_writers` is called on
the error and success paths as today.

### Applier 2 — child, BATCH (unchanged from the C branch)

`build_child_redir_plan` keeps the batch model — it MUST, because the child
opens files in the parent and replays `dup2`/`close` in a `pre_exec` hook (open/
heredoc-spawn are not async-signal-safe, so they cannot move into the child).
Its loop calls `lower_one_redirect` for resolution and keeps the T4 `fd_state`
dup validation (the mechanism that earned the 8 external fixes), then
`redir_plan_to_child` translates to `ChildRedirPlan`. `$v` is NOT assigned in the
parent (bash doesn't for external). The child `{var}` numbering (#141) and
external `$v`-visibility (#140) remain as pre-existing divergences.

### What is deleted / reused

- Delete the C branch's batch `RedirectScope::apply_plan` (replaced by the
  interleaved `apply_redirects` + `apply_one`).
- Reuse: `PlanOp`, `RedirPlan`/`ChildRedirPlan`, `redir_plan_to_child`,
  `validate_plan_source`/`fd_state` (child only), `open_redirect_file` +
  `FdPlacement`, and the `lower_one_redirect` body (extracted from
  `lower_redirects` + `lower_named_fd`).
- The pre-C `apply`/`apply_var` stay deleted (their logic is now
  `lower_one_redirect` + `apply_one`, shared with the child).

## Testing

1. **`tools/redirect_audit.sh`** is the acceptance gate. Record the divergence
   set on `main` (24) and require this branch to be a strict subset that (a)
   contains none of the 6 in-process `{var}` cases and (b) retains the 8 external
   fixes — target: the 16 persistent divergences, zero new.
2. `fd_torture_diff_check.sh` 38/38 (incl. the T1 parity net + T4 truncation
   matrix) on both binaries. Add three cases pinning the fixed in-process `{var}`
   regressions: `{ …; } {v}>f 2>&$v` (E→f), `{v}>f 2>&9` (`$v` persists),
   `3>a {v}>x` (`$v=10`).
3. `named_fd_integration` 7/7 (external `{fd}>`=10 inheritance unbroken).
4. `huck-engine` lib green; full `run_diff_checks.sh` sweep 0-failed on debug+release.

## Branch / salvage

Start a fresh branch from `main` (`v292b-redirect-interleave`). Salvage the C
branch's `PlanOp`/`FdPlacement`/`redir_plan_to_child`/`fd_state`/tests wholesale;
the only genuinely new code is `lower_one_redirect` (a factoring of existing
code) and the interleaved `apply_redirects`/`apply_one` (a re-expression of the
deleted `apply` on top of the shared resolver). The C branch `v292-redirect-lowering`
stays unmerged as reference.
