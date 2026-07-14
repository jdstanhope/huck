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
                // SAFETY: `slot` is one of the shell's live std fds (callers pass
                // STDOUT_FILENO for merged stderr); the borrow lasts only for the
                // dup below. A closed/invalid `slot` degrades to an EBADF error
                // from `try_clone_to_owned`, not UB.
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
    // Kept as part of the ChildStdio surface for close-list bookkeeping; the
    // current callers compute their close lists from per-field `.raw()` before
    // moving fields into the struct, so this convenience accessor has no caller
    // yet (Phase 2 fd-creation migration will use it).
    #[allow(dead_code)]
    pub(crate) fn owned_raws(&self) -> impl Iterator<Item = RawFd> + '_ {
        [&self.stdin, &self.stdout, &self.stderr]
            .into_iter()
            .filter_map(|f| f.raw())
    }
}

/// Set FD_CLOEXEC on a raw fd so it does NOT leak into an exec'd program.
/// (Moved from executor.rs in Phase 2; used by the best-effort relocation
/// fallback and the macOS `make_pipe` path.)
pub(crate) fn set_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Dup `src` to the lowest free fd >= `min` (`F_DUPFD` / `F_DUPFD_CLOEXEC` per
/// `cloexec`). `src` is left OPEN (caller-owned) — this is the dup-not-move
/// primitive the `{var}` sites need (they keep the source and close it via
/// their own `owns_src` logic). Errors: EMFILE/EBADF.
pub(crate) fn dup_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd> {
    let cmd = if cloexec {
        libc::F_DUPFD_CLOEXEC
    } else {
        libc::F_DUPFD
    };
    let fd = unsafe { libc::fcntl(src, cmd, min) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// `dup_to_high_fd` + `close(src)`. Unconditional: relocates even when `src` is
/// already >= `min` (matches the old `relocate_high_cloexec`; the hot-path
/// `fd > 2` no-op conditional lives only in `make_pipe`). On Err, `src` is left
/// OPEN (caller cleans up). huck's analogue of bash's `move_to_high_fd`, but
/// with an explicit `min` + kernel lowest-free-≥min (one `F_DUPFD` syscall, no
/// downward scan): the thresholds (3, 10) are load-bearing and frozen.
pub(crate) fn move_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd> {
    let new = dup_to_high_fd(src, min, cloexec)?;
    unsafe {
        libc::close(src);
    }
    Ok(new)
}

/// THE pipe-creation helper. Both ends are guaranteed >= 3 so a freed std fd
/// (e.g. after `exec <&-`) can never be silently reused as a pipe end and
/// aliased onto a child's stdio (#130, and the procsub/heredoc/stdin_pipe
/// latents). `cloexec` chooses the ends' close-on-exec state: false = inherited
/// across exec (pipeline wiring, procsub /dev/fd/N, heredoc feed); true =
/// shell/embedder-internal (stdin_pipe). The relocation uses the MATCHING dup
/// flavor so a CLOEXEC end keeps CLOEXEC when moved off a low number.
pub(crate) fn make_pipe(cloexec: bool) -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as RawFd; 2];
    // `pipe2(O_CLOEXEC)` is Linux-only; elsewhere create then fcntl both ends.
    #[cfg(target_os = "linux")]
    let ret = unsafe {
        if cloexec {
            libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC)
        } else {
            libc::pipe(fds.as_mut_ptr())
        }
    };
    #[cfg(not(target_os = "linux"))]
    let ret = unsafe {
        let r = libc::pipe(fds.as_mut_ptr());
        if r == 0 && cloexec {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        r
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let (r0, w0) = (fds[0], fds[1]);
    let r = if r0 > 2 {
        r0
    } else {
        match move_to_high_fd(r0, 3, cloexec) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r0);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    let w = if w0 > 2 {
        w0
    } else {
        match move_to_high_fd(w0, 3, cloexec) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    Ok((r, w))
}

#[cfg(test)]
mod tests;
