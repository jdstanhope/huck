# fd-plumbing Phase 1 (`ChildFd`/`ChildStdio`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the raw-fd-number sentinels in huck's two child spawners with a
concrete owned `ChildStdio`/`ChildFd` type so a redirect fd that lands on a freed
0/1/2 no longer vanishes on exec (#132 + its `fork_and_run_in_subshell` sibling),
and pipeline-stage spawn failures stop leaking fds (#78 leak-half).

**Architecture:** A new `pub(crate)` module `crates/huck-engine/src/child_fd.rs`
holds `enum ChildFd { Inherit, Owned(OwnedFd) }` and `struct ChildStdio { stdin,
stdout, stderr }` plus construction/duplication helpers, leaning on
`std::os::fd::OwnedFd` for RAII (single-ownership, close-on-drop, CLOEXEC travels
with the fd). Both spawners (`spawn_external_with_fds`, `fork_and_run_in_subshell`
in `executor.rs`) change signature to CONSUME a `ChildStdio`; all 11 call sites
build the value from knowledge they already have, and every hand-written
"`if fd > 2 { close }`" / `went_external` bookkeeping site is deleted.

**Tech Stack:** Rust (edition 2024), `std::os::fd::{OwnedFd, BorrowedFd, RawFd}`,
`std::process::{Command, Stdio}`, `libc`. Two engine crates: `huck-engine`
(library) and `huck` (binary). Bash-diff harnesses under `tests/scripts/`.

## Global Constraints

- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- OOM constraint: **NEVER** `cargo test --workspace`. Per-crate single-threaded:
  `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`; build the binary
  with `cargo build -p huck`; integration binaries run locally single-threaded
  under `ulimit -v 6000000` (e.g.
  `ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`).
- Reference the issue: the branch PR will `Closes #132`
  (https://github.com/jdstanhope/huck/issues/132) + `Closes` the §H2b issue
  (filed during this work) + addresses the #78 leak-half
  (https://github.com/jdstanhope/huck/issues/78).
- Behavior-preserving by construction EXCEPT the listed bug fixes (#132, §H2b,
  #78 leak-half); the `fd_torture` harness + the full
  `tests/scripts/run_diff_checks.sh` sweep are the regression net.

**Design reference:** `docs/superpowers/specs/2026-07-13-fd-plumbing-phase1-design.md`
(spec §1–§5, §Testing). **Parent context:**
`docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md` §4 Phase 1.
Line numbers below are approximate on `main` @ `958061f`; **function names are the
stable handles** — re-locate by name if a line has drifted.

**Settled fact (do not re-litigate):** `Stdio::from(OwnedFd)` correctly handles an
owned fd whose number already equals its target slot (0/1/2) — std duplicates it
to ≥ 3 in the parent and dup2s it in the child, clearing CLOEXEC. Verified from
std source (1.95.0) and an empirical probe. No `pre_exec` dup2 fallback is needed.

---

## Task 1: `child_fd.rs` module + unit tests

**Files:**
- Create: `crates/huck-engine/src/child_fd.rs`
- Create: `crates/huck-engine/src/child_fd/tests.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add `pub(crate) mod child_fd;`, ~line 15)

**Interfaces:**
- Produces (consumed by Task 2):
  - `enum ChildFd { Inherit, Owned(OwnedFd) }` (`pub(crate)`, `#[derive(Debug)]`, NO `Clone`)
  - `unsafe fn ChildFd::owned_raw(fd: RawFd) -> ChildFd`
  - `fn ChildFd::raw(&self) -> Option<RawFd>`
  - `fn ChildFd::try_clone(&self) -> io::Result<ChildFd>`
  - `fn ChildFd::try_clone_resolving(&self, slot: RawFd) -> io::Result<ChildFd>`
  - `fn ChildFd::into_raw(self) -> Option<RawFd>`
  - `impl From<OwnedFd> for ChildFd`, `impl From<File> for ChildFd`
  - `struct ChildStdio { pub(crate) stdin: ChildFd, pub(crate) stdout: ChildFd, pub(crate) stderr: ChildFd }` (`pub(crate)`, `#[derive(Debug)]`, NO `Clone`)
  - `fn ChildStdio::new(ChildFd, ChildFd, ChildFd) -> ChildStdio`
  - `fn ChildStdio::inherit_all() -> ChildStdio`
  - `fn ChildStdio::owned_raws(&self) -> impl Iterator<Item = RawFd> + '_`

**Dead-code note:** these helpers have no caller until Task 2. To keep CI green
(the repo builds with warnings-as-hard-review), put a single
`#![allow(dead_code)]` **inner attribute at the top of `child_fd.rs` only** (not a
crate-wide allow), with the exact comment
`// TODO(phase1-task2): drop this once executor.rs wires ChildStdio through.`
Task 2 removes this line as its final edit. Rationale: one line, one file,
one marked TODO — least residue, and the whole surface goes live together in
Task 2 so per-item allows would just be N copies of the same TODO.

- [ ] **Step 1: Create `crates/huck-engine/src/child_fd.rs`**

```rust
//! Owned representation of "the fd environment a child will start with"
//! (fd-plumbing remediation Phase 1; see the 2026-07-13 engine review §4 and
//! docs/superpowers/specs/2026-07-13-fd-plumbing-phase1-design.md).
//!
//! A child stdio slot is either `Inherit` (use the shell's real fd N as-is) or
//! `Owned` (a file/pipe fd the parent opened for this child; the spawner dup2s
//! it onto N and closing it is the spawner's job). Meaning and ownership travel
//! in the type, never in the fd NUMBER — a freshly opened redirect fd that
//! happens to land on a freed 0/1/2 is `Owned` and gets dup2'd/CLOEXEC-cleared
//! like any other fd (#132, and the fork-path sibling).
//!
//! `OwnedFd` gives RAII: exactly one owner, close-on-drop, so fd leaks and
//! double-closes become type errors (#78 leak half).
//!
//! ## fork / `pre_exec` Drop safety contract (load-bearing)
//! No `OwnedFd` destructor may run in a forked child before `exec`
//! (`pre_exec` is async-signal-constrained; a stray close over a reused fd
//! number is UB). Enforced by construction:
//!  1. The external spawner converts every `OwnedFd` to `Stdio` (or drops it)
//!     in the PARENT before `spawn()`; nothing owned crosses into `pre_exec`.
//!  2. The fork spawner's child branch calls `into_raw()` on all three slots as
//!     its FIRST act, so no `OwnedFd` exists in the child at any point where a
//!     panic-unwind or early drop could close a live fd number. The parent
//!     branch keeps the `ChildStdio` and drops it right after fork.
#![allow(dead_code)] // TODO(phase1-task2): drop this once executor.rs wires ChildStdio through.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

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

impl ChildFd {
    /// Wrap a raw fd this call site exclusively owns (bridge for the
    /// `make_pipe`/`into_raw_fd` sites that still traffic in `RawFd`; Phase 2
    /// migrates those creation sites to `OwnedFd` directly).
    ///
    /// # Safety
    /// `fd` must be open, and no other code may close it or hold an owned
    /// handle to it.
    pub(crate) unsafe fn owned_raw(fd: RawFd) -> Self {
        Self::Owned(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// The raw fd number, if owned. For close-list bookkeeping ONLY — never for
    /// an inherit/close decision (that's the sentinel disease this type kills).
    pub(crate) fn raw(&self) -> Option<RawFd> {
        match self {
            Self::Inherit => None,
            Self::Owned(fd) => Some(fd.as_raw_fd()),
        }
    }

    /// Non-consuming duplicate, for handing the same source to several children
    /// (the bg-pipeline stage-0 /dev/null default can feed multiple stages).
    /// `Inherit` stays `Inherit`; `Owned` dups via `OwnedFd::try_clone`
    /// (`F_DUPFD_CLOEXEC` — the clone is consumed by a spawner, which clears
    /// CLOEXEC via dup2).
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Self::Inherit => Self::Inherit,
            Self::Owned(fd) => Self::Owned(fd.try_clone()?),
        })
    }

    /// Duplicate that RESOLVES `Inherit` against the shell's real fd at `slot`.
    /// Used for kernel-level merged stderr ("stderr := a copy of whatever
    /// stdout will be"): cloning stdout's `ChildFd` for the stderr slot must
    /// dup the real fd 1 when stdout is `Inherit`.
    pub(crate) fn try_clone_resolving(&self, slot: RawFd) -> io::Result<Self> {
        match self {
            Self::Inherit => {
                let real = unsafe { BorrowedFd::borrow_raw(slot) };
                Ok(Self::Owned(real.try_clone_to_owned()?))
            }
            Self::Owned(fd) => Ok(Self::Owned(fd.try_clone()?)),
        }
    }

    /// Consume into a raw fd WITHOUT closing (`Inherit` -> None). The fork
    /// spawner calls this in the child immediately after fork so no `OwnedFd`
    /// destructor can ever run in the forked child.
    pub(crate) fn into_raw(self) -> Option<RawFd> {
        match self {
            Self::Inherit => None,
            Self::Owned(fd) => Some(fd.into_raw_fd()),
        }
    }
}

impl From<OwnedFd> for ChildFd {
    fn from(fd: OwnedFd) -> Self {
        Self::Owned(fd)
    }
}

impl From<File> for ChildFd {
    fn from(f: File) -> Self {
        Self::Owned(f.into())
    }
}

/// The fd environment a child starts with: one `ChildFd` per stdio slot.
/// Exactly three fields — the review's `extra: Vec<(RawFd, ChildFd)>` is Phase 3
/// (it would be dead code today; see the spec Non-goals).
#[derive(Debug)]
pub(crate) struct ChildStdio {
    pub(crate) stdin: ChildFd,
    pub(crate) stdout: ChildFd,
    pub(crate) stderr: ChildFd,
}

impl ChildStdio {
    pub(crate) fn new(stdin: ChildFd, stdout: ChildFd, stderr: ChildFd) -> Self {
        Self { stdin, stdout, stderr }
    }

    pub(crate) fn inherit_all() -> Self {
        Self::new(ChildFd::Inherit, ChildFd::Inherit, ChildFd::Inherit)
    }

    /// Raw fd numbers of the owned slots (close-list bookkeeping; skips Inherit).
    pub(crate) fn owned_raws(&self) -> impl Iterator<Item = RawFd> + '_ {
        [&self.stdin, &self.stdout, &self.stderr]
            .into_iter()
            .filter_map(|f| f.raw())
    }
}

#[cfg(test)]
mod tests;
```

- [ ] **Step 2: Register the module in `lib.rs`**

Add the line (alphabetical slot, right after `pub(crate) mod callbacks_thread_local;` at ~line 15):

```rust
pub(crate) mod child_fd;
```

- [ ] **Step 3: Write the failing unit tests**

Create `crates/huck-engine/src/child_fd/tests.rs`. These use FRESH fds only
(pipes / `/dev/null`), never process-global 0/1/2, so they are safe in the lib
test binary (the fd-isolation rule targets tests that swap real std fds).

```rust
use super::*;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

// F_GETFD probe: returns Ok if fd is open, Err(EBADF) if closed.
fn fd_is_open(fd: RawFd) -> bool {
    unsafe { libc::fcntl(fd, libc::F_GETFD) != -1 }
}

// A fresh owned fd via /dev/null (never 0/1/2 in the test binary).
fn fresh_owned() -> OwnedFd {
    let f = std::fs::File::open("/dev/null").expect("open /dev/null");
    f.into()
}

#[test]
fn try_clone_inherit_stays_inherit() {
    let c = ChildFd::Inherit.try_clone().unwrap();
    assert!(matches!(c, ChildFd::Inherit));
    assert_eq!(c.raw(), None);
}

#[test]
fn try_clone_owned_yields_distinct_open_fd() {
    let orig = ChildFd::Owned(fresh_owned());
    let orig_raw = orig.raw().unwrap();
    let clone = orig.try_clone().unwrap();
    let clone_raw = clone.raw().unwrap();
    assert_ne!(orig_raw, clone_raw, "clone must be a distinct fd number");
    assert!(fd_is_open(orig_raw) && fd_is_open(clone_raw));
    // Dropping one leaves the other open.
    drop(clone);
    assert!(fd_is_open(orig_raw));
    assert!(!fd_is_open(clone_raw));
}

#[test]
fn try_clone_resolving_inherit_dups_the_slot() {
    // Use a fresh pipe read-end as the stand-in "slot" fd (NOT a real 0/1/2).
    let mut fds = [0 as RawFd; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (r, w) = (fds[0], fds[1]);
    let resolved = ChildFd::Inherit.try_clone_resolving(r).unwrap();
    let dup_raw = resolved.raw().expect("Inherit resolved to an Owned dup");
    assert_ne!(dup_raw, r, "resolved dup must be a new fd number");
    assert!(fd_is_open(dup_raw));
    drop(resolved);
    assert!(!fd_is_open(dup_raw));
    unsafe {
        libc::close(r);
        libc::close(w);
    }
}

#[test]
fn into_raw_does_not_close_but_drop_does() {
    let owned = fresh_owned();
    let raw = owned.as_raw_fd();
    let c = ChildFd::Owned(owned);
    let taken = c.into_raw().expect("Owned -> Some(raw)");
    assert_eq!(taken, raw);
    assert!(fd_is_open(taken), "into_raw must NOT close");
    // We now own `taken` again; wrap + drop closes it.
    drop(unsafe { OwnedFd::from_raw_fd(taken) });
    assert!(!fd_is_open(raw), "drop of Owned closes the fd");
    // Inherit -> None, closes nothing.
    assert_eq!(ChildFd::Inherit.into_raw(), None);
}

#[test]
fn owned_raws_skips_inherit_slots() {
    let a = fresh_owned();
    let b = fresh_owned();
    let (ar, br) = (a.as_raw_fd(), b.as_raw_fd());
    let stdio = ChildStdio::new(ChildFd::Owned(a), ChildFd::Inherit, ChildFd::Owned(b));
    let got: Vec<RawFd> = stdio.owned_raws().collect();
    assert_eq!(got, vec![ar, br], "inherit stdout skipped, order preserved");
}

#[test]
fn inherit_all_is_all_inherit() {
    let s = ChildStdio::inherit_all();
    assert_eq!(s.owned_raws().count(), 0);
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p huck-engine --jobs 1 --lib child_fd -- --test-threads 1`
Expected: the 6 `child_fd::tests::*` tests PASS. (They are written to pass against
the Step 1 implementation directly — this module is pure type plumbing, so the
red-then-green rhythm collapses to "write behavior + its test, confirm green.")

- [ ] **Step 5: Verify the whole engine lib still builds warning-clean**

Run: `cargo build -p huck-engine 2>&1 | tail -20`
Expected: no errors; no NEW warnings (the `#![allow(dead_code)]` suppresses the
unused-helper warnings until Task 2 wires them).

- [ ] **Step 6: `cargo fmt` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/child_fd.rs crates/huck-engine/src/child_fd/tests.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v290 task 1: add child_fd module (ChildFd/ChildStdio + helpers)

Types-only module for fd-plumbing Phase 1 (#132). ChildFd { Inherit,
Owned(OwnedFd) } + ChildStdio carry a child's fd environment with RAII
ownership; callers wire in Task 2. Unit tests cover try_clone /
try_clone_resolving / into_raw / owned_raws on fresh fds.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Convert both spawners + all 11 callers (atomic)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (both spawners + 9 fork callers + 2 external callers)
- Modify: `crates/huck-engine/src/procsub.rs` (2 fork callers)
- Modify: `crates/huck-engine/src/child_fd.rs` (delete the Task-1 `#![allow(dead_code)]` line)

**Interfaces:**
- Consumes (from Task 1): all of the `child_fd` surface listed in Task 1's
  Produces block. Bring it into `executor.rs` with
  `use crate::child_fd::{ChildFd, ChildStdio};` and into `procsub.rs` with
  `use crate::child_fd::ChildStdio;`.
- Produces (new spawner signatures, used nowhere outside these two files):
  - `fn spawn_external_with_fds(cmd: &SimpleCommand, shell: &mut Shell, sink: &mut StdoutSink, err_sink: &mut StderrSink, stdio: ChildStdio, pgid_target: i32, parent_fds_to_close: &[RawFd]) -> Result<i32, io::Error>`
  - `pub fn fork_and_run_in_subshell(cmd: &Command, shell: &mut Shell, stdio: ChildStdio, pgid_target: i32, parent_fds_to_close: &[RawFd], stdout_dup_target: Option<i32>, stderr_dup_target: Option<i32>) -> Result<i32, io::Error>`

**Why atomic (one commit):** changing a spawner signature breaks every caller at
compile time; there is no honest half-way tree. The sub-steps below therefore do
NOT each compile — only Step 15 (build clean) is the gate. Do NOT introduce a
sentinel-bridge overload to make intermediate steps compile; the spec forbids any
sentinel/bridge code in the final tree, and a bridge here would be pure throwaway.
Convert in this order (spawners first so the new signatures exist, then callers).

### Spawner rewrites

- [ ] **Step 1: `fork_and_run_in_subshell` — signature + child-side 3-pass install**

In `executor.rs` (~:8659) change the signature from the three
`stdin_fd/stdout_fd/stderr_fd: RawFd` params to a single `stdio: ChildStdio`
(keep `parent_fds_to_close`, `stdout_dup_target`, `stderr_dup_target`). Replace
the child-side dup2/close block (today ~:8697–8722, steps "3. dup2", "4. close
originals", "5. close parent-held") with the 3-pass sequence. The surrounding
child body (signal resets, `setpgid`, step 6 dup targets, `clear_for_subshell`,
`run_command` dispatch, `_exit`) is UNCHANGED.

Before (~:8697–8722):

```rust
            // 3. dup2 the stdio fds to 0/1/2.
            if stdin_fd != 0 {
                libc::dup2(stdin_fd, 0);
            }
            if stdout_fd != 1 {
                libc::dup2(stdout_fd, 1);
            }
            if stderr_fd != 2 {
                libc::dup2(stderr_fd, 2);
            }
            // 4. Close the originals if not already at 0/1/2.
            for fd in [stdin_fd, stdout_fd, stderr_fd] {
                if fd > 2 {
                    libc::close(fd);
                }
            }
            // 5. Close every other pipe fd the parent held [...]
            for &fd in parent_fds_to_close {
                if fd != stdin_fd && fd != stdout_fd && fd != stderr_fd {
                    libc::close(fd);
                }
            }
```

After:

```rust
            // 3-5. Install stdio from ChildStdio. Convert to raw NOW so no
            // OwnedFd destructor can run in the forked child (Drop safety).
            let ChildStdio { stdin, stdout, stderr } = stdio;
            let mut plan: [(Option<RawFd>, RawFd); 3] =
                [(stdin.into_raw(), 0), (stdout.into_raw(), 1), (stderr.into_raw(), 2)];
            let original_raws: [RawFd; 3] = {
                // fd numbers this child owns as stdio sources, -1 for Inherit.
                [
                    plan[0].0.unwrap_or(-1),
                    plan[1].0.unwrap_or(-1),
                    plan[2].0.unwrap_or(-1),
                ]
            };
            // Pass 1 (PRE-MOVE): move any owned source in 0..=2 up to >=3, so
            // pass 2's dup2 always has source != target (clears FD_CLOEXEC ->
            // the §H2b fix) and installs are order-independent. F_DUPFD (not
            // _CLOEXEC): the moved copy must survive exec if its install no-ops.
            for (src, _) in plan.iter_mut() {
                if let Some(s) = *src
                    && s <= 2
                {
                    let moved = libc::fcntl(s, libc::F_DUPFD, 3);
                    if moved >= 0 {
                        libc::close(s);
                        *src = Some(moved);
                    }
                    // On failure keep s: degraded to old behavior, never worse.
                }
            }
            // Pass 2 (INSTALL): sources now all >=3 and pairwise distinct.
            for (src, slot) in plan {
                if let Some(s) = src {
                    libc::dup2(s, slot);
                    libc::close(s);
                }
            }
            // Pass 3: close every parent-held pipe fd, skipping this child's own
            // stdio sources by their ORIGINAL numbers.
            for &fd in parent_fds_to_close {
                if fd != original_raws[0] && fd != original_raws[1] && fd != original_raws[2] {
                    libc::close(fd);
                }
            }
```

The parent branch (after fork, ~:8774) needs no explicit close — the `stdio`
value is already moved into the child branch's `let ChildStdio { .. } = stdio;`?
No: the child branch does `libc::_exit` (never returns), but the destructure
happens only in the `if pid == 0` block. In the parent branch `stdio` is still
live and drops at function end, closing the parent's owned copies. Confirm the
compiler is satisfied that `stdio` is moved on the child path and dropped on the
parent path (it is: the child path diverges).

Add `use crate::child_fd::{ChildFd, ChildStdio};` near the top of `executor.rs`
with the other `use crate::` imports.

- [ ] **Step 2: `spawn_external_with_fds` — signature + Stdio matches**

In `executor.rs` (~:8833) change the three `stdin_fd/stdout_fd/stderr_fd: RawFd`
params to `stdio: ChildStdio`. Replace the sentinel block (~:8956–8987) with
matches. Everything else (resolve, xtrace, dup-target resolution,
`build_child_extra_ops`, chained pre_execs, `process_group`, `mem::forget`) is
UNCHANGED. Remove the now-unused `use std::os::fd::{FromRawFd, OwnedFd};` at
~:8847 (the `OwnedFd::from_raw_fd` calls are gone).

Before (~:8956–8987):

```rust
    let stdin_stdio = if stdin_fd == 0 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stdin_fd)) }
    };
    let stdout_stdio = if stdout_dup_target.is_some() {
        if stdout_fd != 1 {
            unsafe {
                libc::close(stdout_fd);
            }
        }
        Stdio::inherit()
    } else if stdout_fd == 1 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stdout_fd)) }
    };
    let stderr_stdio = if stderr_dup_target.is_some() {
        if stderr_fd != 2 {
            unsafe {
                libc::close(stderr_fd);
            }
        }
        Stdio::inherit()
    } else if stderr_fd == 2 {
        Stdio::inherit()
    } else {
        unsafe { Stdio::from(OwnedFd::from_raw_fd(stderr_fd)) }
    };
```

After:

```rust
    let ChildStdio { stdin, stdout, stderr } = stdio;
    let stdin_stdio = match stdin {
        ChildFd::Inherit => Stdio::inherit(),
        ChildFd::Owned(fd) => Stdio::from(fd),
    };
    let stdout_stdio = if stdout_dup_target.is_some() {
        // Dup on stdout: inherit so the dup2 pre_exec redirects to target.
        // Dropping the owned fd (if any) closes the parent's copy.
        drop(stdout);
        Stdio::inherit()
    } else {
        match stdout {
            ChildFd::Inherit => Stdio::inherit(),
            ChildFd::Owned(fd) => Stdio::from(fd),
        }
    };
    let stderr_stdio = if stderr_dup_target.is_some() {
        drop(stderr);
        Stdio::inherit()
    } else {
        match stderr {
            ChildFd::Inherit => Stdio::inherit(),
            ChildFd::Owned(fd) => Stdio::from(fd),
        }
    };
```

The two early `return Err(...)` above (resolve failure ~:8861, extra-ops failure
~:8887) now drop `stdio` on return → the #78 leak-half fix, automatic.

### Fork-caller conversions (the §4 table, one sub-step per site)

- [ ] **Step 3: F1 — `run_command` Subshell arm (~:671)**

The `fork_and_run_in_subshell(cmd, shell, libc::STDIN_FILENO, stdout_fd,
stderr_fd, ...)` call passes `stdout_fd` (`STDOUT_FILENO` or a capture-pipe write
end) and `stderr_fd` (`STDERR_FILENO`, the Merged alias `stdout_fd`, or an
err-capture write end). Build a `ChildStdio` just before the call.

The `(stdout_fd, capture_read_fd)` and `(stderr_fd, capture_err_read_fd)` blocks
(~:615–669) still compute raw fds; keep them. Immediately before the
`fork_and_run_in_subshell` call, add:

```rust
    // Build the child's fd environment. stdin inherits; stdout/stderr are the
    // shell's real streams (Inherit) or freshly-made capture-pipe write ends
    // (Owned). Merged stderr dups whatever stdout resolves to.
    let child_stdout = if stdout_fd == libc::STDOUT_FILENO {
        ChildFd::Inherit
    } else {
        unsafe { ChildFd::owned_raw(stdout_fd) }
    };
    let child_stderr = match err_sink {
        StderrSink::Terminal => ChildFd::Inherit,
        StderrSink::Merged => child_stdout.try_clone_resolving(libc::STDOUT_FILENO)?,
        StderrSink::Capture(_) => unsafe { ChildFd::owned_raw(stderr_fd) },
    };
    let child_stdio = ChildStdio::new(ChildFd::Inherit, child_stdout, child_stderr);
```

Change the call to:

```rust
            let pid = match fork_and_run_in_subshell(
                cmd,
                shell,
                child_stdio,
                if interactive { 0 } else { NO_PGROUP },
                &[],
                None,
                None,
            ) {
```

Then DELETE the now-double-close parent bookkeeping that closed those fds after a
successful fork (~:730–744): the `if stdout_fd != libc::STDOUT_FILENO { close }`
block AND the `if matches!(err_sink, StderrSink::Capture(_)) && stderr_fd != ...
{ close }` block. The `capture_read_fd` / `capture_err_read_fd` READ ends are NOT
touched — they remain raw and are drained later (~:756–787). In the fork-ERROR
arm (~:694–714), DELETE the `if stdout_fd != libc::STDOUT_FILENO { close }` and
`if stderr_fd != libc::STDERR_FILENO && stderr_fd != stdout_fd { close }` blocks
(the moved-in `child_stdio` was consumed by the failed call and already dropped);
KEEP the `capture_read_fd`/`capture_err_read_fd` closes.

> Note the `?` in `try_clone_resolving(...)?`: this arm sits in a function
> returning `ExecOutcome`, not `Result`. Replace the `?` with an explicit match
> that emits a `fork: {}`-style error via `err_writer` and
> `return ExecOutcome::Continue(1)` on `Err`, mirroring the existing fork-error
> arm. (Same pattern wherever a `try_clone*` `?` appears in an `ExecOutcome` fn
> below — expand it to a match; do not leave a bare `?`.)

- [ ] **Step 4: F2 — `run_background_subshell` + retire `AsyncStdin` (~:3060, ~:3120)**

Change `async_default_stdin` (~:3072) to return a `ChildFd` directly and DELETE
the `AsyncStdin` enum (~:3060–3065).

Before (~:3060–3096):

```rust
enum AsyncStdin {
    Inherit,
    DevNull(RawFd),
}

fn async_default_stdin(
    inherit: bool,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<AsyncStdin, ()> {
    if inherit || shell.is_interactive {
        return Ok(AsyncStdin::Inherit);
    }
    use std::os::unix::io::IntoRawFd;
    match File::open("/dev/null") {
        Ok(f) => Ok(AsyncStdin::DevNull(f.into_raw_fd())),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "/dev/null: {}", crate::bash_io_error(&e));
            Err(())
        }
    }
}
```

After:

```rust
/// Decide an async child's default stdin as a `ChildFd`. `inherit` is true when
/// the unit must keep the shell's stdin regardless of interactivity (a bare
/// multi-stage pipeline). Interactive always inherits. Otherwise stdin defaults
/// to `/dev/null` (`Owned`); on open failure emit `/dev/null: <error>` + Err.
fn async_default_stdin(
    inherit: bool,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<ChildFd, ()> {
    if inherit || shell.is_interactive {
        return Ok(ChildFd::Inherit);
    }
    match File::open("/dev/null") {
        Ok(f) => Ok(ChildFd::from(f)),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "/dev/null: {}", crate::bash_io_error(&e));
            Err(())
        }
    }
}
```

In `run_background_subshell` (~:3115–3136), replace the stdin computation + the
call + the manual devnull close:

Before:

```rust
    let stdin_fd = match async_default_stdin(inherit_stdin, shell, sink, err_sink) {
        Ok(AsyncStdin::Inherit) => libc::STDIN_FILENO,
        Ok(AsyncStdin::DevNull(fd)) => fd,
        Err(()) => return ExecOutcome::Continue(1),
    };
    let fork_result = fork_and_run_in_subshell(
        cmd,
        shell,
        stdin_fd,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
        /*parent_fds_to_close=*/ &[],
        None,
        None,
    );
    if stdin_fd != libc::STDIN_FILENO {
        unsafe {
            libc::close(stdin_fd);
        }
    }
```

After:

```rust
    let stdin = match async_default_stdin(inherit_stdin, shell, sink, err_sink) {
        Ok(c) => c,
        Err(()) => return ExecOutcome::Continue(1),
    };
    let fork_result = fork_and_run_in_subshell(
        cmd,
        shell,
        ChildStdio::new(stdin, ChildFd::Inherit, ChildFd::Inherit),
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
        /*parent_fds_to_close=*/ &[],
        None,
        None,
    );
    // The parent's /dev/null copy (if any) was consumed + dropped by the call.
```

- [ ] **Step 5: F3/F4/E1 — `run_background_sequence` stage-0 default + assign stage + main stage (~:3192, ~:3268, ~:3867)**

(a) Stage-0 default (~:3192–3200). Change `stage0_stdin_default: RawFd` (pushed
into `parent_held`) to a parent-local `stage0_default: ChildFd`:

Before:

```rust
    let stage0_stdin_default: RawFd =
        match async_default_stdin(pipeline.commands.len() > 1, shell, sink, err_sink) {
            Ok(AsyncStdin::Inherit) => libc::STDIN_FILENO,
            Ok(AsyncStdin::DevNull(fd)) => {
                parent_held.push(fd);
                fd
            }
            Err(()) => return ExecOutcome::Continue(1),
        };
```

After:

```rust
    let stage0_default: ChildFd =
        match async_default_stdin(pipeline.commands.len() > 1, shell, sink, err_sink) {
            Ok(c) => c,
            Err(()) => return ExecOutcome::Continue(1),
        };
```

(b) Every reader of `stage0_stdin_default` becomes a per-use clone of
`stage0_default`. There are two (the assign-stage `stdin_fd` ~:3229 and the two
`prev_pipe_read.take().unwrap_or(stage0_stdin_default)` arms ~:3465 and ~:3469).
Convert the per-stage `stdin_fd: RawFd` computation (~:3356–3470) to produce a
`stdin: ChildFd` instead: at the `RedirectSlot::Read` open arm use
`ChildFd::from(f)` (drop the `f.into_raw_fd()`); at the heredoc/herestring arms
use `unsafe { ChildFd::owned_raw(r) }` on the read end from `spawn_heredoc_writer`;
at the two fallback arms use
`match prev_pipe_read.take() { Some(r) => unsafe { ChildFd::owned_raw(r) }, None => match stage0_default.try_clone() { Ok(c) => c, Err(e) => { /* emit "dup: {e}" + bail_teardown_bg */ } } }`.
Remove the `parent_held.retain(|&fd| fd != stdin_fd)` at ~:3473 (the read end is
now owned by `stdin`, never entered `parent_held`).

(c) Assign-only stage (~:3268). Build `ChildStdio` from the local `stdin: ChildFd`
(clone of `stage0_default`), stdout (`owned_raw(w)` for the pipe write end, or
`Inherit`), stderr `Inherit`; pass it to `fork_and_run_in_subshell`. DELETE the
post-fork `if stdout_fd > 2 { retain + close }` (~:3280–3285) and the fork-error
`if stdout_fd > 2 { close }` (~:3314–3318) — the write end is owned by the moved
`ChildStdio`. Do NOT push the pipe write end `w` to `parent_held` (~:3237 pushes
both r and w today → push only r).

(d) The explicit stdout/stderr redirect blocks (~:3476–3677) and the pipe/orphan
blocks (~:3680–3768): make each open arm produce a `ChildFd` (via
`ChildFd::from(file)`), and each pipe write end via `owned_raw(w)` WITHOUT pushing
`w` to `parent_held` (push only the read end `r`). The per-arm error paths that
today do `if stdin_fd > 2 { close }` / `close(explicit_stdout_fd)` become plain
`bail_teardown_bg(...)` calls — the constructed `ChildFd`s drop automatically.

(e) The classify+spawn dispatch (~:3863–3893): build the `ChildStdio` ONCE from
the local `stdin`/`stdout`/`stderr` `ChildFd`s before `classify_stage`, and move
it into whichever spawner. DELETE `let went_external;` (~:3863) and both arms'
`went_external = true/false;`. Pass `child_stdio` to both
`spawn_external_with_fds` (E1) and `fork_and_run_in_subshell` (F4). Compute
`fds_to_close_in_child` from `parent_held` (now containing only read ends +
sibling write ends), plus append `stage0_default.raw()` when the stage's stdin is
the shared default AND `Some` (so the child closes the parent's default copy —
preserving today's fd table). DELETE the entire `if !went_external { ... }`
post-spawn close blocks (~:3904–3919 error arm, ~:3937–3956 success arm) and the
`for fd in [stdout_fd, stdin_fd, stderr_fd] { if fd > 2 { retain } }` bookkeeping;
replace with `parent_held.retain(...)` only for the read ends already handled.
Since fds are owned, the only remaining parent close is the final
`for fd in parent_held.drain(..)` (~:3981) which closes leftover read ends —
unchanged.

- [ ] **Step 6: F5 — `run_coproc` (~:6674)**

Before:

```rust
    let pid = match fork_and_run_in_subshell(
        body,
        shell,
        in_r,
        out_w,
        libc::STDERR_FILENO,
        0,
        &[in_w, out_r],
        None,
        None,
    ) {
```

After:

```rust
    let pid = match fork_and_run_in_subshell(
        body,
        shell,
        ChildStdio::new(
            unsafe { ChildFd::owned_raw(in_r) },
            unsafe { ChildFd::owned_raw(out_w) },
            ChildFd::Inherit,
        ),
        0,
        &[in_w, out_r],
        None,
        None,
    ) {
```

DELETE the post-fork parent close of the child ends (~:6707–6710
`libc::close(in_r); libc::close(out_w);`) — they are owned by the moved
`ChildStdio` and dropped by the call. In the fork-ERROR arm (~:6687–6692) DELETE
`libc::close(in_r); libc::close(out_w);` but KEEP `libc::close(in_w);
libc::close(out_r);` (those are the parent-kept ends, still raw).

- [ ] **Step 7: F6 — `run_multi_stage` assign-only stage (~:7033)**

Build `ChildStdio::new(ChildFd::Inherit, stdout_child, ChildFd::Inherit)` where
`stdout_child` is `unsafe { ChildFd::owned_raw(w) }` for the pipe/capture write
end (~:6982 / ~:7009) or `ChildFd::Inherit` for `STDOUT_FILENO`. Do NOT push the
write end `w` to `parent_held` (~:6985 pushes both — push only `r`; for the
capture arm ~:7008 keep pushing the read end `r` to `parent_held`). Pass the
`ChildStdio` to `fork_and_run_in_subshell`. DELETE the post-fork
`if stdout_fd > 2 { remove + close }` (~:7046–7054).

- [ ] **Step 8: F7/E2 — `run_multi_stage` main stage (~:7104–7797)**

Mirror Step 5's transformation in `run_multi_stage`, with these DELTAS from the
bg version (this function has capture + Merged, which bg lacks):

- Stdin block (~:7104–7213): identical arm rewrite to Step 5(b), EXCEPT the
  fallback arms use `ChildFd::Inherit` for the `STDIN_FILENO` case (there is no
  stage-0 devnull default here): `_ => match prev_pipe_read.take() { Some(r) =>
  unsafe { ChildFd::owned_raw(r) }, None => ChildFd::Inherit }`. Heredoc/herestring
  arms push the writer pid to `heredoc_writers` (unchanged) and wrap the read end
  as `owned_raw(r)`.
- Stdout block (~:7379–7497): open arms → `ChildFd::from(file)`; inter-stage
  pipe and capture arms → `owned_raw(w)`, pushing ONLY the read end `r` (or
  `capture_read_fd`) to `parent_held`, NOT `w`.
- Stderr block (~:7512–7557): `explicit_stderr_fd` open → `ChildFd::from(file)`;
  `StderrSink::Terminal` → `ChildFd::Inherit`; `StderrSink::Merged` →
  `stdout.try_clone_resolving(libc::STDOUT_FILENO)` (this REPLACES the
  `stderr_fd = stdout_fd` alias at ~:7517 and fixes the §5 double-`OwnedFd`); the
  per-stage `libc::dup(shared)` capture arm (~:7521) → wrap the dup result via
  `owned_raw(dup_fd)`. On the `try_clone_resolving`/`dup` error path, emit
  `dup: {e}` and `bail_teardown_stage` as today.
- Classify+spawn (~:7658–7691): build `child_stdio` ONCE before `classify_stage`;
  DELETE `let went_external;` and both `went_external = ...;`; pass `child_stdio`
  to both spawners. DELETE the `if !went_external { ... }` error-arm closes
  (~:7705–7721), the post-spawn `if !went_external { ... }` closes (~:7756–7768),
  and the `if stdout_fd > 2 { remove + (if !went_external) close }` block
  (~:7774–7785) — replace the last with a plain
  `parent_held.retain(|&fd| Some(fd) != <the read end already handled>)` only
  where a read end must survive; owned write ends need no parent close. KEEP the
  `capture_read_fd`/`capture_err_read_fd` survival logic at ~:7808–7815 (those
  READ ends stay raw and are drained after the loop).
- Per-arm error paths in the stdin/stdout/stderr blocks that today do
  `if stdin_fd > 2 { close }` / `close(explicit_*)` collapse to plain
  `bail_teardown_stage(...)` (constructed `ChildFd`s drop). KEEP the
  `capture_read_fd` explicit-close-before-drain guards where present.

- [ ] **Step 9: F8 — `procsub::realize_via_devfd` (`procsub.rs` ~:75)**

Add `use crate::child_fd::{ChildFd, ChildStdio};` at the top of `procsub.rs`.

Before:

```rust
    let (parent_fd, inner_stdin, inner_stdout, child_closes) = match dir {
        ProcDir::In => (read_fd, libc::STDIN_FILENO, write_fd, read_fd),
        ProcDir::Out => (write_fd, read_fd, libc::STDOUT_FILENO, write_fd),
    };
    // ...
    let child_close_list = [child_closes];
    let pid = crate::executor::fork_and_run_in_subshell(
        &inner,
        shell,
        inner_stdin,
        inner_stdout,
        libc::STDERR_FILENO,
        shell.shell_pgid,
        &child_close_list,
        None,
        None,
    )
    .inspect_err(|_| unsafe {
        libc::close(read_fd);
        libc::close(write_fd);
    })?;

    let inner_end = match dir {
        ProcDir::In => write_fd,
        ProcDir::Out => read_fd,
    };
    unsafe {
        libc::close(inner_end);
    }
```

After — the pipe end destined for the CHILD (`inner_end`) becomes `Owned`, the
other slot inherits; the parent-kept end stays raw. `child_close_list` still
lists the parent-kept end so the child closes it. The post-fork
`close(inner_end)` is DELETED (the moved `ChildStdio` owns + drops it). The
`inspect_err` must NOT double-close the child end: on error the moved
`ChildStdio` is already dropped, so close ONLY the parent-kept end.

```rust
    // parent_fd = the end the parent keeps; inner_end = the end the child owns.
    let (parent_fd, inner_end, child_stdio) = match dir {
        // <(cmd): child writes stdout to the pipe; parent reads.
        ProcDir::In => (
            read_fd,
            write_fd,
            ChildStdio::new(ChildFd::Inherit, unsafe { ChildFd::owned_raw(write_fd) }, ChildFd::Inherit),
        ),
        // >(cmd): child reads stdin from the pipe; parent writes.
        ProcDir::Out => (
            write_fd,
            read_fd,
            ChildStdio::new(unsafe { ChildFd::owned_raw(read_fd) }, ChildFd::Inherit, ChildFd::Inherit),
        ),
    };
    let child_close_list = [parent_fd]; // the child must close the parent-kept end
    let pid = crate::executor::fork_and_run_in_subshell(
        &inner,
        shell,
        child_stdio,
        shell.shell_pgid,
        &child_close_list,
        None,
        None,
    )
    .inspect_err(|_| unsafe {
        // child_stdio (with inner_end) already dropped on the error path;
        // close only the parent-kept end here.
        libc::close(parent_fd);
    })?;
    let _ = inner_end; // owned by child_stdio; nothing to close in the parent.
```

Note: the old code named `inner_stdin`/`inner_stdout`/`child_closes`; those
locals are removed. The `let path = format!("/dev/fd/{parent_fd}");` tail
(~:100) is UNCHANGED (`parent_fd` still bound).

- [ ] **Step 10: F9 — `procsub::realize_via_fifo` (`procsub.rs` ~:175)**

Before:

```rust
    let child_close_list: &[RawFd] = &[];
    let pid_child = crate::executor::fork_and_run_in_subshell(
        &inner,
        shell,
        libc::STDIN_FILENO,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        shell.shell_pgid,
        child_close_list,
        None,
        None,
    )
```

After:

```rust
    let child_close_list: &[RawFd] = &[];
    let pid_child = crate::executor::fork_and_run_in_subshell(
        &inner,
        shell,
        ChildStdio::inherit_all(),
        shell.shell_pgid,
        child_close_list,
        None,
        None,
    )
```

- [ ] **Step 11: Remove the Task-1 dead-code allow**

In `crates/huck-engine/src/child_fd.rs` DELETE the line
`#![allow(dead_code)] // TODO(phase1-task2): drop this once executor.rs wires ChildStdio through.`
All helpers now have callers.

- [ ] **Step 12: Grep for stragglers**

Run: `grep -n "went_external\|AsyncStdin\|stdin_fd == 0\|stdout_fd == 1\|stderr_fd == 2\|Stdio::inherit()" crates/huck-engine/src/executor.rs`
Expected: NO `went_external`, NO `AsyncStdin`, NO `== 0/1/2` stdio sentinels
remain; `Stdio::inherit()` appears ONLY inside the two new spawner matches
(Steps 1–2) and the dup-target arms. Any other hit is an un-converted site — fix
it before proceeding.

- [ ] **Step 13: `cargo fmt` + build the engine lib clean**

Run: `cargo fmt --all && cargo build -p huck-engine 2>&1 | tail -30`
Expected: no errors, **no warnings** (unused-import warnings for the removed
`FromRawFd`/`OwnedFd`/`IntoRawFd` uses are red flags — delete those imports).

- [ ] **Step 14: Build the binary + run engine lib tests**

Run:
```bash
cargo build -p huck 2>&1 | tail -5
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -20
```
Expected: binary builds clean; all engine lib tests PASS (~1773 + the 6 new
`child_fd` tests).

- [ ] **Step 15: Verify the two Problem-section repros now match bash**

Run (from repo root):
```bash
cd /tmp && printf 'x\n' > inA
echo "== #132 =="; diff <(bash -c 'exec <&-; /bin/cat < inA | /bin/cat; echo end') \
                        <(/home/john/projects/huck/target/debug/huck -c 'exec <&-; /bin/cat < inA | /bin/cat; echo end') && echo MATCH
echo "== H2b =="; diff <(bash -c 'exec <&-; { /bin/cat; } < inA | /bin/cat; echo end') \
                       <(/home/john/projects/huck/target/debug/huck -c 'exec <&-; { /bin/cat; } < inA | /bin/cat; echo end') && echo MATCH
cd - >/dev/null
```
Expected: both print `MATCH` (previously huck printed `Bad file descriptor`).

- [ ] **Step 16: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/procsub.rs crates/huck-engine/src/child_fd.rs
git commit -m "$(cat <<'EOF'
v290 task 2: thread ChildStdio through both spawners + all 11 callers

Both spawners now CONSUME a ChildStdio; the raw-fd-number sentinels
(stdin_fd==0 -> inherit, fd>2 -> close) and the went_external
close-bookkeeping are deleted. fork_and_run_in_subshell installs stdio
via a 3-pass (pre-move owned 0..=2 up to >=3, dup2, close) so a redirect
fd on a freed std slot no longer skips the CLOEXEC-clearing dup2 (#132 +
its fork-path sibling). spawn_external_with_fds early returns now drop
the owned fds (#78 leak half). AsyncStdin retired in favor of ChildFd.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Flip `fd_torture` green, file §H2b, docs

**Files:**
- Modify: `tests/scripts/fd_torture_diff_check.sh` (header + 4 new cases)
- Modify: `docs/architecture.md` (module-table row + executor pointer)
- Modify: `docs/superpowers/specs/2026-07-13-fd-plumbing-phase1-design.md` (replace `#NNN`)

- [ ] **Step 1: Enable the 4 `fd_torture` cases + update the exclusion header**

In `tests/scripts/fd_torture_diff_check.sh` change the header comment (lines 7–8)
from mentioning both #132 and #50 to leaving ONLY #50 excluded:

```bash
# Deliberately excluded until its fixing phase: stage redirect source-order (#50).
```

Add these `check` lines in the "freed std fds x pipelines" group (after ~line 32).
`inA` already exists in `$WORK` (contains `FA`):

```bash
# --- #132 + §H2b: redirect fd landing on a freed std slot (Phase 1 fix) ---
check "132 external stage on freed fd0" 'exec <&-; cat < inA | cat; echo end'
check "H2b compound stage on freed fd0" 'exec <&-; { /bin/cat; } < inA | /bin/cat; echo end'
check "132 stdout to file on freed fd1" 'exec >&-; /bin/echo hi > f | cat; cat f'
check "132 bg pipeline file redirect"   'exec <&-; cat < inA | cat & wait; echo end'
```

- [ ] **Step 2: Build both binaries + run `fd_torture` green**

Run:
```bash
cargo build -p huck 2>&1 | tail -3
cargo build --release --locked --bin huck 2>&1 | tail -3
ulimit -v 6000000; bash tests/scripts/fd_torture_diff_check.sh 2>&1 | tail -25
```
Expected: `Fail: 0` — every case PASS, including the 4 new ones and the existing
#128/#129 checks.

- [ ] **Step 3: Full bash-diff sweep**

Run:
```bash
( ulimit -v 1500000; timeout 1200 bash tests/scripts/run_diff_checks.sh ) 2>&1 | tail -40
```
Expected: the sweep reports all harnesses green (each on its own default binary;
never override `HUCK_BIN`).

- [ ] **Step 4: Prioritized `-p huck` integration binaries**

Run each (single-threaded, memory-guarded); expected PASS for every one:
```bash
for t in bg_sequence coproc subshell subshell_pipeline pipeline_subshell \
         subshell_pipeline_position fd_dup named_fd external_fd_redirects \
         builtin_fd_ordering builtin_stdout_dup builtin_pipe_flush \
         compound_redirects heredoc_forked_writer heredoc here_string \
         process_sub captured_pipeline_drain pipefail sigpipe io_error \
         noclobber wait async_list disown_h disown_pid jobs_flags \
         exit_inherits function_redirect cmdsub_subshell \
         subshell_pipeline_pty procsub_stop_pty jobcontrol_pgroup_pty; do
  echo "== $t =="
  ( ulimit -v 6000000; cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 ) 2>&1 | tail -3
done
```
Expected: `test result: ok` for each. (If any pty test is flaky on the box, re-run
it once in isolation; a persistent failure is a real regression — stop and fix.)

- [ ] **Step 5: File the §H2b issue and capture its number**

Run:
```bash
gh issue create \
  --label divergence --label bug --label sev:medium \
  --title "InProcess pipeline-stage file redirect on freed 0/1/2 vanishes on the grandchild's exec (fork_and_run_in_subshell sibling of #132)" \
  --body "$(cat <<'EOF'
Sibling of #132 in the OTHER spawner. When a compound / builtin / function
pipeline stage (classified InProcess -> fork_and_run_in_subshell) has a file
redirect whose fd lands on a freed std slot (e.g. after `exec <&-`), the
child-side `if stdin_fd != 0 { dup2 }` sentinel skips the CLOEXEC-clearing
dup2; if that in-process child then execs an external grandchild, the fd
vanishes on the grandchild's exec.

Confirmed divergent on the v289 binary:

    printf 'x\n' > inA
    huck -c 'exec <&-; { /bin/cat; } < inA | /bin/cat; echo end'
    # huck: /bin/cat: -: Bad file descriptor        bash: x + end

Fixed in the same PR as #132 (fd-plumbing Phase 1, v290): the child-side
install pre-moves any owned fd in 0..=2 up to >=3 before dup2, so dup2 always
has source != target and clears FD_CLOEXEC.

Class: same as #132 (raw-fd-number sentinel), distinct spawner.
EOF
)"
```
Record the printed issue number as `<H2B>`.

- [ ] **Step 6: Replace the spec `#NNN` placeholder with `<H2B>`**

In `docs/superpowers/specs/2026-07-13-fd-plumbing-phase1-design.md` replace the
two `#NNN` occurrences (the header issue line and any body reference) with the
real `#<H2B>` number from Step 5.

- [ ] **Step 7: Update `docs/architecture.md`**

Add a module-table row for `child_fd.rs` (place it in the same table that lists
`executor.rs`, ~line 105) with text:

```
| `child_fd.rs` | `ChildFd { Inherit, Owned(OwnedFd) }` + `ChildStdio` — the owned "fd environment a child starts with", consumed by the two spawners. RAII (close-on-drop) replaces the old raw-fd-number sentinels (fd-plumbing Phase 1). |
```

Append to the `executor.rs` row's "Pipeline fork/exec" sentence:
` Child stdio is carried by `ChildStdio`/`ChildFd` (see `child_fd.rs`).`

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add tests/scripts/fd_torture_diff_check.sh docs/architecture.md docs/superpowers/specs/2026-07-13-fd-plumbing-phase1-design.md
git commit -m "$(cat <<'EOF'
v290 task 3: fd_torture #132/H2b cases green + docs + file H2b issue

Enable the 4 fd_torture cases the Phase 1 fix flips green (exec <&- +
file redirect into a pipeline stage, external + compound + bg + freed
fd1); leave only #50 excluded. Add the child_fd.rs architecture row and
executor pointer. File the fork-path sibling issue and wire its number
into the spec.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review (writing-plans checklist)

**1. Spec coverage** — every spec section maps to a task:
- §1 types/module + fork/Drop contract → Task 1 (module doc carries the contract).
- §2 external spawner match → Task 2 Step 2.
- §3 fork spawner 3-pass pre-move/install/close → Task 2 Step 1.
- §4 all 11 caller sites (F1 Step 3, F2 Step 4, F3/F4/E1 Step 5, F5 Step 6,
  F6 Step 7, F7/E2 Step 8, F8 Step 9, F9 Step 10) → Task 2.
- §5 double-close elimination (Merged `try_clone_resolving`) → Task 2 Step 8 stderr delta.
- §Testing (unit tests, 4 fd_torture flips, full sweep, integration binaries) →
  Task 1 Step 3–4 + Task 3 Steps 1–4.
- §Non-goals (deferred `extra`, spawner-move deferred, #78 message-half, #50) →
  respected: `ChildStdio` has 3 fields, spawners stay in `executor.rs`, only the
  leak-half is claimed, #50 stays excluded from `fd_torture`.
- §Risks — the close-deletion breadth risk is mitigated by Step 12's grep gate.
- Issue-filing (§H2b) + spec `#NNN` fixup → Task 3 Steps 5–6.

**2. Placeholder scan** — the only intentional placeholder token is `#NNN` in the
spec, resolved in Task 3 Step 6, and `<H2B>` (a capture variable defined in Task 3
Step 5). No `TBD`/`TODO`/"similar to"/"handle edge cases" remain in executable
steps; the one `TODO(phase1-task2)` is a deliberate, explicitly-removed marker
(Task 2 Step 11). Every code step shows complete code.

**3. Type/signature consistency** — the two spawner signatures in Task 2's
Interfaces block match their use in Steps 3–10 (all callers pass exactly
`(cmd, shell, [sink, err_sink,] ChildStdio, pgid_target, &[RawFd] [, Option, Option])`).
`ChildFd`/`ChildStdio` helper names (`owned_raw`, `try_clone`,
`try_clone_resolving`, `into_raw`, `raw`, `owned_raws`, `new`, `inherit_all`,
`from`) are identical between Task 1's definitions and Task 2's call sites. The
`async_default_stdin` return type change (`Result<ChildFd, ()>`) is consistent
between its definition (Step 4) and both readers (Steps 4 and 5).

**Task count: 3.**
