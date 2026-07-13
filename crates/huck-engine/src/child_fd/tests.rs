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
