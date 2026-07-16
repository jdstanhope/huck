//! #169: `heredoc_body_to_fd` delivers a heredoc/here-string body WITHOUT a
//! forked writer — a pipe for bodies <= HEREDOC_PIPESIZE, an unlinked temp file
//! above it. These tests pin the path SELECTION (via `fstat`), which a bash-diff
//! harness structurally cannot check: the temp file is unlinked and its path
//! differs per process, so it can never be byte-identical to bash's.

use super::{HEREDOC_PIPESIZE, heredoc_body_to_fd};
use std::os::fd::RawFd;

/// The st_mode file-type bits of `fd` (S_IFIFO for a pipe, S_IFREG for a file).
fn fd_kind(fd: RawFd) -> libc::mode_t {
    // SAFETY: `st` is zeroed POD and `fd` is open; fstat only writes `st`.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::fstat(fd, &mut st) };
    assert_eq!(r, 0, "fstat failed: {}", std::io::Error::last_os_error());
    st.st_mode & libc::S_IFMT
}

/// Drain `fd` to EOF and close it.
fn read_all(fd: RawFd) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: `buf` is a live local; `fd` is open.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        assert!(n >= 0, "read failed: {}", std::io::Error::last_os_error());
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    // SAFETY: `fd` is open and owned by this test.
    unsafe { libc::close(fd) };
    out
}

fn body(n: usize) -> Vec<u8> {
    vec![b'x'; n]
}

#[test]
fn body_at_threshold_uses_a_pipe() {
    // bash: herelen <= 65536 -> pipe. Verified on 5.2.21 via readlink /proc/$$/fd/3.
    let b = body(HEREDOC_PIPESIZE);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(
        fd_kind(fd),
        libc::S_IFIFO,
        "body of exactly 65536 must be a pipe"
    );
    assert_eq!(read_all(fd), b);
}

#[test]
fn body_over_threshold_uses_a_regular_file() {
    // bash: herelen > 65536 -> unlinked temp file. This is the #169 case: with a
    // file there is no writer to block on, so `exec 3<<<BIG` cannot hang.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(
        fd_kind(fd),
        libc::S_IFREG,
        "body of 65537 must be a temp file"
    );
    assert_eq!(read_all(fd), b);
}

#[test]
fn small_body_round_trips_through_the_pipe() {
    let b = b"hello\nworld\n".to_vec();
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFIFO);
    assert_eq!(read_all(fd), b);
}

#[test]
fn empty_body_yields_an_immediately_empty_pipe() {
    let fd = heredoc_body_to_fd(&[], None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFIFO);
    assert!(read_all(fd).is_empty());
}

#[test]
fn pipe_path_fd_is_read_only() {
    // bash: `exec 3<<<hi; echo x >&3` -> "write error: Bad file descriptor".
    let fd = heredoc_body_to_fd(b"hi\n", None).expect("heredoc_body_to_fd");
    // SAFETY: `fd` is open; writing 1 byte from a live local.
    let n = unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
    let err = std::io::Error::last_os_error();
    assert_eq!(n, -1, "a heredoc pipe fd must not be writable");
    assert_eq!(err.raw_os_error(), Some(libc::EBADF), "err: {err}");
    unsafe { libc::close(fd) };
}

#[test]
fn tempfile_path_fd_is_read_only() {
    // bash's temp-file fd has access mode O_RDONLY (fdinfo `flags: 0100000`), so
    // writing to it fails EBADF exactly as in the pipe case.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    // SAFETY: `fd` is open; writing 1 byte from a live local.
    let n = unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
    let err = std::io::Error::last_os_error();
    assert_eq!(n, -1, "a heredoc temp-file fd must not be writable");
    assert_eq!(err.raw_os_error(), Some(libc::EBADF), "err: {err}");
    unsafe { libc::close(fd) };
}

#[test]
fn tempfile_starts_at_offset_zero() {
    // bash reopens the file O_RDONLY (rather than rewinding the writable fd), so
    // the reader starts at 0. Guard: the FIRST bytes read are the body's first.
    let mut b = body(HEREDOC_PIPESIZE + 1);
    b[0] = b'A';
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    let got = read_all(fd);
    assert_eq!(got.len(), b.len());
    assert_eq!(got[0], b'A', "temp-file fd must start at offset 0");
}

#[cfg(target_os = "linux")]
#[test]
fn tempfile_honors_tmpdir_and_is_unlinked() {
    // bash: TMPDIR is honored (shell variable, exported or not) and the file is
    // unlinked immediately -> readlink shows "<path> (deleted)".
    let dir = std::env::temp_dir().join(format!("huck-t1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, Some(dir.to_str().unwrap())).expect("heredoc_body_to_fd");
    let link = std::fs::read_link(format!("/proc/self/fd/{fd}")).expect("readlink");
    let link = link.to_string_lossy().into_owned();
    assert!(
        link.starts_with(dir.to_str().unwrap()),
        "TMPDIR not honored: {link}"
    );
    assert!(
        link.ends_with("(deleted)"),
        "temp file not unlinked: {link}"
    );
    unsafe { libc::close(fd) };
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unusable_tmpdir_falls_back_to_tmp() {
    // bash silently falls back to /tmp when TMPDIR is unusable — verified with
    // both TMPDIR=/nonexistent/xx and TMPDIR=/proc (unwritable): rc 0, no
    // diagnostic, file lands in /tmp.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, Some("/nonexistent/xx")).expect("must fall back to /tmp");
    assert_eq!(fd_kind(fd), libc::S_IFREG);
    assert_eq!(read_all(fd), b);
}
