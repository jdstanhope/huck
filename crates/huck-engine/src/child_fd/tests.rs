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
    // Park the source at a private HIGH number (>=700) first. move_to_high_fd
    // closes it, and we then probe that it's closed. A low source number would
    // race: a concurrent test's open() reuses the lowest free fd the instant we
    // free it, so the negative F_GETFD probe would spuriously see it "open".
    // Lowest-free allocation never wanders up to 700, so a high source is safe.
    let f = std::fs::File::open("/dev/null").unwrap();
    let low = f.into_raw_fd();
    let src = unsafe { libc::fcntl(low, libc::F_DUPFD, 700) };
    assert!(src >= 700, "could not park source at a high fd");
    unsafe { libc::close(low) };

    let hi = move_to_high_fd(src, 10, true).unwrap();
    assert!(hi >= 10);
    // src (the high, non-reusable number) is now closed.
    assert_eq!(unsafe { libc::fcntl(src, libc::F_GETFD) }, -1);
    let flags = unsafe { libc::fcntl(hi, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    unsafe {
        libc::close(hi);
    }
}

#[test]
fn move_to_high_fd_err_on_bad_src_leaves_state_sane() {
    // -1 is deterministically invalid -> EBADF; the fn returns Err without
    // panicking. Using a *closed real* fd number here would race: a concurrent
    // test could reuse that number, the F_DUPFD would succeed, and the close of
    // the original src would then close the OTHER test's live fd. -1 can never
    // be a live fd, so there is nothing to race.
    assert!(move_to_high_fd(-1, 10, false).is_err());
}

#[test]
fn make_pipe_non_cloexec_ends_are_high_and_roundtrip() {
    let (r, w) = make_pipe(false).unwrap();
    assert!(r >= 3 && w >= 3);
    assert_eq!(
        unsafe { libc::fcntl(r, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert_eq!(
        unsafe { libc::fcntl(w, libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    let msg = b"hi\n";
    assert_eq!(unsafe { libc::write(w, msg.as_ptr().cast(), msg.len()) }, 3);
    let mut buf = [0u8; 8];
    assert_eq!(
        unsafe { libc::read(r, buf.as_mut_ptr().cast(), buf.len()) },
        3
    );
    assert_eq!(&buf[..3], msg);
    unsafe {
        libc::close(r);
        libc::close(w);
    }
}

#[test]
fn make_pipe_cloexec_sets_flag_on_both_ends() {
    let (r, w) = make_pipe(true).unwrap();
    assert!(r >= 3 && w >= 3);
    assert_eq!(
        unsafe { libc::fcntl(r, libc::F_GETFD) } & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC
    );
    assert_eq!(
        unsafe { libc::fcntl(w, libc::F_GETFD) } & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC
    );
    unsafe {
        libc::close(r);
        libc::close(w);
    }
}
