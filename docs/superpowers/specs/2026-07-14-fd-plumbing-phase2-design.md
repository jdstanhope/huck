# v291 — fd-plumbing remediation, Phase 2: universal fd hygiene at creation

**Issues:**
- Primary — [#135](https://github.com/jdstanhope/huck/issues/135) (`divergence`
  + `bug`): an in-process whole-command redirect whose opened file lands on a
  freed 0/1/2 vanishes on the child's exec (`exec <&-; { /bin/cat; } < inA`).
  **Closed by this PR** — T2's ≥10 relocation of the opened source fd fixes it
  (all four flavors verified empirically; see the Problem section).
- [#137](https://github.com/jdstanhope/huck/issues/137) (`divergence` + `bug`):
  a builtin write to a closed fd 1 is swallowed (bash prints `write error: Bad
  file descriptor`, rc 1). **Filed OPEN; NON-GOAL here** — it is a
  builtin-diagnostics gap, not fd wiring. Referenced in Non-goals.

**Parent context:** Phase 2 of the 6-phase (0–5)
fd/redirect/process-launch remediation in
`docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md` (§4
"Phase 2", with §3 H3 "three high-fd helpers" and H4 "CLOEXEC inconsistency"
and §H2d "latent hazards"). Phase 0 shipped as v289 (#128/#129 + the
`fd_torture` net); Phase 1 as v290 (merge `a191f33`: the `child_fd` module,
`ChildFd`/`ChildStdio`, both spawners converted, #132 closed). This phase is
scoped, per maintainer decision: **one v291 iteration, THREE tasks in dependency
order** —

- **T1** — fold the three high-fd helpers (`move_fd_above_stdio`,
  `relocate_high_cloexec`, `alloc_high_fd`) into ONE parameterized pair
  (`dup_to_high_fd` / `move_to_high_fd(src, min, cloexec)`), keeping each
  caller's exact threshold + CLOEXEC choice.
- **T2** — ONE `open_redirect_file(mode, path, noclobber) -> io::Result<OwnedFd>`
  (relocated ≥ 10 + CLOEXEC) replacing the ~11 File-open copies **including the
  in-process `RedirectScope::apply`/`apply_var` sites** (uniform, maintainer's
  choice). **This is the #135 fix.**
- **T3** — unify the two production `make_pipe` implementations behind one
  `make_pipe(cloexec) -> io::Result<(RawFd, RawFd)>` (≥3 relocated) and route
  the three raw `libc::pipe` sites (`procsub`, `spawn_heredoc_writer`,
  `stdin_pipe`) through it, preserving each site's CLOEXEC policy.

**Behavior-preserving EXCEPT where it closes #135** (and the §H2d latents, none
of which reproduce with simple CLI triggers today — so this is otherwise a
hygiene/hardening phase). Line numbers below are approximate on `main` @
`a191f33` (post-v290) and will drift; **function names are the stable handles.**

---

## Problem

The review's H3 + H4: the engine has **no single "move an internally created fd
above the stdio range with an explicit CLOEXEC decision" discipline**, and the
redirect File-open matrix is duplicated. Concretely, on the current tree:

### The three high-fd helpers (`crates/huck-engine/src/executor.rs`)

| Helper | Def | Shape | Threshold | CLOEXEC | Closes src? | Callers |
|---|---|---|---|---|---|---|
| `move_fd_above_stdio(fd) -> io::Result<RawFd>` | :6720 | conditional move (no-op if `fd > 2`) | ≥ 3 | no (`F_DUPFD`) | yes | `make_pipe` only (:6743, :6753) |
| `relocate_high_cloexec(fd) -> RawFd` | :5670 | unconditional move, best-effort | ≥ 10 | yes (`F_DUPFD_CLOEXEC`) | yes | :5971 (plan numeric File), :6007/:6036 (plan heredoc/herestring rfd), :6153 (extra-ops File) |
| `alloc_high_fd(src) -> io::Result<RawFd>` | :5686 | unconditional **dup** (src kept) | ≥ 10 | no | no | :1364 (`apply_var`), :5860 (plan `{var}`), :6644/:6670 (`run_coproc`) |

Two thresholds (3, 10), three CLOEXEC policies, applied inconsistently.
**Review-row correction:** the review calls `alloc_high_fd` "deliberately
non-CLOEXEC," which is true of the helper — but `run_coproc` immediately does
`close(src)` + `set_cloexec(hi)` after each call (:6644–6651, :6670–6677), so
coproc's *net* state is "move ≥10 WITH CLOEXEC," a hand-rolled `move_to_high_fd`.
Only the `{var}` sites want the true non-CLOEXEC dup.

### The pipe-creation sites

**Two production `make_pipe` implementations** (the review said three — refined):

| Site | ≥3 move | CLOEXEC | Notes |
|---|---|---|---|
| `executor.rs::make_pipe` :6737 | yes (#130) | no | shared; 12 call sites (subshell capture, bg/fg pipeline, coproc, `make_orphan_pipe_for_eof_reader`) |
| `stdin_pipe.rs::make_pipe` :127 | **no** (§H2d fd-0 hazard) | yes (`pipe2(O_CLOEXEC)` / fcntl fallback) | embedder stdin feed |
| `wait_loop.rs::make_pipe` :315 | no | yes | **inside `#[cfg(test)] mod tests`** — unit-test scaffolding, NOT a production site; left alone |

Plus **two raw `libc::pipe` sites** with no ≥3 move and no CLOEXEC:
`procsub.rs::realize_via_devfd` :55 and `spawn_heredoc_writer` :4301.

### The redirect File-open matrix — 11 open regions / 16 open expressions

Full 4-arm matrix (ReadOnly `File::open` / Truncate honoring `noclobber` /
Clobber / Append / ReadWrite `<>` no-truncate), five verbatim copies:

| # | Site | Context | Relocates today? |
|---|---|---|---|
| M1 | `RedirectScope::apply` :1038 | in-process | **no** → the `new_fd == target` arm :1087 → **#135** |
| M2 | `RedirectScope::apply_var` :1255 | in-process `{var}` | no (then `alloc_high_fd`) |
| M3 | `build_child_redir_plan` `{var}` :5758 | child-plan | no (then `alloc_high_fd`) |
| M4 | `build_child_redir_plan` numeric :5923 | child-plan | yes (`relocate_high_cloexec`) |
| M5 | `build_child_extra_ops` :6107 | child-plan (pipeline extras) | yes |

Six slot-path variants (`RedirectSlot`, no ReadWrite arm) wrapping to
`ChildFd::from(File)`: bg-pipeline stdin/stdout/stderr (:3393, :3562+:3591,
:3632+:3661) and fg-pipeline stdin/stdout/stderr (:7045, :7161+:7178,
:7207+:7224). Shared leaves `open_resolved`/`open_writable`/`ResolvedRedirect`/
`resolved_path` (:4363–:4401) are used ONLY by these copies.

### #135 — confirmed root cause

`RedirectScope::apply`'s File arm opens the file (Rust std ⇒ `O_CLOEXEC`) and
does NOT relocate it. After `exec <&-` frees fd 0, the open lands on fd 0, which
equals the redirect target, so the `new_fd == target` arm (:1087–1094) records a
`(target, -1)` "was-closed" restore and — critically — **skips the `dup2` that
would have cleared `FD_CLOEXEC`**. The redirect looks applied in the shell but
the descriptor vanishes on the exec of any external child (`/bin/cat` → `Bad
file descriptor`). The child-plan paths (M4/M5) avoid this because they relocate
≥10 and `replay_redir_ops`' `source == target` arm (:5609–5624) clears CLOEXEC;
the in-process path has neither defense. A parallel instance exists for the
heredoc/here-string read end (`apply` :1148/:1176): the raw `spawn_heredoc_writer`
rfd can land on a freed target, and `close(rfd)` after the no-op dup2 closes the
just-installed target.

**Empirically verified fix** (scratch binary = v290 + only the T2-shaped
`relocate_high_cloexec` added after the open in `apply`, plus the same for the
heredoc rfd; reverted after — tree pristine):

| Case | bash | huck v290 | huck + T2 relocation |
|---|---|---|---|
| `exec <&-; { /bin/cat; } < inA; echo end` | `FA` + `end` | `Bad file descriptor` ×2 | **matches** |
| `exec <&-; ( /bin/cat ) < inA; echo end` | `FA` + `end` | `Bad file descriptor` ×2 | **matches** |
| `exec >&-; { /bin/echo hi; } > out` → `out` | `hi` | inner `write error` | **`out` = `hi`, matches** |
| `exec 3<&-; { /bin/cat <&3; } 3<<EOF…` | `hh` | `/bin/cat: Bad file descriptor` | **matches** |

All four flavors (including a previously-unreported heredoc flavor) match bash
byte-for-byte with the relocation. **The PR closes #135.**

---

## Design

**Placement:** T1's helpers and T3's `make_pipe` go in
`crates/huck-engine/src/child_fd.rs` (Phase 1's fd module — its stated remit is
"fd construction/duplication helpers," and `stdin_pipe.rs`/`procsub.rs` are
cross-module callers, which rules out executor-private fns). T2's
`open_redirect_file` stays in `executor.rs` (needs `FileMode`, the `noclobber`
semantics, and `open_writable`). `set_cloexec` (:5655) moves to `child_fd.rs`
alongside the helpers (used by T1's best-effort fallback and T3's macOS path).

### T1 — `dup_to_high_fd` / `move_to_high_fd`

```rust
// child_fd.rs (pub(crate)):

/// Dup `src` to the lowest free fd >= `min` (`F_DUPFD` / `F_DUPFD_CLOEXEC` per
/// `cloexec`). `src` is left open (caller-owned). Errors: EMFILE/EBADF.
pub(crate) fn dup_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd>;

/// `dup_to_high_fd` + `close(src)`. On Err, `src` is left open (caller cleans
/// up). Unconditional: relocates even when `src` is already >= `min` (matches
/// today's `relocate_high_cloexec`; the hot-path `fd > 2` no-op conditional
/// lives only in `make_pipe`). huck's analogue of bash's `move_to_high_fd`.
pub(crate) fn move_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd>;
```

**Why the dup-vs-move split** (not a single `move_to_high_fd(fd, min, cloexec)`):
the `{var}` sites (`apply_var`, plan `{var}`) must keep the source fd open — the
`owns_src` flag decides the source close *separately* from the relocation. A
move-only helper would force those callers to re-dup or to special-case, whereas
a dup core + a `dup + close` wrapper expresses both shapes with no per-caller
bookkeeping.

**bash note (documented in the doc comment):** bash's `move_to_high_fd` scans
DOWN from `getdtablesize()` for a free slot; we keep an explicit `min` +
kernel-assigned lowest-free-≥min via `F_DUPFD` (one syscall, no scan,
deterministic). The two thresholds in use (3, 10) are load-bearing and frozen
(maintainer: "do not change thresholds").

**Caller → (min, cloexec) mapping (behavior-preserving):**

| Old helper / site | New | (min, cloexec) | Note |
|---|---|---|---|
| `move_fd_above_stdio` → `make_pipe` (:6743, :6753) | inline `if fd > 2 { fd } else { move_to_high_fd(fd, 3, cloexec)? }` inside T3's `make_pipe` | (3, per-flag) | the `fd > 2` no-op preserved; old fn deleted |
| `relocate_high_cloexec` (:5971, :6007, :6036, :6153) | KEEP as a 5-line adapter: `move_to_high_fd(fd, 10, true).unwrap_or_else(\|_\| { set_cloexec(fd); fd })` | (10, true) | the best-effort-on-EMFILE policy stays in one place; M4/M5 File uses fold into T2, the heredoc-rfd uses remain (joined by T2's new in-process rfd relocations) |
| `alloc_high_fd` `{var}` (:1364, :5860) | `dup_to_high_fd(src, 10, false)` | (10, false) | source kept (owns_src close is separate) |
| `alloc_high_fd` coproc (:6644, :6670) | `move_to_high_fd(end, 10, true)?` | (10, true) | absorbs the manual `close(src)` + `set_cloexec(hi)`; net fd state identical; the error arms keep their sibling-end closes (valid — Err leaves src open) |

Every site keeps its exact threshold and CLOEXEC outcome; the only mechanical
changes are coproc's three syscalls collapsing to two with the same end state
and Err-cleanup paths that were already src-left-open. `alloc_high_fd`,
`move_fd_above_stdio`, and (after T2) `relocate_high_cloexec`'s File callers are
removed; `relocate_high_cloexec` itself survives as the named best-effort
adapter for the heredoc-rfd sites.

### T2 — `open_redirect_file`

```rust
// executor.rs:

/// THE redirect file-open matrix: open `path` per `mode` (ReadOnly / Truncate
/// honoring `noclobber` / Clobber / Append / ReadWrite-no-truncate), then
/// relocate the fd >= 10 with FD_CLOEXEC (best-effort on EMFILE, like
/// `relocate_high_cloexec`) so a parent-opened redirect *source* can never land
/// in the 0..9 range that redirect *targets* operate on (#135, #132-class).
/// Callers report failures via `redir_open_error(path, ..)` as today.
fn open_redirect_file(mode: &FileMode, path: &str, noclobber: bool) -> io::Result<OwnedFd>;
```

Body: the M1 match (ReadOnly → `File::open`; Truncate → `open_writable(path,
noclobber)`; Clobber → `open_writable(path, false)`; Append → `OpenOptions`
create+append; ReadWrite → read+write+create+truncate(false)), then relocate
≥10 CLOEXEC, then `OwnedFd::from_raw_fd`. The
`ResolvedRedirect`/`open_resolved`/`resolved_path` indirection dies with the
copies (its `resolved_path` is always the input path, so error messages are
unchanged); `open_writable` survives as the internal noclobber leaf.

Slot-mode mapping (S1–S6): `RedirectSlot::Read → FileMode::ReadOnly`,
`Truncate → Truncate`, `Clobber → Clobber`, `Append → Append`;
`noclobber = shell.shell_options.noclobber` (the matrix internally guards only
Truncate — exactly the slot sites' current `noclobber && !Clobber`).

**Per-site adaptation:**

| Site | Context | Under T2 |
|---|---|---|
| M1 `apply` File | in-process | `open_redirect_file(...)?` → fd ≥ 10 + CLOEXEC; `redirect()` dup2 (clears CLOEXEC on target) + drop. **Keep a generalized `new_fd == target` arm** (now reachable only for targets ≥ 10, e.g. `12< f` with 10/11 busy): record `(target, -1)` AND clear `FD_CLOEXEC` in place, mirroring `replay_redir_ops`' `source == target` arm — today's arm misses the clear, which IS #135's mechanism. **This is the #135 fix.** |
| M1 `apply` Heredoc/HereString :1148, :1176 | in-process | relocate the `spawn_heredoc_writer` rfd (`relocate_high_cloexec`) before `redirect()` — the verified heredoc flavor of #135 |
| M2 `apply_var` File | in-process `{var}` | `open_redirect_file(...)?` → `dup_to_high_fd(fd, 10, false)` → drop the OwnedFd (replaces the File-arm `owns_src` manual close; the Dup arm's not-owned source untouched). Heredoc arm: relocate rfd as in M1. |
| M3 plan `{var}` File | child-plan | `open_redirect_file(...)?`; `dup_to_high_fd(.., 10, false)`; drop |
| M4 plan numeric File | child-plan | `open_redirect_file(...)?` → push to `held` directly (relocate is inside) |
| M5 extra-ops File | child-plan | same as M4 |
| S1–S6 slot files | pipeline | `open_redirect_file(...)?` → `ChildFd::from(OwnedFd)` (Phase 1 spawners dup2 Owned fds regardless of number, so relocation here is pure hygiene: it removes the parent-side "slot file on a low fd between open and fork" window and makes every redirect source obey one rule) |

**The H7 software-sink guard (must NOT change):** the capture/merge routing
layer (`redirs_write_stdout`, `final_dests_for_1_2`,
`run_builtin_with_redirects`' borrow dance, `emit_exec_spawn_diag`'s
`stderr_follows_stdout`) derives routing from the REDIRECT LIST, not from opened
fd numbers. T2 changes only *where the temp source fd sits* + the `==target`
CLOEXEC-clear — never the list interpretation, the sink enums, or the
error-emission writers (`redir_open_error` keeps routing through the caller's
redirect-aware writer, v269 T4fix). No decision function is touched.
`error_message_diff_check.sh` + the capture cases in the sweep are the net.

**Behavior deltas (exhaustive, all hazard-closures):** (1) #135 fixed (all four
verified flavors). (2) In-process temp source fds now live ≥ 10 + CLOEXEC for
their instant of existence (previously lowest-free, CLOEXEC) — invisible except
through the freed-fd hazards. (3) The `new_fd == target` arm gains the CLOEXEC
clear (same bug at targets ≥ 10 that #135 exhibits at 0/1/2). Everything else
byte-identical.

### T3 — unified `make_pipe(cloexec)`

```rust
// child_fd.rs (pub(crate)):

/// THE pipe-creation helper. Both ends are guaranteed >= 3 so a freed std fd
/// (e.g. after `exec <&-`) can never be silently reused as a pipe end and
/// aliased onto a child's stdio (#130, and the procsub/heredoc/stdin_pipe
/// latents in review §H2d). `cloexec` chooses the ends' close-on-exec state:
/// false = inherited across exec (pipeline wiring, procsub /dev/fd/N, heredoc
/// feed); true = shell/embedder-internal (stdin_pipe).
pub(crate) fn make_pipe(cloexec: bool) -> io::Result<(RawFd, RawFd)>;
```

Implementation: `cloexec=true` → `pipe2(O_CLOEXEC)` on Linux, `pipe` +
`fcntl(F_SETFD)` fallback elsewhere (lifted verbatim from
`stdin_pipe.rs::make_pipe`, which documents the macOS race as negligible);
`cloexec=false` → plain `pipe`. Then per end:
`if fd <= 2 { move_to_high_fd(fd, 3, cloexec)? }` — T1's helper with the
**matching dup flavor** (`F_DUPFD_CLOEXEC` when `cloexec`, so a relocated
stdin_pipe end keeps CLOEXEC; today's `move_fd_above_stdio` is non-CLOEXEC-only
and would silently strip it). Error cleanup closes both original ends (today's
`make_pipe` shape). Return type stays `(RawFd, RawFd)` this phase — the callers
traffic in raws feeding `ChildFd::owned_raw`/`parent_held`; an `OwnedFd` pair is
the Phase 3/4 migration.

**Per-site routing (each site's CURRENT policy preserved):**

| Site | Change | Policy kept | Hazard closed |
|---|---|---|---|
| `executor.rs::make_pipe` (12 call sites) | `make_pipe()` → `make_pipe(false)`; old def + `move_fd_above_stdio` deleted | non-CLOEXEC, ≥3 | none new (already #130-fixed) |
| `procsub.rs` :55 | raw `libc::pipe` → `make_pipe(false)` | non-CLOEXEC (REQUIRED: the parent-kept end must survive the consuming command's exec for `/dev/fd/N` to resolve) | a freed std fd can no longer become `/dev/fd/0` / alias the inner child's stdio (§H2d) |
| `spawn_heredoc_writer` :4301 | raw `libc::pipe` → `make_pipe(false)` | non-CLOEXEC (read end is dup2'd by consumers) | the heredoc pipe end can no longer land on a freed 0/1/2 (pairs with T2's in-process rfd relocation) |
| `stdin_pipe.rs` :127 | delete private impl → `make_pipe(true)` | CLOEXEC | the §H2d fd-0 hazard: with fd 0 closed at entry, `r` can no longer BE 0, so `dup2(r, 0)` is real (and clears CLOEXEC on the installed fd 0) and `close(r)` no longer destroys the just-installed fd |
| `wait_loop.rs` :315 | **leave alone** — `#[cfg(test)]` test scaffolding, not production (refinement vs the review's "three impls"); folding it couples unit-test code to production relocation for zero coverage gain | n/a | n/a |

**stdin_pipe residual (documented, NOT closed here):** even with relocated ends,
`with_stdin_fd0` on a closed fd 0 hits `saved = dup(0)` → EBADF → the existing
best-effort bail (diagnostic + run without the feed). That is a sane,
non-corrupting degradation (vs today's silent read-from-closed-stdin) and
embedder-only; upgrading the bail is a 3-line follow-on left out to keep T3
purely a creation-site change. Noted in the module doc.

`make_orphan_pipe_for_eof_reader` (:6773, `#[allow(dead_code)]`) follows
mechanically (`make_pipe(false)`); leave its dead-code status to Phase 3 unless
the `allow` can simply be dropped (cleanup has unmasked dead_code warnings
before — the implementer should check).

---

## Testing

All from the repo root; NEVER `--workspace` (OOM-kills this 1-core/1.9 GB box).

1. **Unit tests** — new, in `child_fd/tests.rs` (fresh fds ONLY; never touch
   process-global 0/1/2 in the lib binary, per the fd-isolation rule):
   - `dup_to_high_fd`: result ≥ min; src still open (`fcntl(F_GETFD)` ok);
     CLOEXEC flag matches the param (both values).
   - `move_to_high_fd`: result ≥ min; src closed (`F_GETFD` → EBADF); CLOEXEC
     per param; Err path (already-closed src → EBADF, state sane).
   - `make_pipe(false)` / `make_pipe(true)`: both ends ≥ 3, `FD_CLOEXEC` per the
     flag, write→read round-trip. (The ≤2-relocation branch isn't unit-testable
     without closing std fds — covered by `fd_torture`.)
   - Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
     huck-syntax is untouched.
2. **Binaries:** `cargo build -p huck` (debug) AND
   `cargo build --release --locked --bin huck` — the sweep needs both, each
   harness on its intended default binary (the funcnest lesson: never sweep on
   the wrong binary, never override `HUCK_BIN`).
3. **`fd_torture_diff_check.sh` — flip the #135 family green** (update the header:
   the exclusion list shrinks to #50 only):
   - `exec <&-; { /bin/cat; } < inA; echo end`
   - `exec <&-; ( /bin/cat ) < inA; echo end`
   - `exec >&-; { /bin/echo hi; } > f; /bin/cat f >&2` — **route the proof
     through stderr**, avoiding a trailing builtin write to the closed fd 1
     (huck's builtin write-error reporting diverges — that is #137, a separate
     gap, so the case must not depend on it)
   - `exec 3<&-; { /bin/cat <&3; } 3<<EOF` heredoc flavor (verified fixed)
   - Do NOT add the saved-fd `readlink /proc/self/fd/0` probe — it is NOT a
     user-visible divergence (both bash and huck print nothing); it is an
     internal-hardening note for Phase 3, not a test case.
4. **Full sweep:** `ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh`.
5. **`-p huck` integration binaries** (they run locally — the v289 CI lesson;
   `--lib` alone is not enough). Each as
   `ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`.
   Highest-value for this diff: `heredoc`, `here_string`,
   `heredoc_forked_writer`, `process_sub`, `coproc`, `fd_dup`, `named_fd`,
   `external_fd_redirects`, `compound_redirects`, `function_redirect`,
   `builtin_fd_ordering`, `builtin_stdout_dup`, `builtin_pipe_flush`,
   `noclobber`, `io_error`, `bg_sequence`, `subshell`, `subshell_pipeline`,
   `pipeline_subshell`, `pipefail`, `sigpipe`, `exit_inherits`,
   `cmdsub_subshell`; pty: `subshell_pipeline_pty`, `procsub_stop_pty`,
   `jobcontrol_pgroup_pty`. Sweep the full `tests/*.rs` set before the PR.
6. **Manual spot-checks** (not byte-diff automatable): the four #135 flavors vs
   bash; `diff <(echo a) <(echo a)`; `exec <&-; cat <(echo hi)` (procsub on a
   freed fd — matched bash pre-phase, must still match); coproc round-trip
   (`coproc X { cat; }; echo hi >&${X[1]}; read -u ${X[0]} v; echo $v`); the
   embedder stdin feed (via the stdin_pipe lib tests in step 1).

---

## Non-goals

- **Phase 3's `lower_redirects` / `RedirectScope` consolidation** — T2 unifies
  only the file-OPEN leaf; the five lowering orchestrations (M1–M5 minus their
  open arms), the `RedirectSlot` fast-path (#50), the diagnostics prologue (#69,
  #78-message), and the `extra` field on `ChildStdio` all stay put.
- **The saved-fd relocation** (`RedirectScope::redirect`/`close_target`'s
  `dup(target)` saves land lowest-free + non-CLOEXEC) — **NOT a filed bug**: it
  is not a user-visible divergence (`exec <&-; { readlink /proc/self/fd/0; }`
  matches bash — both empty; restore-correctness self-heals via reverse-order
  restore). Recorded here only as a **Phase-3 verification note**: when the
  save/restore path is consolidated into `lower_redirects`, confirm saved fds
  are relocated high + CLOEXEC too. No issue.
- **[#137](https://github.com/jdstanhope/huck/issues/137)** (builtin write to a
  closed fd 1 swallowed, no `write error` / rc 1) — a builtin-diagnostics gap,
  distinct from fd wiring; filed OPEN, not this phase. (The `fd_torture` #135
  case is deliberately crafted to not depend on it.)
- **`OwnedFd`-returning `make_pipe` / migrating `parent_held` to owned types** —
  Phase 3/4.
- **procsub pgroup (#97/#45), the stdin_pipe saved-EBADF bail upgrade** — noted,
  out of scope.
- **The `wait_loop.rs` test-module `make_pipe`** — left untouched (not production).

---

## Risks

1. **The in-process redirect path (M1/M2) is the most-diff-tested,
   most-H7-entangled surface in the repo.** T2 changes only "where the temp fd
   sits" + the `==target` CLOEXEC-clear, but this is where a v286-style
   missed-sibling regression would hide. Mitigation: the arm keeps its shape
   (+ the clear), `error_message_diff_check.sh` + the full sweep + the
   whole-branch review (the recurring missed-sibling catcher) are the net.
2. **H7 software-sink layer untouched by design** — no sink enum, routing
   heuristic, or emission writer changes; the capture matrix guards it.
3. **The dup-vs-move correctness** — `dup_to_high_fd` MUST leave the source open
   (the `{var}` `owns_src` callers), `move_to_high_fd` MUST close it (coproc,
   relocation); the unit tests assert both, and the per-flag CLOEXEC in
   `make_pipe` is load-bearing (a CLOEXEC end relocated with plain `F_DUPFD`
   would silently lose CLOEXEC — asserted explicitly).
4. **EMFILE degradation:** `open_redirect_file` inherits the best-effort fallback
   (CLOEXEC'd original, unrelocated) — identical to today's
   `relocate_high_cloexec` policy; under fd exhaustion the #135 corner can
   reappear, as bash does under the same pressure. Documented, accepted.
5. **coproc's alloc→move switch** (3 syscalls → 2, same net state) — the `coproc`
   integration binary + the manual round-trip cover it.
6. **procsub `/dev/fd/N` numbering** shifts only in freed-low-fd corners (opaque
   to scripts; bash's numbers differ anyway) — `process_sub` + `procsub_stop_pty`
   + the manual check cover it.

**Docs on the branch:** `docs/architecture.md` :45 (the stdin_pipe row) gains
"shared `make_pipe` (≥3-relocated)" wording; no other live doc names the
removed/renamed helpers (swept). No crate public-API surface change (all
`pub(crate)`), so no crate-API doc impact.
