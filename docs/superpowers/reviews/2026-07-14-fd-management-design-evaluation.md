# fd-management design evaluation — differential audit + recommendation

**Date:** 2026-07-14. **Context:** paused v292 (Phase 3a redirect-lowering
consolidation) after its whole-branch review found `{var}` regressions. The
maintainer asked: run a systematic differential audit of huck's redirect/fd
behavior vs bash 5.2, then evaluate the pre-OwnedFd design against the new
(OwnedFd + batch-lowering) design and determine how huck should manage fds to
match bash.

## Method

A differential harness (`tools/redirect_audit.sh` + `redirect_audit_cases.sh`)
runs each redirect construct through **bash 5.2.21** and huck identically in a
fresh temp dir and compares ALL observables: combined stdout+stderr stream,
exit code, resulting file contents, and `$v` (the `{var}` fd number). 157 cases:
single redirect atoms, ordered interaction pairs (both orders), input/heredoc
redirects, and constructs **inspired by** the bash test suite's `redir.tests` /
`redir*.sub` (the canonical `a=2 echo foo 2>&1 >file` ordering, `exec` dup/move/
close lifecycles, nested-subshell dups, `read <&N`, builtin-writes-to-closed-fd),
each exercised in three contexts: in-process compound, `exec` (persistent), and
external command.

The same 157 cases were run against the **main baseline** (`31801f9`, the
pre-implementation OLD design: `apply`/`apply_var` per-redirect + `build_child_redir_plan`
batch, raw fd numbers) and the **v292 branch** (`a868619`, OwnedFd + one batch
`lower_redirects` + two appliers), and the divergence sets were diffed.

## The empirical divergence map

| | count |
|---|---|
| Divergences on **main** (old design) | **24** |
| Divergences on **v292 branch** (new design) | **22** |
| v292 **INTRODUCED** (regressions) | **6** |
| v292 **FIXED** | **8** |
| **Persistent** (diverge on both; fd-management-independent) | **16** |

### v292 INTRODUCED — 6, all in-process/`exec` `{var}` (regressions)
- `{v}>f 2>&$v` — a later redirect can't see `$v` assigned by an earlier `{var}` (bash: `E`→`f`, `rc=0`; huck: "bad fd", `rc=1`).
- `{v}>f 2>&9` — the `{var}` must PERSIST (`$v=10`) even when a later redirect fails (huck drops it: `v=unset`).
- `3>a {v}>x` — `{var}` fd-number allocation shifts 10→11 (a held file temp occupies fd 10 during batch lowering).

**Root cause:** batch-lower-then-apply. `{var}` carries *ordered side effects* — it assigns `$v` (visible to later redirects), allocates a persistent fd whose number depends on earlier temps, and its assignment must survive a later redirect's failure. Lowering the whole list before applying erases that ordering. The old `apply` interleaved resolve+apply per redirect, so it was correct.

### v292 FIXED — 8, all EXTERNAL path
- `2>&9`, `>&9`, `2>&9 >f`, `>f 2>&9`, `<&9` — invalid-dup detection/truncation on external commands.
- `1>&- 2>&1`, `4>&3 3>f` — external dup ordering.
- external `{v}>f 2>&9` — external `{var}`-then-fail.

**Root cause:** the child/external path genuinely benefits from batch lowering + the T4 lower-time `fd_state` validation. It can't touch real fds before `fork`, so an ordered plan + an fd-table *simulation* is the correct model, and it made the external path substantially MORE bash-faithful. The Option-B decision (apply the lower-time validation to both paths) is validated here.

### Persistent — 16, on BOTH designs (independent of fd-management approach)
- **#137 — write to a closed fd is not detected (8):** `>&-`, `2>&1-` (move closes fd1), `{v}>&1-`, `echo/printf >&-`. bash prints `cmd: write error: Bad file descriptor` and returns `rc=1`; huck writes nothing and returns 0. This is the **software-sink write path**, not redirect lowering.
- **`{var}` error-message wording + external `$v`-visibility (8):** `{v}>&9` (bash "redirection error: cannot duplicate fd" vs huck "9: Bad file descriptor"), `2>&$v {v}>f` (bash "ambiguous redirect" vs huck "bad fd"), `{v}>&-` then use (bash echoes `$v`, huck the resolved number), external `{v}>f 2>&$v`.

## Design evaluation

**1. OwnedFd (P1/P2) is exonerated — keep it.** It caused ZERO divergences in either direction. It eliminated the raw-fd-number sentinels, fd leaks, and double-closes (the sibling-bug generator the remediation was built to kill). The ownership/lifetime layer is sound and orthogonal to every divergence found. This was the right call.

