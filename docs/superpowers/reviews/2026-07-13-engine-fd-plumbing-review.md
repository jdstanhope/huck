# huck engine fd/redirect/process-launch plumbing — architectural review

Read-only review of `crates/huck-engine` at commit 35eed38 (`main`), 2026-07-13.
All line numbers refer to `/home/john/projects/huck/crates/huck-engine/src/executor.rs`
unless another file is named. No code was modified.

---

## 1. Verdict

**Yes — there is a systematic root cause, and it is confirmable from the code.**
The engine has **no single owned representation of "the fd environment a child
will start with."** Instead, each of ~10 process-launch paths re-derives child
stdio by hand from raw `RawFd` integers whose *meaning* ("explicitly
redirected file" vs "inherit the shell's std stream" vs "pipe end I must
close") is encoded in the **fd number itself** (0/1/2 vs >2), combined with
**three parallel redirect representations** (the ordered `Redirection` list,
the lossy `RedirectSlot` fast-path, the lowered `ChildRedirOp` replay) and
**two child-side application mechanisms** with opposite ownership semantics.
Because meaning and ownership are not carried in a type, every path needs
hand-written bookkeeping (46 `fd > 2` guard sites, two bail helpers extracted
from "~19" and "~21 byte-identical" error sites), and every fix must be
manually replicated at each sibling site — which is exactly the observed
whack-a-mole (#121 needed 5 sites; #126's fix landed in 1 of 2 background
paths; #130's fix landed in 1 of 4 pipe-creation sites).

One caveat up front: **#128 is NOT part of this class** (it is an exit-time
SIGHUP policy bug — see §3.6) and #120 was a wait-loop bug, also outside the
class. The rest of the recent and older bugs map cleanly onto four structural
defects (§3).

---

## 2. The map: process-launch / fd-wiring code paths

### 2.1 Launch paths (who forks/spawns, and how fds are wired)

| # | Path | Entry | Redirect mechanism | Child fd application |
|---|------|-------|--------------------|----------------------|
| 1 | Single builtin / function / eval / source | `run_exec_single_inner` :5281–5347 → `run_builtin_with_redirects` :1662–1884, `with_redirect_scope` :1553–1660 | Full ordered `Redirection` list via **`RedirectScope::apply`** :990–1184 (parent-side dup2 + save/restore) + in-memory sink routing (`final_dests_for_1_2` :1474, `route_out_to_err`/`route_err_to_out` :1684–1689) | None (in-process) |
| 2 | Single external command | :5354 → **`build_child_redir_plan`** :5775–6136 → **`run_subprocess`** :6300–6593 | Full ordered list lowered to `ChildRedirOp` replay | `std::process::Command` + up to 3 chained `pre_exec`s (signal reset :6321, `replay_redir_ops` :6355, Merged-stderr dup2 :6386) + `Stdio::piped()` for capture |
| 3 | `exec` builtin (permanent redirects / process replacement) | `run_exec_builtin` :5564–5661 → **`apply_redirects_permanently`** :5505–5557 | `RedirectScope` applied then made permanent (drain `saved`, `mem::forget`) | `process.exec()` |
| 4 | **Foreground multi-stage pipeline** | **`run_multi_stage`** :6879–7934 (~1,055 lines) | **`RedirectSlot` fast-path** (`slot_stdin/stdout/stderr`, inline file opens :7112–7385) + `build_child_extra_ops` :6152–6259 for everything the slots drop | Per stage: `spawn_external_with_fds` :8841–9040 (External) or `fork_and_run_in_subshell` :8667–8790 (InProcess) |
| 5 | **Background bare-trailing-`&` pipeline** | **`run_background_sequence`** :3162–4029 (~867 lines) | Near-verbatim clone of path 4's slot fast-path (:3364–3869) | Same two spawners |
| 6 | Background subshell / and-or group | `run_background_subshell` :3102–3160 (dispatch :256–288, :538–567) | `async_default_stdin` :3072–3096 (#126 fix lives ONLY here) | `fork_and_run_in_subshell` |
| 7 | Foreground `( … )` subshell | `run_command` Subshell arm :609–~700 | Own `make_pipe` wiring for capture/merged | `fork_and_run_in_subshell` |
| 8 | `coproc` | `run_coproc` :6607–6786 | Two `make_pipe`s + `alloc_high_fd` + `set_cloexec` by hand | `fork_and_run_in_subshell` |
| 9 | Process substitution | `procsub.rs::realize_via_devfd` :48–109 | **Raw `libc::pipe`** (no fd≥3 move, no CLOEXEC), pgid hardcoded to `shell.shell_pgid` :81 | `fork_and_run_in_subshell` |
| 10 | Heredoc/here-string feeder | `spawn_heredoc_writer` :4373–4435 | **Raw `libc::pipe`** (no fd≥3 move, no CLOEXEC) | raw `fork` + `_exit` |
| 11 | Embedder stdin feed | `stdin_pipe.rs::with_stdin_fd0` :33–125 | **Its own private `make_pipe`** (CLOEXEC, no fd≥3 move) :127–147 | dup2 onto fd 0 in the shell itself |

Paths 4+5 additionally hand-manage `parent_held: Vec<RawFd>`,
`prev_pipe_read`, `fds_to_close_in_child`, and a `went_external` flag that
flips fd-close responsibility (:7662–7793, :3871–3964).

### 2.2 Concrete duplication inventory

**(a) `run_background_sequence` is an ~870-line hand-maintained clone of
`run_multi_stage`.** Its own header admits it ("Spawn each stage using the
same per-stage fork dispatch as run_multi_stage" :3172). Corresponding
near-verbatim blocks:

| Logic | run_background_sequence | run_multi_stage |
|---|---|---|
| Assign-only stage | :3222–3337 | :6965–7085 |
| slot stdin (Read/Heredoc/HereString) | :3364–3478 | :7112–7221 |
| slot stdout open (Truncate/Clobber/Append) | :3484–3573 | :7232–7297 |
| slot stderr open (same, again) | :3576–3685 | :7300–7385 |
| stdout pipe / orphan-pipe / capture | :3687–3776 | :7387–7505 |
| Dup-target pre-resolution | :3798–3869 | :7589–7660 |
| classify + spawn + `went_external` close bookkeeping | :3871–3964 | :7662–7793 |

Differences are three policy items (stage-0 stdin default, job registration
vs. wait, non-blocking procsub drain) — everything else is copy drift waiting
to happen. The two bail helpers (`bail_teardown_bg` :4035, doc: "the ~19
byte-identical bail sites"; `bail_teardown_stage` :4057, "~21") are the
fossil record of this duplication.

**(b) The redirect File-open matrix (ReadOnly / Truncate|Append|Clobber with
noclobber / ReadWrite) appears verbatim FIVE times**, plus four more
slot-path variants:

1. `RedirectScope::apply` :1019–1068
2. `RedirectScope::apply_var` :1236–1282
3. `build_child_redir_plan` `{var}` arm :5831–5877
4. `build_child_redir_plan` numeric arm :5996–6042
5. `build_child_extra_ops` :6180–6226
6.–9. the `open_writable`-based slot blocks in both pipeline functions
   (:3487–3568, :3579–3680, :7235–7291, :7303–7380)

**(c) `RedirOp::Move` (#121, the most recent fix) had to be implemented at
FIVE sites**: `apply` :1093–1118, `apply_var` :1223+1285–1294+1369–1372,
`build_child_redir_plan` (both arms) :5815–5818/5970–5974 + :6053–6071, and
`build_child_extra_ops` :6234–6249. That a single new redirect operator costs
five parallel implementations is the duplication tax in its purest form.

**(d) Two child-side fd application mechanisms with opposite ownership.**
`spawn_external_with_fds` **consumes** stdio fds via `OwnedFd::from_raw_fd`
(:8964–8995); `fork_and_run_in_subshell` **borrows** them (caller closes,
:8706–8719). Callers must branch on `went_external` at every close site
(:7713–7729, :7764–7793, :3912–3964). Get it wrong once → leak or
double-close.

**(e) Three redirect representations.** `Redirection`/`RedirOp` (ordered,
complete) vs `RedirectSlot` (`slots_for_simple_path`,
`huck-syntax/src/command.rs:185–205, 499–507` — last-wins, order-lossy,
0/1/2-only) vs `ChildRedirOp` (:5668–5673). The slot fast-path is used ONLY
by the two pipeline functions, and its lossiness is a *documented* residual
limitation (`build_child_extra_ops` doc :6145–6151: source order not
preserved on pipeline stages = **#50**; fd>2 heredoc on a stage dropped).

### 2.3 What IS shared (credit where due)

`resolve_dup_source` :4228, `validate_fd_open` :4247, `expand_single` :4194,
`spawn_heredoc_writer`, `open_resolved`/`open_writable` :4437–4473,
`replay_redir_ops` :5679, `make_pipe` :6812, the two bail helpers, and
`classify_stage` :8813 are genuinely shared. The sharing is at the *leaf*
level; the *orchestration* above the leaves is what's duplicated.

---

## 3. Root-cause analysis (hypotheses confirmed/refuted)

### H1 — stdio/redirect wiring duplicated across launch paths: **CONFIRMED, and worse than stated**

See §2.2. It is not just "a fix in one misses the others" — the sibling sets
are different per feature:

- #121 Move: 5 redirect-lowering siblings (all found, fixed).
- #126 async stdin: 2 background-path siblings (`run_background_subshell`
  :3115 uses `async_default_stdin`; `run_background_sequence` :3189–3208 +
  :3473 hardcodes `/dev/null` unconditionally — ignoring both
  `shell.is_interactive` and the bare-multi-stage-pipeline inherit rule the
  helper encodes). **That inconsistency IS #129**, confirmed in code.
- #132 CLOEXEC/inherit misroute: 2 spawn-mechanism siblings (§H2), only the
  issue's two named functions are callers; the fork-path sibling (§H2b) is a
  third latent one.

### H2 — raw fd-number sentinels decide behavior: **CONFIRMED**

**(a) `spawn_external_with_fds` :8964–8995** — the #132 mechanism, verbatim:

```rust
let stdin_stdio = if stdin_fd == 0 { Stdio::inherit() } else { … OwnedFd … };
… else if stdout_fd == 1 { Stdio::inherit() } … else if stderr_fd == 2 { Stdio::inherit() }
```

A freshly opened redirect `File` (Rust std ⇒ `O_CLOEXEC`) that happens to
land on a freed 0/1/2 is treated as "already at its natural slot" →
`inherit()` → nobody clears CLOEXEC → the fd **vanishes on exec**. Reachable
from BOTH pipeline functions (they pass `explicit_stdout_fd` /
`explicit_stderr_fd` / slot-opened `stdin_fd` straight in: :7673–7683,
:3875–3885), exactly as #132 states.

**(b) A third, unfiled sibling: `fork_and_run_in_subshell` :8706–8719** uses
the same sentinels (`if stdin_fd != 0 { dup2 }`, `if fd > 2 { close }`). An
InProcess stage whose slot-opened CLOEXEC file landed on freed fd 0/1/2 skips
the dup2 (so CLOEXEC is never cleared by `dup2`) and skips the close; if that
in-process child then execs an external grandchild (via `run_subprocess`),
the std fd vanishes on the grandchild's exec — same class as #132, different
spawner. **Recommend filing this when #132 is fixed.**

**(c) The `> 2` disease is pervasive**: 46 `fd > 2` guard sites in
executor.rs. Each one silently encodes "0/1/2 are the shell's streams, not
mine to close" — false the moment a std fd has been freed by `exec <&-`
(#130's precondition). E.g. `if stdin_fd > 2 { close }` (:7765) *leaks* a
slot-opened file that landed at fd ≤ 2.

**(d) Smaller latent instances of the same class** (not filed, found during
review):
- `stdin_pipe.rs:68–85`: if fd 0 is closed at entry, its private `make_pipe`
  returns `r == 0`; `dup2(r,0)` is a no-op and the subsequent `close(r)`
  closes the pipe read end that IS fd 0 → the body reads from a closed stdin.
- `procsub.rs:53–57`: raw `libc::pipe` with no fd≥3 move → a freed std fd can
  become `/dev/fd/0`, and the parent-kept end can alias the inner child's
  stdio (`child_closes` collides).

### H3 — no global "move internal fds above the stdio range" discipline: **CONFIRMED**

There are **three different "high fd" helpers with two thresholds and three
CLOEXEC policies**, applied inconsistently:

| Helper | Threshold | CLOEXEC | Used by |
|---|---|---|---|
| `move_fd_above_stdio` :6795 | ≥ 3 | no | `make_pipe` :6812 only (#130 fix) |
| `relocate_high_cloexec` :5744 | ≥ 10 | yes | plan builders' file/heredoc sources |
| `alloc_high_fd` :5760 | ≥ 10 | **deliberately no** | `{var}` fds, coproc ends |

NOT covered by any discipline: `procsub.rs` pipes (:53), `spawn_heredoc_writer`
pipes (:4374), `stdin_pipe.rs` pipes (:127), and — crucially — **every
slot-path `File::open`/`open_writable` result in the two pipeline functions**
(:3387, :3509, :3549, :3606, :3656, :7133, :7251, :7279, :7324, :7362),
which flow to the H2 sentinels. bash's `move_to_high_fd` is applied
universally; huck's equivalent is applied at exactly one of ~8 creation
sites.

### H4 — inconsistent CLOEXEC policy: **CONFIRMED**

- Rust `File`/`OpenOptions` opens: CLOEXEC (kernel-level, implicit).
- `make_pipe`/`move_fd_above_stdio`: non-CLOEXEC (deliberate, documented :6791).
- `procsub`/`spawn_heredoc_writer` pipes: non-CLOEXEC (raw `pipe()`, undocumented).
- `stdin_pipe`'s pipe: CLOEXEC.
- `alloc_high_fd`: non-CLOEXEC deliberately; `relocate_high_cloexec`: CLOEXEC deliberately.

The policy is *mostly* reasoned per-site but **implicit and non-local**. The
tell: `replay_redir_ops` :5682–5697 needs a special `source == target` arm
whose sole job is to undo Rust's implicit CLOEXEC when a parent-opened file
happened to land on its own target fd — a patch over the fact that "will this
fd survive exec?" is decided at open time by whichever API was convenient,
then corrected downstream case by case. #132 is precisely the case where the
downstream correction (dup2 clearing CLOEXEC) is skipped by an H2 sentinel.

### Root causes the hypotheses missed

**H5 — the `RedirectSlot` fast-path (lossy second representation).**
`slots_for_simple_path` collapses the ordered redirect list into three
last-wins slots; ordering between a slot op and an extra op is lost (**#50**,
documented at :6145–6151), fd>2 heredocs on stages are dropped (:6252–6255),
and slot handling forces the pipeline functions to re-implement file opening
inline (§2.2b items 6–9). The single-command paths already abandoned the slot
system (v156, `resolve()` comment :4352–4356); the pipeline paths never did.

**H6 — diagnostics are emitted at each duplicated site with whatever context
that site has.** Redirect-open errors in the pipeline slot paths go through
`redir_open_error` with no line number (**#69**), and `command`/`builtin`
wrapper option errors are emitted before/outside the real-fd redirect scope
so a `2>&1` capture misses them (**#77**). Also concretely wrong:
`spawn_external_with_fds` :8867–8869 wraps a resolve failure (whose
diagnostic `resolve()` already printed) as
`io::Error::other("resolve failed with code {code}")`, which the pipeline
caller then prints *again* through `bash_io_error` (:7704–7710) — half of
**#78**'s wrong-message symptom.

**H7 (out of class, but adjacent): the software-sink dimension.**
`StdoutSink`/`StderrSink` capture/merge routing interacts with real-fd
redirects through per-path heuristics (`redirs_write_stdout` :1429,
`final_dests_for_1_2` :1474, `run_builtin_with_redirects`' 8-arm borrow dance
:1724–1869, `emit_exec_spawn_diag`'s `stderr_follows_stdout` re-derivation
from lowered ops :6335–6353). This is a second, orthogonal plumbing surface;
v286's "missed-sibling `redirs_write_stdout`" finding came from here. Any
remediation should *not* try to redesign this at the same time, but must not
break it.

### Bug → root-cause mapping

| Bug | Status | Root cause(s) |
|---|---|---|
| #120 100ms foreground latency | fixed | none of the above — wait-loop tick bug (`stream_loop.rs:37–43` fast path). Genuine one-off. |
| #121 move-fd `<&N-`/`>&N-` | fixed | H1 (5 sibling lowering sites) |
| #126 async stdin /dev/null | fixed (1 of 2 paths) | H1 (background-path duplication) |
| #129 bg-sequence stdin inconsistent | open | H1 — same rule, second copy (:3473 vs :3072) |
| #130 pipe reuses freed fd 0 | fixed (make_pipe only) | H3 (+H2: the alias only mattered because numbers carry meaning) |
| #132 fresh redirect fd on freed 0/1/2 vanishes | open | H2 + H4 (sentinel skips the CLOEXEC-clearing dup2); sibling in `fork_and_run_in_subshell` unfiled |
| #128 bare-`&` child killed at `huck -c` exit | open | **none of the above** — `shell.rs:281` calls `hangup_jobs()` (`shell_state.rs:3046`) on every clean exit incl. non-interactive; bash SIGHUPs jobs only for interactive+`huponexit` (default off) or on receipt of SIGHUP. One-line policy fix + maybe a `huponexit` shopt. |
| #50 stage redirect source-order | open | H5 |
| #78 stage spawn-failure fd leak + wrong message | open | H1/H6 + D: on error return before :8964, `spawn_external_with_fds` has NOT consumed stdin/stdout/stderr fds, but callers assume `went_external ⇒ consumed` (:7712–7713) → leak; message half is H6 (:8867–8869). |
| #77 wrapper option-error leaks under `2>&1` | open | H6 (+H7) |
| #69 stage redirect errors omit `line N:` | open | H6 (duplicated emit sites) |
| #97 macOS Ctrl-Z + procsub hang | open | H1-adjacent: procsub is its own launch path with hardcoded pgroup (`procsub.rs:81`) + non-blocking-drain interplay; partially subsumable |
| #45 procsub edge cases | open | same as #97 |
| #79 piped-stdin line numbers | open | not this class (lineno plumbing) |
| #62 herestr empty command name | open | not this class (expansion/diagnostics) |

---

## 4. Remediation: phased plan

Design goal: **one type that says what a child's fds will be, and one
function that builds it** — then every launch path becomes a thin client.
All phases are individually shippable as normal `vNN` iterations, each
guarded by the existing bash-diff sweep plus a new fd-torture harness.

### Phase 0 — targeted policy fixes (quick wins, not structural)
- **#128**: gate `hangup_jobs()` at `shell.rs:281` on interactive (+
  optionally implement `shopt huponexit`, default off). *S, low risk.*
- **#129**: replace `run_background_sequence`'s inline `/dev/null` open
  (:3189–3208, :3473) with a call to `async_default_stdin` (inherit rule:
  bare multi-stage pipeline / interactive). *S, low risk.* (Subsumed later by
  Phase 4, but cheap now and unblocks #126 testing on that path once #128 is
  fixed.)
- **New harness**: `fd_torture_diff_check.sh` — `exec <&-` / `exec >&-`
  prefixes composed with pipelines, redirects-to-fresh-files, heredocs,
  background. This is the objective regression net for Phases 1–4. *S.*

### Phase 1 — kill the sentinels: explicit `ChildFd` ownership (fixes #132 class)

**Design decision (maintainer, 2026-07-13): encapsulate as a CONCRETE owned type +
a dedicated module, NOT a polymorphic trait.** The problem is that meaning and
ownership live in raw `i32`s, not that we have interchangeable fd-handling
*strategies* — so the fix is to make illegal states unrepresentable with a type,
leaning on `std::os::fd::OwnedFd` for RAII (single-ownership, close-on-drop,
CLOEXEC travels with the fd; leak/double-close become type errors → #78 leak-half).
A trait is the wrong tool here and is only a *maybe-later* option for abstracting
the two spawners (`spawn_external_with_fds` vs `fork_and_run_in_subshell`) — declined
for now (YAGNI: exactly two, different return types/lifecycles, no third impl or test
seam yet). Foreground-vs-background is a `SpawnMode` enum (Phase 4), not a trait.
Carve the whole thing into a focused `child`/`fdplan` module out of the 8k-line
`executor.rs`, with a narrow public surface (`ChildStdio`, `ChildFd`,
`lower_redirects`, the two spawn fns) — the module boundary IS the encapsulation.
Caveat to design carefully: `OwnedFd`+`Drop` across `fork`/`pre_exec` (do NOT run
destructors in the forked child before exec; `pre_exec` is async-signal-constrained)
— the `fd_torture` harness (Phase 0) is the net.

Introduce a tiny enum and thread it through the two spawners:

```rust
enum ChildFd { Inherit,            // use the shell's real fd N
               Owned(OwnedFd) }    // dup2 me onto N (and I know my CLOEXEC state)

struct ChildStdio { stdin: ChildFd, stdout: ChildFd, stderr: ChildFd,
                    extra: Vec<(RawFd, ChildFd)> }   // "the fd environment a child starts with"
```

`spawn_external_with_fds(stdin: ChildFd, stdout: ChildFd, stderr: ChildFd, …)`
and `fork_and_run_in_subshell(likewise)`. Callers already *know* at
construction time whether an fd is "the shell's stdin" or "a file/pipe I
opened" — today they erase that knowledge into a number and the spawner
guesses it back. With `ChildFd`:
- `Owned` on fd 0/1/2 is dup2'd/CLOEXEC-cleared like any other fd → **#132
  fixed in both spawners at once**, including the unfiled fork-path sibling.
- `Owned(OwnedFd)` makes leak/double-close a type error → the #78 fd-leak
  half largely evaporates (error returns drop the OwnedFd).
- The `went_external` close-bookkeeping split shrinks drastically.

*M (mechanical but touches every call site of both spawners: ~8 callers).
Risk: moderate — behavior-preserving by construction, verified by the
fd-torture harness. Subsumes: #132, #78 (leak half), the H2b latent sibling.*

### Phase 2 — universal fd hygiene at creation (finishes #130's job)
One creation-site rule: **every internally created fd is immediately moved
above the stdio range with an explicit CLOEXEC decision.** Concretely:
- `open_redirect_file(mode, path, noclobber) -> io::Result<OwnedFd>` — the
  single File-open matrix (replaces the 5 verbatim copies + 4 slot variants),
  returning an fd already relocated ≥ 10 + CLOEXEC.
- Route `procsub.rs`, `spawn_heredoc_writer`, and `stdin_pipe.rs` through the
  shared `make_pipe` (or a `make_pipe_cloexec` sibling) instead of raw
  `libc::pipe` — collapsing four pipe-creation policies into one documented
  pair.
- Fold `move_fd_above_stdio` / `relocate_high_cloexec` / `alloc_high_fd` into
  one `move_to_high_fd(fd, cloexec: bool)` (bash's exact shape).

*M. Risk: low-moderate; each sub-item is independently landable. Subsumes:
the remaining #130-class latents (§H2d), most of H3/H4.*

### Phase 3 — one redirect lowering: `lower_redirects()` (fixes #50 class)
Merge `build_child_redir_plan` + `build_child_extra_ops` + the file-open
bodies of `RedirectScope::apply`/`apply_var` into a single
`lower_redirects(&[Redirection], shell, …) -> RedirPlan { ops, held, heredoc_writers }`.
- In-process paths (`RedirectScope`) apply the plan to the shell's own fds
  with save/restore; child paths replay it via `replay_redir_ops`. The
  *semantic interpretation* of every `RedirOp` then exists exactly once.
- **Retire the `RedirectSlot` fast-path**: pipeline stages stop reading
  `slot_stdin/stdout/stderr` and instead pass (pipe-wiring ops + the full
  ordered plan) to the spawners. This directly fixes **#50** (source order on
  stages) and the documented fd>2-stage-heredoc gap, and deletes the four
  ~90-line inline open blocks from each pipeline function.
- Diagnostics: the single lowering function is the natural place to attach
  `line N:` context uniformly → **#69**, and to fix the double/garbled
  spawn-failure message path (**#78** message half, `#77` partially — the
  wrapper-error site also needs its emit moved inside the scope).

*L (this is the big one; suggest two iterations: 3a merge the lowering
functions behind the existing call sites; 3b flip the pipeline stages off the
slot path). Risk: highest of the plan — pipeline redirects are heavily
diff-tested, which is the safety net; keep the slot path deletable-but-intact
for one iteration behind the flip (mirroring the v264 lexer-flip playbook).
Subsumes: #50, #69, #78, part of #77; makes #124 (`&>`)/#125 (word-source
move) single-site features.*

### Phase 4 — merge the two pipeline functions (fixes the biggest sibling pair)
After Phase 3 the bodies of `run_multi_stage` and `run_background_sequence`
differ only in: stage-0 stdin policy, job registration vs `wait_pipeline_raw`,
and blocking vs non-blocking procsub drain. Merge into one
`spawn_pipeline(commands, mode: Foreground | Background) -> SpawnedPipeline`,
with the wait/register epilogue per mode. Deletes ~800 duplicated lines and
makes the #126/#129 policy a single site permanently.

*M–L (mostly deletion once Phase 3 lands). Risk: moderate; the background
path is under-tested today (#128 blocked observing it — hence Phase 0 first).*

### Phase 5 — procsub/job-control alignment (opt-in, riskiest)
Make `procsub::realize` a client of the shared pipe + spawner conventions and
let the *caller's job* decide the pgroup instead of hardcoding
`shell.shell_pgid` (`procsub.rs:81`), so a Ctrl-Z on a job containing a
procsub stops/continues the whole tree coherently (**#97**, **#45**).
*L, needs macOS verification; do last.*

Suggested order: 0 → 1 → 2 → 3a → 3b → 4 → 5. Each phase is
behavior-preserving except where it closes a listed bug.

---

## 5. Quick wins vs deep fixes

**Genuinely one-off (just fix):**
- **#128** — exit-time SIGHUP policy (`shell.rs:281`); not fd plumbing at all.
- **#120** — already fixed; wait-loop, unrelated.
- **#79, #62** — lineno/diagnostic plumbing, outside this class.
- **#129** — one-off *today* (Phase 0), though Phase 4 is what prevents its
  recurrence.

**Symptoms the structural work eliminates (do NOT patch individually):**
- **#132** → Phase 1 (patching it with another `fcntl` special case inside
  the sentinel branches would add a 4th policy to H4).
- **#50** → Phase 3b (unfixable cleanly while the slot path exists).
- **#78** → leak half by Phase 1, message half by Phase 3.
- **#69** → Phase 3 (single lowering site owns the prologue).
- **#77** → Phase 3 + a small emit-inside-scope fix; partially H7.
- **#97/#45** → Phase 5.
- The unfiled `fork_and_run_in_subshell` CLOEXEC sibling (§H2b), the
  `stdin_pipe` fd-0 hazard, and the procsub raw-pipe hazard (§H2d) → Phases 1–2.

---

## 6. Risks / unknowns / unverified claims

1. **Nothing was executed.** All findings are from code reading; the #132
   sibling in `fork_and_run_in_subshell` (§H2b), the `stdin_pipe.rs` fd-0
   hazard, and the procsub freed-fd hazard are *code-path* deductions, not
   reproduced. They should each get a repro script before being filed.
2. **`std::process::Command` constraints.** Phase 1 keeps the `Stdio`
   bridge; an `Owned` fd targeted at 0/1/2 must go through
   `Stdio::from(OwnedFd)` — I did *not* verify std's internal dup handling
   when the owned fd's number equals its target slot (std does handle the
   equal-fd case by duping, but confirm on the MSRV in use). Fallback: route
   such fds through a `pre_exec` dup2 like `replay_redir_ops` already does.
3. **The H7 software-sink layer** (capture/merge routing,
   `redirs_write_stdout`, `final_dests_for_1_2`) is interleaved with the
   real-fd work in paths 1–2. Phases 1–3 deliberately leave it alone, but the
   builtin path's `RedirectScope` refactor (Phase 3) must preserve the
   `force_terminal` / `route_*` decisions exactly — this is where v286-style
   missed-sibling regressions would hide. The `error_message_diff_check.sh`
   capture matrix is the guard.
4. **Interactive/job-control behavior can't be fully validated by the diff
   harnesses** (tty-dependent: tcsetpgrp, WUNTRACED stops, #97). Phases 4–5
   need manual tty testing, and macOS for Phase 5.
5. **Line numbers** cited are from `main` @ 35eed38 and will drift; function
   names are the stable handles. Note the working tree also carries v286-era
   changes per memory (`RedirOp::Move` present), consistent with what I read.
6. **Effort estimates are relative** (S < 1 iteration, M ≈ 1, L ≈ 2): the
   Phase 3b slot-path flip is the least predictable — the slot path encodes
   subtle EOF-propagation behaviors (`make_orphan_pipe_for_eof_reader`,
   M-125) that must be re-expressed in plan form and could surface latent
   ordering bugs currently masked by last-wins semantics.
7. I did not review `wait_loop.rs`/`stream_loop.rs` in depth beyond
   confirming the #120 fix shape (`blocking_wait` fast path,
   `stream_loop.rs:37–43`); the wait side has its own duplication
   (`wait_with_untraced` vs `wait_pipeline_raw` vs `external_capture_loop`)
   but it was not implicated in the recent bug cluster.
