# v290 — fd-plumbing remediation, Phase 1: `ChildFd`/`ChildStdio` (kill the raw-fd sentinels)

**Issues:**
- Primary — [#132](https://github.com/jdstanhope/huck/issues/132) (`divergence`
  + `bug` + `sev:medium`): a pipeline-stage file redirect whose fd lands on a
  freed 0/1/2 vanishes on exec. **Closed by this PR.**
- [#78](https://github.com/jdstanhope/huck/issues/78) (`divergence` + `bug` +
  `sev:low`): pipeline-stage spawn-failure leak + wrong message. This phase
  addresses the **fd-leak half only**; the wrong-message half is Phase 3.
- **Implementation finding:** the §H2b compound repro originally attributed to
  a `fork_and_run_in_subshell` spawner sibling turned out, on investigation, to
  be a **distinct, pre-existing bug in the in-process whole-command redirect
  path** (`apply_redirections`/`RedirectScope`) — it reproduces with **no
  pipeline at all** and on the pre-Phase-1 baseline. Filed as
  [#135](https://github.com/jdstanhope/huck/issues/135) (`divergence` + `bug`,
  **open**, deferred to Phase 3). **This PR does NOT close #135.** See the
  "Implementation finding" note after the Problem section for the full
  correction.

**Parent context:** Phase 1 of the 6-phase (0–5) fd/redirect/process-launch
remediation in
`docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md` (§4
"Phase 1"). Phase 0 shipped as v289 (#128/#129 + the `fd_torture` net). This
phase is scoped, per maintainer decision:
- **(A) types-only new module** `crates/huck-engine/src/child_fd.rs` holding the
  new types + their construction/duplication helpers ONLY. The two spawner
  functions STAY in `executor.rs`; only their signatures change. Moving the
  spawners into the module is a deliberate later follow-up (a non-goal here).
- **Both spawners converted this iteration** — `spawn_external_with_fds` AND
  `fork_and_run_in_subshell` — so the whole #132 class is fixed at once. The
  fork spawner's pre-move additionally **hardens** the fork path against the
  freed-fd/CLOEXEC class defensively; the compound-redirect divergence
  (originally thought to be a spawner sibling reachable through this path)
  turned out to be #135, the in-process path, deferred to Phase 3.
- A CONCRETE owned type + dedicated module, **not** a polymorphic trait
  (maintainer decision, 2026-07-13; recorded in the review §4).

Line numbers below are approximate on `main` @ `958061f` (post-v289) and will
drift; **function names are the stable handles.**

---

## Problem

The engine has no single owned representation of "the fd environment a child
will start with." Each launch path re-derives child stdio from raw `RawFd`
integers whose *meaning* — "explicitly redirected file" vs "inherit the shell's
std stream" vs "pipe end I must close" — is encoded in the **fd number itself**
(0/1/2 vs >2). The two spawner functions then *guess the meaning back* from the
number:

- `spawn_external_with_fds` (`executor.rs` ~:8956–8987): `stdin_fd == 0 →
  Stdio::inherit()`, else `OwnedFd::from_raw_fd`. A freshly opened redirect
  `File` (Rust std ⇒ `O_CLOEXEC`) that lands on a freed fd 0/1/2 is misrouted
  into the `inherit()` branch, so nobody clears CLOEXEC and the fd **vanishes on
  exec** (this IS #132).
- `fork_and_run_in_subshell` (`executor.rs` ~:8698–8712): `if stdin_fd != 0 {
  dup2 }`, `if fd > 2 { close }`. An InProcess stage whose slot-opened CLOEXEC
  file landed on freed fd 0/1/2 skips the CLOEXEC-clearing dup2; if that
  in-process child then execs an external grandchild, the fd would vanish on
  the grandchild's exec — same class as #132, distinct spawner. **No
  reproduced case actually flows through this spawner today** (see below); the
  conversion here is defensive hardening against the class, not a fix for an
  observed repro.

Two repros were considered. The **external** repro is **confirmed divergent on
the v289 release binary** and IS the #132 spawner bug fixed by this PR (bash
prints the file contents + `end`; huck prints `Bad file descriptor`):

```
printf 'x\n' > inA
# #132 — External stage (fixed by this PR):
huck -c 'exec <&-; /bin/cat < inA | /bin/cat; echo end'
#   huck: /bin/cat: -: Bad file descriptor        bash: x + end
```

The **compound** repro was originally assumed to reach `fork_and_run_in_subshell`
and be closed by the same fix. Investigation during Task 2 showed otherwise: it
reproduces even with **no pipeline at all**, and on the **pre-Phase-1 baseline**,
so it is not a spawner bug — it is a distinct, pre-existing bug in the
in-process whole-command redirect path (`apply_redirections`/`RedirectScope`).
Filed as [#135](https://github.com/jdstanhope/huck/issues/135), open, deferred
to Phase 3; **not closed by this PR**:

```
# #135 — in-process whole-command redirect on a freed std fd (NOT fixed here):
huck -c 'exec <&-; { /bin/cat; } < inA; echo end'
#   diverges from bash with NO pipeline present — confirms it is not a
#   spawner/pipeline-stage bug.
```

The fix makes illegal states unrepresentable: carry *meaning + ownership* in a
type, leaning on `std::os::fd::OwnedFd` for RAII (single-ownership,
close-on-drop, CLOEXEC travels with the fd). Then leak and double-close become
type errors (#78 leak half), and an `Owned` fd numbered 0/1/2 is dup2'd like any
other fd (#132; the fork spawner's own conversion hardens against the same
class defensively, though no reproduced case flows through it today).

**Implementation finding:** during Task 2, the compound repro above (`{ ...; }
< inA` on a freed fd) was found to reproduce identically with the pipeline
removed and on the pre-Phase-1 baseline binary — proving it is unrelated to
`fork_and_run_in_subshell` or pipeline staging at all. The original design
mis-attributed it as a "§H2b" spawner sibling to be filed and closed alongside
#132; that attribution was wrong. It has instead been filed as
[#135](https://github.com/jdstanhope/huck/issues/135) (`divergence` + `bug`,
open, deferred to Phase 3, in the in-process `apply_redirections`/
`RedirectScope` path) and is explicitly **not** closed by this PR. This spec
has been corrected throughout to reflect that; any remaining "§H2b" label
below refers only to the fork spawner's own freed-fd/CLOEXEC hardening, not to
a closed issue.

### `Stdio::from(OwnedFd)` verification (settled — do not re-litigate)

The review's open risk was whether `Stdio::from(OwnedFd)` handles an owned fd
whose NUMBER already equals its target slot (0/1/2). **Resolved: std handles it
correctly; no `pre_exec` dup2 fallback is needed.** Two confirmations:

1. **std source** (toolchain 1.95.0,
   `library/std/src/sys/process/unix/common.rs:407–413`): a `Stdio::Fd` whose
   raw fd is `0..=2` is `duplicate()`d in the parent (`F_DUPFD_CLOEXEC` to ≥ 3)
   and the child `dup2`s the duplicate onto the slot (`unix.rs:285–293`, and via
   `posix_spawn_file_actions_adddup2` on the posix_spawn path). `dup2` with
   source ≠ dest clears `FD_CLOEXEC` on the target, so the aliasing is
   impossible.
2. **Empirical probe** (rustc 1.95.0): close fd 0; `File::open` lands on fd 0
   with `FD_CLOEXEC` set (verified via `fcntl`);
   `Command::new("cat").stdin(Stdio::from(owned))` → child reads the FILE, exit
   0 — on BOTH std spawn paths (plain, and with a registered `pre_exec`, which
   is huck's case since it always chains pre_execs and thus forces the
   fork/exec path).

**MSRV:** the workspace pins no `rust-version`; edition 2024 already requires
≥ 1.85, and `From<OwnedFd> for Stdio` (stable 1.74) + the equal-fd `duplicate()`
handling both predate that. No MSRV action.

---

## Design

### §1 — The types & module (`crates/huck-engine/src/child_fd.rs`)

Register in `lib.rs` as `pub(crate) mod child_fd;`. Everything is `pub(crate)`.
Types + construction/duplication helpers ONLY — no spawn logic, no redirect
lowering (Phase 3's `lower_redirects` joins this module later per the review's
end-state).

```rust
/// One child stdio slot.
#[derive(Debug)]
pub(crate) enum ChildFd {
    /// Leave the slot alone: the child uses the shell's real fd N.
    Inherit,
    /// A parent-opened fd destined for slot N. The spawner consumes it: dup2
    /// onto N in the child (CLOEXEC cleared by dup2), closed in the parent
    /// after spawn/fork — including on every error path (RAII).
    Owned(OwnedFd),
}

/// The fd environment a child starts with: one `ChildFd` per stdio slot.
#[derive(Debug)]
pub(crate) struct ChildStdio {
    pub(crate) stdin: ChildFd,
    pub(crate) stdout: ChildFd,
    pub(crate) stderr: ChildFd,
}
```

**Exactly three fields** — see Non-goals for the deliberately deferred `extra`.

Helper surface (all `pub(crate)`):

- `ChildFd::owned_raw(fd: RawFd) -> Self` (`unsafe`) — wrap a raw fd this site
  exclusively owns; the bridge for the many `make_pipe`/`into_raw_fd` sites that
  still traffic in `RawFd` (Phase 2 migrates those creation sites to `OwnedFd`).
- `ChildFd::raw(&self) -> Option<RawFd>` — the fd number if owned; for
  close-list bookkeeping ONLY, never for inherit/close decisions.
- `ChildFd::try_clone(&self) -> io::Result<Self>` — non-consuming duplicate
  (`Inherit → Inherit`; `Owned` → `F_DUPFD_CLOEXEC`). For the bg stage-0
  `/dev/null` default reused across several stages.
- `ChildFd::try_clone_resolving(&self, slot: RawFd) -> io::Result<Self>` —
  clone that RESOLVES `Inherit` against the shell's real fd at `slot`
  (`BorrowedFd::borrow_raw(slot).try_clone_to_owned()`). For kernel-level merged
  stderr ("stderr := a copy of whatever stdout will be"): cloning stdout's
  `ChildFd` for the stderr slot must dup the real fd 1 when stdout is `Inherit`.
- `ChildFd::into_raw(self) -> Option<RawFd>` — consume into a raw fd WITHOUT
  closing (`Inherit → None`). The fork child's Drop-safety primitive.
- `From<OwnedFd> for ChildFd`, `From<File> for ChildFd`.
- `ChildStdio::new(stdin, stdout, stderr)`, `::inherit_all()`,
  `::owned_raws(&self) -> impl Iterator<Item = RawFd>` (owned slots' numbers,
  skipping `Inherit`).

**No `Clone` derive** on either type — `OwnedFd` is affine, so aliasing a child
fd (today's Merged `stderr_fd = stdout_fd`, and the shared bg stage-0 default)
must go through an explicit `try_clone*` that performs a real `dup`. This is
what eliminates the §5 double-close latent bug. No custom `Drop` — `OwnedFd`
closes on drop; `Inherit` has nothing to close.

**Fork/Drop safety contract** (load-bearing; stated in the module doc, enforced
by construction):

1. *External spawner*: every `OwnedFd` is converted to `Stdio` (or dropped) in
   the PARENT before `spawn()`. Nothing owned crosses into `pre_exec`; the
   pre_exec closures keep capturing only `Vec<RawFd>`/`i32` (async-signal-safe
   close/dup2), exactly as today.
2. *Fork spawner*: the child branch destructures `ChildStdio` and calls
   `into_raw()` on all three slots as its FIRST act — after that point no
   `OwnedFd` exists in the child, so neither the `_exit` path nor a
   panic-unwind can run a destructor over an fd number that has been
   dup2'd/closed/reused. `into_raw_fd` does not allocate. The parent branch
   still owns the `ChildStdio` (the child branch diverges via
   `libc::_exit(..) -> !`) and drops it right after fork, closing the parent's
   copies.

Unit tests live in a sibling `child_fd/tests.rs` (`#[cfg(test)] mod tests;`,
house style since v278). They manipulate FRESH fds only (pipes / `/dev/null`
opens), never process-global 0/1/2, so they are safe in the lib test binary
(the huck-test-fd-isolation rule applies to tests that swap real std fds; these
don't).

### §2 — `spawn_external_with_fds` conversion

Current signature (`executor.rs` ~:8833) takes `stdin_fd, stdout_fd, stderr_fd:
RawFd`. New signature takes `stdio: ChildStdio` by value (consumed; closed on
ALL paths):

```rust
fn spawn_external_with_fds(
    cmd: &SimpleCommand, shell: &mut Shell,
    sink: &mut StdoutSink, err_sink: &mut StderrSink,
    stdio: ChildStdio,                       // consumed; closed on all paths
    pgid_target: i32, parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error>
```

The sentinel block (~:8956–8987) is deleted, replaced by matches:

```rust
let stdin_stdio = match stdio.stdin {
    ChildFd::Inherit    => Stdio::inherit(),
    ChildFd::Owned(fd)  => Stdio::from(fd),
};
let stdout_stdio = if stdout_dup_target.is_some() {
    drop(stdio.stdout);            // was a manual libc::close at ~:8964–8968
    Stdio::inherit()               // the dup2 pre_exec applies the real redirect
} else {
    match stdio.stdout { ChildFd::Inherit => Stdio::inherit(),
                         ChildFd::Owned(fd) => Stdio::from(fd) }
};
// stderr: identical shape to stdout (dup-target arm mirrors ~:8975–8987).
```

Unchanged: resolve/xtrace, dup-target resolution, `build_child_extra_ops`, the
chained pre_execs, `process_group`, the `mem::forget(child)` (B-09) pattern, and
the `fds_to_close` filter against `extra_targets`.

**#78 leak-half fix, by construction:** today the early `return Err(...)`s
(resolve failure ~:8861, `build_child_extra_ops` failure ~:8887) exit BEFORE the
fds are consumed, while both callers assume `went_external ⇒ consumed` — so the
stage's stdin/stdout/stderr fds leak. With `stdio: ChildStdio` taken by value,
every early return drops it → closed. No extra caller work beyond the
uniform-ownership switch.

### §3 — `fork_and_run_in_subshell` conversion

Current signature (`executor.rs` ~:8659) BORROWS its fds (the caller closes).
New signature CONSUMES `ChildStdio`, symmetric with the external spawner (this
is what lets `went_external` be deleted):

```rust
pub fn fork_and_run_in_subshell(
    cmd: &Command, shell: &mut Shell,
    stdio: ChildStdio,                       // consumed; parent copies closed
    pgid_target: i32, parent_fds_to_close: &[RawFd],
    stdout_dup_target: Option<i32>, stderr_dup_target: Option<i32>,
) -> Result<i32, io::Error>
```

**Child-side install** (replaces today's steps 3–5, ~:8697–8722). Today's
sentinels — `if stdin_fd != 0 { dup2 }` (skips the CLOEXEC-clearing dup2 when an
owned fd lands on its own slot — hardens the fork path against the freed-fd/
CLOEXEC class defensively; the compound repro that motivated looking here
turned out to be #135, the in-process path) and `if fd > 2 { close }` (leaks
owned fds numbered ≤ 2) — become a 3-pass sequence, mimicking std's shape from
§2:

```rust
// CHILD (single-threaded; fcntl/dup2/close are async-signal-safe).
// Convert to raw NOW: no OwnedFd may live in the forked child (Drop hazard).
let ChildStdio { stdin, stdout, stderr } = stdio;
let mut plan: [(Option<RawFd>, RawFd); 3] =
    [(stdin.into_raw(), 0), (stdout.into_raw(), 1), (stderr.into_raw(), 2)];
let original_raws: Vec<RawFd> = plan.iter().filter_map(|(s, _)| *s).collect();

// Pass 1 (PRE-MOVE): move any owned source sitting in 0..=2 up to >=3, closing
// the original. This (a) makes pass 2 order-independent (an owned fd parked on
// ANOTHER slot's number can't be clobbered before its own install) and (b)
// guarantees pass 2's dup2 has source != target, so dup2 always clears
// FD_CLOEXEC on the slot — hardens the fork path against the freed-fd/CLOEXEC
// class (defensive; the compound repro turned out to be #135, the in-process
// path). F_DUPFD (not _CLOEXEC): the moved copy must survive exec if its slot
// install is a no-op.
for (src, _) in plan.iter_mut() {
    if let Some(s) = *src && s <= 2 {
        let moved = libc::fcntl(s, libc::F_DUPFD, 3);   // lowest free fd >= 3
        if moved >= 0 { libc::close(s); *src = Some(moved); }
        // On failure keep s: degraded to today's behavior, never worse.
    }
}
// Pass 2 (INSTALL): sources are now all >=3 and pairwise distinct (exclusive
// ownership), so no aliasing / double close.
for (src, slot) in plan {
    if let Some(s) = src { libc::dup2(s, slot); libc::close(s); }
}
// Pass 3 (was step 5): close parent-held pipe fds, skipping this child's own
// stdio sources by their ORIGINAL numbers.
for &fd in parent_fds_to_close {
    if !original_raws.contains(&fd) { libc::close(fd); }
}
// Step 6 (dup targets), traps-clear, run_command dispatch, _exit: unchanged.
```

`Inherit` slots (`None` in the plan) are never touched — identical to today's
`stdin_fd == 0` fast path when the caller really means "the shell's stdin."

**Parent-side ownership shift:** the parent branch (after fork, ~:8774) drops
the `ChildStdio` at scope end, closing the parent's copies of all `Owned` fds;
fork failure (~:8675) drops them too. Consequence for callers: **every
post-call and error-path `libc::close` of the three stdio fds is deleted** (they
would now be double-closes). `parent_fds_to_close` stays `&[RawFd]` (those fds
remain parent-owned raws until Phase 2). One deliberate timing change: parent
copies of pipe write-ends now close immediately after fork instead of a few
statements later in each caller — strictly earlier EOF propagation, the
direction every caller already wants (the child has its own fork copies).

### §4 — Caller conversion inventory (all 11 sites)

`fork_and_run_in_subshell`: 9 call sites; `spawn_external_with_fds`: 2. All on
`main` @ `958061f` (line numbers approximate).

| # | Caller (fn, ~site) | Passes today | Builds under Phase 1 | Close-bookkeeping change |
|---|---|---|---|---|
| F1 | `run_command` Subshell arm, ~:671 | stdin=`STDIN_FILENO`; stdout=`STDOUT_FILENO` \| capture-pipe w; stderr=`STDERR_FILENO` \| Merged→`stdout_fd` alias \| err-capture-pipe w | stdin=`Inherit`; stdout=`Inherit`\|`owned_raw(w)`; stderr=`Inherit`\|`stdout.try_clone_resolving(1)?` (Merged)\|`owned_raw(err_w)` | Deletes post-fork closes of `stdout_fd`/`stderr_fd` (~:730–741) + fork-error closes (~:704–713); capture READ ends stay caller-managed raws |
| F2 | `run_background_subshell`, ~:3120 | stdin=`STDIN_FILENO` \| devnull fd from `async_default_stdin`; stdout/stderr inherit | change `async_default_stdin` (~:3072) to return `Result<ChildFd, ()>` (`Inherit`\|`Owned(File)`); pass `ChildStdio::new(stdin, Inherit, Inherit)` | Deletes manual devnull close ~:3131–3136; the `AsyncStdin` enum (~:3060) retires |
| F3 | `run_background_sequence` assign-only stage, ~:3268 | stdin=`stage0_stdin_default`; stdout=pipe w \| `STDOUT`; stderr inherit | stdin=`stage0_default.try_clone()?`; stdout=`owned_raw(w)`\|`Inherit`; stderr=`Inherit` | Deletes stdout closes ~:3280–3285, ~:3314–3318 |
| F4 | `run_background_sequence` InProcess stage, ~:3881 | stdin=slot-file/heredoc-r/prev-pipe-r/default; stdout=explicit file\|pipe w\|`STDOUT`; stderr=explicit file\|`STDERR` | stdin=`from(File)`\|`owned_raw(r)`\|`stage0_default.try_clone()?`; stdout=`from(File)`\|`owned_raw(w)`\|`Inherit`; stderr=`from(File)`\|`Inherit` | **Deletes `went_external` (~:3863) + all `fd > 2` close blocks** (~:3904–3919, ~:3937–3956); per-stage error-path closes (~12 sites, ~:3484–3677) collapse to RAII drops before `bail_teardown_bg` |
| F5 | `run_coproc`, ~:6674 | stdin=`in_r`, stdout=`out_w`; stderr inherit; close-list `[in_w, out_r]` | stdin=`owned_raw(in_r)`, stdout=`owned_raw(out_w)`, stderr=`Inherit` | Deletes post-fork `close(in_r); close(out_w)` (~:6707–6710) and those two from the fork-error block (~:6687–6692); `in_w`/`out_r` stay caller's |
| F6 | `run_multi_stage` assign-only stage, ~:7033 | stdin=`STDIN_FILENO`; stdout=pipe w \| capture w \| `STDOUT` | stdin=`Inherit`; stdout=`owned_raw(w)`\|`Inherit` | Deletes stdout close ~:7046–7054 |
| F7 | `run_multi_stage` InProcess stage, ~:7679 | as F4, plus stderr=Merged alias (`stdout_fd`, ~:7517) \| per-stage `libc::dup` of the shared capture write-end (~:7521) | as F4; Merged→`stdout.try_clone_resolving(1)?`; err-capture→`owned_raw(dup(shared))` (mechanics unchanged) | **Deletes `went_external` (~:7658) + blocks** (~:7705–7721, ~:7756–7785); error paths (~:7403–7415, ~:7449–7457, ~:7482–7491, ~:7535–7551) collapse to drops |
| F8 | `procsub::realize_via_devfd`, `procsub.rs` ~:75 | In: stdin=`STDIN`, stdout=`write_fd`; Out: stdin=`read_fd`, stdout=`STDOUT`; stderr inherit | the pipe end destined for the child → `owned_raw`; the parent-kept end stays a raw in `ProcSub` (Phase 2/5) | Deletes the post-fork `close(inner_end)` (~:96–98); the `inspect_err` (~:86–89) now closes ONLY the parent-kept end |
| F9 | `procsub::realize_via_fifo`, `procsub.rs` ~:175 | all three std fds | `ChildStdio::inherit_all()` | none |
| E1 | `run_background_sequence` External stage, ~:3867 | `stdin_fd, stdout_fd, stderr_fd` raws | the SAME `ChildStdio` as F4, built ONCE before `classify_stage` and moved into whichever spawner | merged with F4's simplification |
| E2 | `run_multi_stage` External stage, ~:7665 | same | the SAME `ChildStdio` as F7 | merged with F7's simplification |

Shared mechanics for the two pipeline functions (F4/E1, F7/E2):

- The `ChildStdio` is built once per stage, before `classify_stage`
  (~:3864/~:7659), and moved into whichever spawner the classification picks.
  `went_external` and both of its close-bookkeeping arms are **deleted
  outright** — the review's promised shrink becomes a full removal in Phase 1.
- `parent_held` entries are removed at wrap time (when a held raw becomes
  `Owned`), so `fds_to_close_in_child` becomes "all remaining `parent_held`" —
  the `filter(fd != stdin/stdout/stderr)` at ~:3779–3783 / ~:7572–7576 goes
  away.
- **bg stage-0 default (per-stage clone — accepted):** `stage0_stdin_default`
  (~:3192–3200) becomes a parent-local `stage0_default: ChildFd` (NOT in
  `parent_held`; RAII covers every bail path). Each consuming stage takes
  `stage0_default.try_clone()?` — one `dup` per stage. This preserves today's
  exact child fd-table (an assign-stage chain like `a=1 | b=2` feeds the
  default to several children, and children close the parent's copy): the
  parent's `stage0_default.raw()` is appended to `fds_to_close_in_child` when
  `Some`, and the original drops at function end. Net fd delta per child: zero
  (clone consumed + original in its close list).
- Per-stage bail shape: constructed `ChildFd`s drop on any early return
  (replacing the ~12 hand-written `if fd > 2 close` / `close(explicit_*)`
  blocks per function); `bail_teardown_bg` / `bail_teardown_stage` keep closing
  the remaining raw `parent_held` entries, unchanged.

### §5 — The double-close latent bug the type eliminates

Today a Merged-stderr External stage reaches `spawn_external_with_fds` with
`stderr_fd == stdout_fd > 2` (the Merged arm aliases the two, ~:7517) and
constructs TWO `OwnedFd::from_raw_fd` over the SAME fd (~:8973 + ~:8986). Under
modern std's IO-safety (EBADF-on-close abort), that double-close is a live
hazard, not just untidy. The no-`Clone` `ChildStdio` makes the alias
unrepresentable: the Merged path must go through `try_clone_resolving(1)`, a
real `dup`, so the two slots hold DISTINCT owned descriptors — observationally
identical (both end in dup2 onto the same file description) but each closed
exactly once. This is a correctness win the type buys for free.

---

## Testing

All commands from the repo root. This box OOM-kills on `--workspace`, so
per-crate, single-threaded, guarded:

1. **Unit tests** (new `child_fd/tests.rs` + existing executor tests):
   `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. New tests
   (FRESH fds only — no 0/1/2 swapping, safe in the lib module):
   - `try_clone` maps `Inherit → Inherit` and `Owned → distinct fd, same file`.
   - `try_clone_resolving` on `Inherit` dups the given slot's fd (use a fresh
     pipe fd as the "slot", not a real 0/1/2).
   - `into_raw` does not close (`fcntl(F_GETFD)` still succeeds afterward);
     dropping an `Owned` DOES close (`F_GETFD` → EBADF).
   - `owned_raws` skips `Inherit` slots.
2. **Binaries:** `cargo build -p huck` (debug) and
   `cargo build --release --locked --bin huck` — the sweep needs both.
3. **Flip 3 `fd_torture` cases green** in
   `tests/scripts/fd_torture_diff_check.sh` (they are excluded today; update the
   header comment to exclude only #50 and #135 going forward). Verify each
   case's bash output shape when writing them (byte-identical rule; external
   commands where builtin wording diverges):
   - `#132` External — `exec <&-; cat < inA | cat; echo end`
   - freed fd1 stdout-to-file — `exec >&-; /bin/echo hi > f | cat; cat f`
   - bg pipeline file redirect — `exec <&-; cat < inA | cat & wait; echo end`

   The compound InProcess case (`{ /bin/cat; } < inA | /bin/cat`) is NOT added
   here — it is #135, still red, deferred to Phase 3.
4. **Full bash-diff sweep:** `tests/scripts/run_diff_checks.sh` on the intended
   per-harness default binaries (never override `HUCK_BIN`), guarded:
   `ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh`.
5. **`-p huck` integration binaries** (they run locally — the v289 lesson). Run
   the fd/pipeline-relevant ones first, then sweep ALL before the PR, each as
   `ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`.
   Highest-value: `bg_sequence`, `coproc`, `subshell`, `subshell_pipeline`,
   `pipeline_subshell`, `subshell_pipeline_position`, `fd_dup`, `named_fd`,
   `external_fd_redirects`, `builtin_fd_ordering`, `builtin_stdout_dup`,
   `builtin_pipe_flush`, `compound_redirects`, `heredoc_forked_writer`,
   `heredoc`, `here_string`, `process_sub`, `captured_pipeline_drain`,
   `pipefail`, `sigpipe`, `io_error`, `noclobber`, `wait`, `async_list`,
   `disown_h`, `disown_pid`, `jobs_flags`, `exit_inherits`,
   `function_redirect`, `cmdsub_subshell`. The `*_pty` binaries exercise the
   fork paths under a tty — include `subshell_pipeline_pty`, `procsub_stop_pty`,
   `jobcontrol_pgroup_pty`.
6. **Manual spot-checks** (not automatable byte-diff): the two Problem-section
   repros vs bash; `coproc` round-trip; `<(cmd)` still works
   (`diff <(echo a) <(echo a)`).

---

## Non-goals

- **The `extra: Vec<(RawFd, ChildFd)>` field** from the review's sketch —
  `ChildStdio` has EXACTLY three fields this phase. `extra` replaces
  `build_child_extra_ops` and is Phase 3; adding it now would be
  written-never-read dead code (the repo's dead_code-unmasking history says
  don't). Decided.
- **Moving the spawners into `child_fd.rs`** — scope (A) is types-only; the
  spawner extraction is a deliberate later follow-up.
- **The #78 wrong-message half** — Phase 3 (the single `lower_redirects`
  lowering site owns the diagnostic prologue). Only the leak half lands here.
- **`RedirectSlot` retirement / #50 stage source-order** — Phase 3.
- **procsub pgroup / #97 / #45, universal fd hygiene at creation (#130-class
  latents), the pipe-creation policy merge** — Phases 2 and 5. F8 only wraps
  the child-destined pipe end as `Owned`; the raw `libc::pipe` and the
  parent-kept end are left for those phases.

---

## Risks

1. **Breadth, not depth** — 11 call sites across 2 files, each with bespoke
   error-path closes to DELETE. A missed deletion is a double-close (loud via
   std IO-safety abort if through `OwnedFd`, silent-but-wrong if a stray
   `libc::close` on a consumed number). Mitigation: the conversion makes most of
   these compile errors (the raw variables cease to exist); reviewer checklist =
   the close-deletion column of the §4 table + the whole-branch review (the
   recurring "missed-sibling-site" catcher).
2. **Earlier parent-side close timing** (§3) — analyzed as strictly safe (the
   child holds fork copies), but it is the kind of change only the full sweep +
   pty tests fully vouch for.
3. **Merged-stderr now dups instead of aliasing** (`try_clone_resolving`) — one
   extra fd per merged stage; fd 2 in the child is a separate descriptor of the
   same file description rather than a shared fd number. Observationally
   identical (both routes dup2 onto the same description). Also silently fixes
   the §5 double-`OwnedFd`.
4. **`fcntl(F_DUPFD, 3)` failure in the fork child (EMFILE)** — degraded to
   today's (freed-fd-corner-only-buggy) behavior rather than propagating an
   error from a post-fork child. Deliberate; a child-side error channel is out
   of scope.
5. **#129/#128 interplay** — F2/F3 rework `async_default_stdin`'s return type;
   the v289 `fd_torture` #129 cases guard that the rule's OBSERVABLE behavior is
   unchanged.

**Docs on the branch:** `docs/architecture.md` module table gains a
`child_fd.rs` row and the executor row's "Pipeline fork/exec" sentence gains a
pointer. No crate public-API surface change (`pub(crate)` module; the spawners
were already crate-internal), so no crate-API doc impact.