**2. Batch lowering (Approach C) is right for the child, wrong for in-process.** The audit is unambiguous: batch lowering FIXED 8 external-path divergences and INTRODUCED 6 in-process ones. The two paths are not the same problem:
- The **child/external** path cannot apply to real fds before `fork`, so it must build an ordered plan and simulate the fd table (`fd_state`) to validate/order. Batch is correct and better there.
- The **in-process** path applies to the shell's own live fds. Redirects there have ordered side effects — `{var}`'s `$v` assignment (observed by later redirects), fd-number allocation (dependent on earlier temps), persistence across a later failure, and the live fd table (which later dup validation reads). Only per-redirect **interleaving** reproduces bash. Batch cannot.

**3. The old per-redirect `apply` was semantically correct for in-process** (it interleaved) but suffered the *duplication* problem — the same resolution logic copy-pasted across `apply`, `apply_var`, and `build_child_redir_plan`, so every fix spawned siblings. That duplication, not the interleaving, was the real defect.

## Recommendation: how huck should manage fds

Three orthogonal concerns, each with a clear answer from the data:

1. **Ownership/lifetime → `OwnedFd` RAII everywhere.** Keep the P1/P2 gains. Meaning and ownership live in the type, not in raw `i32`s. (No divergence traces to this.)

2. **In-process application → per-redirect INTERLEAVED** (resolve one → apply to the real fds → resolve next). The real fd table is the source of truth; validate each dup against it at apply time; `{var}` assigns `$v` and persists its fd as it is applied, before the next redirect resolves. This is bash's model and the old `apply`'s model. Fixes the 6 regressions.

3. **Child/external application → BATCH into an ordered plan, replayed post-`fork`,** with an explicit `fd_state` simulation for dup validation and ordering (the T4 mechanism). The child genuinely can't interleave against real fds, and bash's child `{var}` semantics differ (no `$v` persistence to the parent), so it does not need the in-process side-effect model. Keeps the 8 fixes.

**The correct unification is Approach B, not C:** share the per-redirect
*resolution* — `lower_one_redirect` (open file → `OwnedFd`, spawn heredoc writer,
resolve dup word → fd number, allocate `{var}` high fd) — and let the two
appliers differ:
- in-process: loop `[resolve → apply]` (interleaved),
- child: loop `[resolve → collect]` then replay the batch post-fork.

This keeps the DRY win (the copy-pasted resolution unified in one place, the
actual defect) while respecting that the two *application* models are genuinely
different — which is exactly what Approach C got wrong. During brainstorming,
Approach B was rejected as "a half-consolidation"; the audit shows it is the
correct decomposition and C's single-applier unification was the error.

**Orthogonal backlog (do NOT fold into the fd rework):**
- **#137 (write-to-closed-fd not detected)** — 8 divergences, all in the
  software-sink write path (`StdoutSink`/builtin write). Bash checks each write
  for EBADF and reports `write error: Bad file descriptor` + `rc=1`. Its own fix.
- **`{var}` error-message wording + external `$v`-visibility (#140)** — ~8
  divergences, message formatting and external `{var}` sibling-dup resolution.

**Audit blind spot found by a manual probe (the corpus could not see it):** the
harness observes the `{var}` fd via `$v`, which external commands do not persist,
so the **external `{var}` fd *number*** was invisible. A direct `/proc/self/fd`
probe shows `cmd 3>a {v}>x` inherits **fd 11** in huck (fd 12 with two preceding
temps) vs bash's **fd 10**. This is *pre-existing* (main's `build_child_redir_plan`
also batches and holds file temps at the high range during `{var}` allocation),
so it is **not a v292 regression** — but it means the child path cannot match
bash's in-child interleaved `{var}` numbering by simply allocating the lowest real
free fd. The correct child fix is a **virtual `{var}` allocator**: pick the lowest
number ≥10 not used as a *target* by an earlier plan op and not by an earlier
`{var}`, and emit a `dup2(source → that number)` op — the held file temps occupy
the high range only in the parent, but in the child they are dup2'd to their
targets and closed, so the virtual number is free at replay time. Lesson for the
harness: add an external `/proc/self/fd` observable so `{var}` numbering is
covered automatically. (Deferred with #140's family — pre-existing, not blocking
v292.)

## Implications

- **v292:** rework to Approach B — in-process interleaves (fixes the 6
  regressions), child keeps the batch plan + `fd_state` (keeps the 8 fixes).
  `OwnedFd` stays. The T3/T4 resolution code and tests are largely reusable as
  `lower_one_redirect`. The remaining Phase-3b/4/5 work (retire `RedirectSlot`,
  merge the pipeline functions, procsub pgroup) is unaffected by this correction.
- **Test infra:** `tools/redirect_audit.sh` is now a permanent, reusable
  differential net. It converts the redirect subsystem from hand-picked cases
  (which hid the truncation bug for months) to a systematic matrix. Run it in CI
  or before any fd-touching change; extend the corpus as new constructs surface.
- **Strategic:** the "we keep producing bugs" concern was correct AND now bounded.
  The audit proves the *entire* structural regression surface of the new design is
  a single, well-understood class (`{var}` interleaving), fixable by one design
  correction — not an open-ended stream. The remaining divergences are pre-existing
  and orthogonal, with a known finite list.
