# fd-plumbing Phase 3a — one redirect lowering: `lower_redirects()`

**Issue:** [#139](https://github.com/jdstanhope/huck/issues/139) — fd-plumbing Phase 3a:
consolidate redirect lowering into one `lower_redirects()`.

**Status:** design (v292). Behavior-preserving refactor. Closes no bug.

## Context

This is Phase 3a of the six-phase [fd-plumbing
remediation](2026-07-13-engine-fd-plumbing-review.md) (P0 = #134/v289, P1 =
#136/v290, P2 = #138/v291). The review splits Phase 3 into two shippable
iterations:

- **3a (this spec, v292)** — merge the redirect *op-resolution* logic, copy-pasted
  across three producers, into a single neutral lowering behind the existing call
  sites. Strictly behavior-preserving; no bug is closed.
- **3b (v293, separate spec)** — flip the pipeline stages off the lossy
  `RedirectSlot` fast-path onto `lower_redirects`, deleting `build_child_extra_ops`
  and the inline slot opens. 3b is what actually closes #50, #69, #77 (part), #78
  (message half) and makes #124/#125 single-site. It is the highest-risk step of
  the whole remediation and is deliberately isolated from this refactor.

## Problem

Today the logic that turns a `&Redirection` into "what fds this produces" —
expand the target word, check the restricted-shell guard, open the file via
`open_redirect_file` / spawn the heredoc-writer child / resolve the dup-source
word, classify move-vs-close, allocate the `{var}` high fd — is duplicated nearly
verbatim across **three** producers in `executor.rs`:

| Producer | Lines (main @ 35eed38) | Sink |
|---|---|---|
| `RedirectScope::apply` | 1008–1168 | applies to the shell's real fds now, save/restore |
| `RedirectScope::apply_var` (`{var}`) | 1177–~1450 | ditto, plus allocate high fd + assign `$var` |
| `build_child_redir_plan` | 5615–5900 | emits an ordered `ChildRedirOp` (dup2/close) replay list + holds `OwnedFd`s |

Only the **sink** differs: `apply` mutates real fds immediately with save/restore;
the child builder emits a replay op list and holds the opened `OwnedFd`s until the
fork. Everything upstream of the sink — the resolution — is the same code three
times. This is root cause **H1** (duplicated per-path wiring) in the review, and
it is why a redirect fix (v286's `RedirOp::Move`, #135's `{var}` relocation) has to
be hand-replicated at each site or a sibling is missed.

A fourth producer, `build_child_extra_ops` (5916–5985), is the *additive* helper
for the `RedirectSlot` pipeline fast-path. It is **out of scope for 3a** (see
Non-goals): it and the slot path are retired together in 3b.

## Goals

1. **One lowering.** A single `lower_redirects(&[Redirection], …) -> RedirPlan`
   owns all op-resolution. The semantic interpretation of each `RedirOp` exists
   exactly once.
2. **Two thin appliers.** The in-process path (`RedirectScope`) and the
   single-external-command child path (`run_subprocess`) both consume the same
   `RedirPlan` through their own small, mode-specific applier.
3. **Fold the P2 Minor:** replace `open_redirect_file`'s `relocate: bool` with a
   2-variant placement enum (`FdPlacement`).
4. **Byte-for-byte behavior preservation.** The bash-diff sweep +
   `fd_torture_diff_check.sh` staying green is the proof. No user-visible change.

## Non-goals (explicitly deferred)

- **The `RedirectSlot` fast-path stays.** `slots_for_simple_path`, the
  `slot_stdin/stdout/stderr` reads in `run_multi_stage`, the inline file-open
  blocks, `stage_extra_redirects`/`slot_consumes`, and `build_child_extra_ops`
  are **untouched** in 3a. They are retired in 3b, so refactoring them now is
  throwaway work.
- **No bug is closed.** #50/#69/#77/#78/#124/#125 remain open; they belong to 3b.
- **The two deferred behavior-changing P2 edges stay deferred:** the heredoc
  EMFILE-fallback `==target` edge (→ 3b) and std-managed capture pipes landing
  lowest-free (→ Phase 4). Only the `relocate`→enum *cleanup* rides along here
  because it is itself behavior-preserving and touches the exact code being moved.
- **No module extraction.** The consolidated lowering stays in `executor.rs` for
  3a. Carving a `redirect` submodule would require widening a large executor-private
  surface (`open_redirect_file`, `resolve_dup_source`, `spawn_heredoc_writer`,
  `StdoutSink`/`StderrSink`, `relocate_high_cloexec`, …) to `pub(crate)`, and 3b
  reshapes this code again — carve *after* 3b when the shape is final.

## Design

### The neutral plan

```rust
/// The result of lowering an ordered redirect list: what fds the command will
/// see, resolved but not yet installed. Consumed by exactly two appliers.
/// Ownership of every parent-opened temp lives INSIDE the ops (not a separate
/// `held` vec) so an applier controls each temp's close timing precisely, and a
/// lowering error drops them all (no leak; P1 discipline).
struct RedirPlan {
    /// Ordered ops. Source order is preserved (this is the whole point).
    ops: Vec<PlanOp>,
    /// Heredoc / here-string writer child pids, reaped after the body runs.
    heredoc_writers: Vec<libc::pid_t>,
}

enum PlanOp {
    /// A parent-opened temp (`>file`, heredoc/here-string read end) that must be
    /// duped onto `target`; it OWNS the fd. In-process: if `source`'s fd ==
    /// `target` (a relocated file that landed on its own target, target >= 10)
    /// clear FD_CLOEXEC in place and record a `-1` restore (the #135 mechanism),
    /// persisting the fd; else dup2 + save/restore, then close the temp
    /// IMMEDIATELY (so a later `{var}`/open reuses its freed number — holding it
    /// to end-of-plan would shift a subsequent `{var}` from fd 10 to 11). Child:
    /// dup2 (replay's `source == target` arm clears CLOEXEC), held until spawn.
    InstallOwned { target: RawFd, source: OwnedFd },
    /// A borrowed shell fd (`>&w` / `<&w`, and the dup half of a move). `source`
    /// is a resolved fd NUMBER. In-process: validate `source` is open, then dup2 +
    /// save/restore. Child: dup2 (no validation — the fd is inherited).
    InstallDup { target: RawFd, source: RawFd },
    /// `N>&-`, and the source-close half of a move (`>&w-`).
    Close { target: RawFd },
    /// `{var}` named-fd; OWNS the high fd. `high` is the live descriptor the
    /// command sees, already allocated non-CLOEXEC (>= 10). In-process: assign
    /// `$name = high` and `into_raw_fd()` it so it persists past the command
    /// (bash keeps it open until an explicit `{var}>&-` or shell exit). Child:
    /// keep `high` (the child inherits it, non-CLOEXEC), replay a defensive
    /// `dup2(high, high)`, and do NOT assign `$name` (bash doesn't for an external
    /// command).
    NamedFd { high: OwnedFd, name: String },
}
```

> **Why ownership lives in the ops, not a separate `held` vec.** The in-process
> applier must close each `InstallOwned` temp the instant after it dup2s it onto
> the target — exactly as the pre-refactor `apply` did — because a later `{var}`
> allocates its high fd with `dup_to_high_fd(_, 10, …)`, which takes the lowest
> free number >= 10. If a `>file` temp opened at fd 10 stayed open until
> end-of-plan, `>a {v}>x` would give `$v` fd 11 instead of bash's 10 — the same
> class of bug P2 fixed. Carrying the `OwnedFd` inside `InstallOwned`/`NamedFd`
> lets the applier `drop` (close) or `into_raw_fd` (persist) each fd at exactly
> the right moment; the child translation moves them into `ChildRedirPlan.held`.

### `lower_redirects` — the single resolver

```rust
fn lower_redirects(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<RedirPlan, i32>
```

Walks `redirects` in source order. For each entry it does exactly what the three
producers do today, emitting a `PlanOp` instead of applying or emitting a
`ChildRedirOp`:

- `RedirFd::Var(name)` → the `{var}` arm: resolve the source (File opened with
  `FdPlacement::RawLow`, or dup source, or heredoc read end), `dup_to_high_fd(src,
  10, false)` → `high`, close the owned source, push `high` to `held`, emit
  `NamedFd { high, name }`; for a move, also emit `Close { target: original_src }`.
  `{var}>&-` → resolve `$name` to a number, emit `Close`.
- `RedirOp::File { mode, word }` → expand, restricted-check, `open_redirect_file(mode,
  path, noclobber, FdPlacement::Relocated)` → `OwnedFd` (>= 10 + CLOEXEC), push to
  `held`, emit `InstallOwned { target, source: raw }`.
- `RedirOp::Dup { source } | Move { source }` → `resolve_dup_source` → number,
  emit `InstallDup { target, source }`; for a move emit `Close { target: source }`
  unless degenerate (`source == target`, bash's `redir_fd != redirector` no-op).
- `RedirOp::Close` → emit `Close { target }`.
- `RedirOp::Heredoc | HereString` → expand body, `spawn_heredoc_writer`, push pid,
  `relocate_high_cloexec` the read end, push `OwnedFd` to `held`, emit
  `InstallOwned { target, source: rfd }`.

**Critical invariant — no eager validation.** `lower_redirects` resolves dup
*words* to fd *numbers* (position-independent: `&1` is always the number 1) but
does NOT validate that a dup source is currently open, and does NOT check that a
File-target fd exists. Validation is inherently apply-time for the in-process path:
in `3>file 4>&3`, `&3`'s validity depends on the earlier `3>file` having been
applied to the real fd first. The in-process applier therefore validates
`InstallDup` sources lazily, right before its dup2 — exactly as `apply` does today.
The child path never validated (the child inherits the fds) and still doesn't.
This is why the plan is faithful despite being lowered up front: the *following of
current fd contents* happens at dup2 time in both appliers, in source order.

### Applier 1 — in-process (`RedirectScope::apply_plan`)

`RedirectScope` stops walking `Redirection`s. `apply`/`apply_var` are replaced by:

```rust
impl RedirectScope {
    fn apply_plan(
        &mut self,
        plan: RedirPlan,
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome>;
}
```

It absorbs `plan.heredoc_writers` into `self.heredoc_writers`, then walks
`plan.ops` in order:

- `InstallOwned { target, source }` (owns the fd) → if `source`'s fd == `target`,
  `into_raw_fd()` to persist it, in-place `FD_CLOEXEC` clear +
  `self.saved.push((target, -1))` (the existing #135 arm); else
  `self.redirect(source_raw, target)` (dup2 + save) then `drop(source)` to close
  the temp IMMEDIATELY (matches the pre-refactor close-after-dup2, so a later
  `{var}`/open reuses the freed number). On dup2 error, `source` drops (closes)
  on the early return.
- `InstallDup { target, source }` → `validate_fd_open(source)`, then
  `self.redirect(source, target)`.
- `Close { target }` → `self.close_target(target)`.
- `NamedFd { high, name }` → `let fd = high.into_raw_fd()` (persist; not closed on
  Drop, bash keeps it), then assign `$name = fd`. Not added to `self.saved`.

Drop-rollback (`saved` restored in reverse) and heredoc reaping are unchanged. The
three call sites (`with_redirect_scope` ×2 at 1523/1639, and `exec` at 5378) change
from a per-redir `scope.apply(r, …)` loop to `let plan = lower_redirects(…)?; scope.apply_plan(plan, …)?`.
`apply_plan` takes `plan` by value so it owns each temp `OwnedFd` and closes or
persists it at exactly the right moment (see the ownership note above).

### Applier 2 — child replay (`run_subprocess`)

`build_child_redir_plan` becomes a thin adapter (or is replaced at the call site,
5221): call `lower_redirects`, then translate `RedirPlan` into the existing
`ChildRedirPlan { ops: Vec<ChildRedirOp>, held, heredoc_writers }` that
`run_subprocess`/`replay_redir_ops` already consume — OR extend `replay_redir_ops`
to consume `PlanOp` directly. Either way the translation is mechanical:

- `InstallOwned { target, source }` → `ChildRedirOp::Dup { target, source: raw }`
  and move the `OwnedFd` into `ChildRedirPlan.held` (kept alive until spawn).
- `InstallDup { target, source }` → `ChildRedirOp::Dup { target, source }` (no held).
- `Close { target }` → `ChildRedirOp::Close { target }`.
- `NamedFd { high, .. }` → `ChildRedirOp::Dup { target: raw, source: raw }`
  (the existing defensive same-fd op) and move `high` into `held`; do not assign
  `$var`.

The translation REBUILDS `ChildRedirPlan.held` from the ops' owned fds;
`heredoc_writers` moves across unchanged. `replay_redir_ops` is unchanged (it
already handles `source == target`). This preserves the child path byte-for-byte.

**Recommendation:** keep the `ChildRedirPlan` → `ChildRedirOp` bridge (translate
`PlanOp` into the existing child op list) rather than rewriting `replay_redir_ops`
to take `PlanOp`. It is the smaller, lower-risk diff and keeps the
async-signal-safe replay hook (which must not allocate/branch on `String`)
exactly as it is — `NamedFd`'s `name: String` never reaches `pre_exec`.

### The `relocate`→enum cleanup (P2 Minor)

```rust
enum FdPlacement {
    /// Relocate the opened fd to >= 10 and set FD_CLOEXEC (redirect targets on
    /// real fds; the source must survive out of the 0..9 swap range).
    Relocated,
    /// Return the raw low File fd as opened (CLOEXEC). Used only by the `{var}`
    /// arm, which relocates once itself via `dup_to_high_fd` — relocating here
    /// too double-relocates the named fd (fd 11 vs bash's 10, the #135 regression).
    RawLow,
}

fn open_redirect_file(mode: &FileMode, path: &str, noclobber: bool,
                      placement: FdPlacement) -> io::Result<OwnedFd>;
```

Every current `relocate: true` site passes `FdPlacement::Relocated`; the two
`{var}` `relocate: false` sites pass `FdPlacement::RawLow`. Pure rename; the
double-relocation-warning comment travels with the `RawLow` call sites.

## Testing

Behavior preservation is the entire contract, so the test strategy is regression
nets, not new feature tests:

1. **`fd_torture_diff_check.sh` (23 cases)** — the redirect/freed-fd/heredoc/`{var}`
   net from P1/P2, including the four #135 flavors. Must stay 23/23 on both binaries.
2. **Full bash-diff sweep** (`run_diff_checks.sh`, 188 harnesses) on debug **and**
   release. Must stay green. The redirect-specific harnesses
   (`redirect_regen`, `process_sub`, `error_message`, `xtrace_compound`, the
   heredoc/here-string cases) are the direct guard.
3. **`named_fd_integration`** (7 cases) — `{fd}>` must still assign fd 10 and the
   external child must still inherit it (the exact P2 regression class). Run the
   `-p huck` integration binary single-threaded locally before pushing.
4. **`huck-engine` lib** (~1806 tests) single-threaded per the OOM constraint.
5. **In-process ⇄ child parity spot-checks added to `fd_torture`** (if not already
   covered): `3>file 4>&3` (lazy validation), `exec {v}>f; echo $v` (`{var}`
   persists in-process), `{ echo hi; } {v}>f` vs `echo hi {v}>f | cat` (in-process
   vs child `{var}`), `2>&1 >file` ordering in a compound vs a single external.

CI runs the sweep and the `--workspace` threaded suite; run per-crate + the touched
integration binaries locally first (memory: this box OOMs on `--workspace`).

## Risks

- **The `{var}` asymmetry** (in-process assigns `$var` + persists; child doesn't
  assign + child-inherits) is the one place the two appliers genuinely diverge.
  It is localized to the `NamedFd` arm of each applier; the resolution (high-fd
  alloc) is shared. The `named_fd` integration + `{var}` fd_torture cases guard it.
- **Held-fd close timing.** In-process MUST close each File/heredoc temp
  immediately after its dup2 (via `drop(source)` inside `apply_plan`), NOT at
  end-of-plan — otherwise a subsequent `{var}` allocates a shifted high fd
  (`>a {v}>x` → `$v` = 11 not 10). The `3a namedfd` + `3a order` fd_torture cases
  guard this exactly.
- **Async-signal safety.** `PlanOp::NamedFd` carries a `String`; it must be
  translated to a `ChildRedirOp` (no `String`) *before* `pre_exec`. Keeping the
  `ChildRedirPlan` bridge (not feeding `PlanOp` into `replay_redir_ops`) enforces this
  structurally.
- **Ordering.** Every applier walks `plan.ops` strictly in source order; a bug that
  reorders (e.g. emitting a move's `Close` before its `InstallDup`) breaks
  `3>&1 1>&2 2>&3`-style swaps. The fd_torture swap cases guard it.
