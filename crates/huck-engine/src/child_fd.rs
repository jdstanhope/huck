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
        Self {
            stdin,
            stdout,
            stderr,
        }
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
