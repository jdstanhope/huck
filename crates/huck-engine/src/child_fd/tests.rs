use super::*;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

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
    // Dropping one leaves the other open. Assert only the positive: a post-drop
    // `assert!(!fd_is_open(clone_raw))` would race a concurrent open() reusing
    // the just-freed fd number under a parallel test runner (CI runs threaded).
    // Close-on-drop is OwnedFd's guaranteed contract.
    drop(clone);
    assert!(fd_is_open(orig_raw));
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
    // close-on-drop is std's contract; not asserted (parallel-runner fd-reuse race).
    drop(resolved);
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
    // We now own `taken` again; wrap + drop reclaims it (so the test doesn't
    // leak). Not asserting the post-drop close: it would race a concurrent
    // open() reusing the number under a parallel runner. `raw` == `taken`.
    drop(unsafe { OwnedFd::from_raw_fd(taken) });
    let _ = raw;
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

#[test]
fn dup_to_high_fd_keeps_src_open_and_honors_cloexec() {
    // Fresh source fd from /dev/null.
    let f = std::fs::File::open("/dev/null").unwrap();
    let src = f.into_raw_fd();

    // Non-CLOEXEC dup.
    let a = dup_to_high_fd(src, 10, false).unwrap();
    assert!(a >= 10);
    // src still open.
    assert!(unsafe { libc::fcntl(src, libc::F_GETFD) } >= 0);
    let flags = unsafe { libc::fcntl(a, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0);

    // CLOEXEC dup.
    let b = dup_to_high_fd(src, 10, true).unwrap();
    assert!(b >= 10);
    let flags = unsafe { libc::fcntl(b, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);

    unsafe {
        libc::close(a);
        libc::close(b);
        libc::close(src);
    }
}

#[test]
fn move_to_high_fd_closes_src() {
    let f = std::fs::File::open("/dev/null").unwrap();
    let src = f.into_raw_fd();
    let hi = move_to_high_fd(src, 10, true).unwrap();
    assert!(hi >= 10);
    // src is now closed.
    assert_eq!(unsafe { libc::fcntl(src, libc::F_GETFD) }, -1);
    let flags = unsafe { libc::fcntl(hi, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    unsafe {
        libc::close(hi);
    }
}

#[test]
fn move_to_high_fd_err_on_bad_src_leaves_state_sane() {
    // A definitely-closed fd -> EBADF; the fn returns Err without panicking.
    let f = std::fs::File::open("/dev/null").unwrap();
    let bad = f.into_raw_fd();
    unsafe {
        libc::close(bad);
    }
    assert!(move_to_high_fd(bad, 10, false).is_err());
}
