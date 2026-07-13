//! Process substitution `<(cmd)` / `>(cmd)` runtime support (v150).
//!
//! `realize` creates a pipe (or FIFO fallback), forks the inner command via
//! `fork_and_run_in_subshell`, and returns the `/dev/fd/N` (or FIFO) path plus a
//! `ProcSub` cleanup record. `cleanup` closes the parent fd, unlinks any FIFO, and
//! reaps the inner pid. POSIX-only; macOS-portable (no `/proc`).

use crate::child_fd::{ChildFd, ChildStdio};
use crate::command::{Command, Sequence};
use crate::lexer::ProcDir;
use crate::shell_state::Shell;
use std::io;
use std::os::unix::io::RawFd;
use std::path::PathBuf;

// `Clone` is derived only because `Shell` derives `Clone` and holds a
// `Vec<ProcSub>`. A `ProcSub` owns a live fd + child pid, so a cloned copy must
// NEVER be cleaned up independently: the sole runtime `Shell` clone site
// (`run_substitution`) resets `procsub_pending` to empty, so no clone ever carries
// a live `ProcSub`. Do not call `cleanup` on a cloned `ProcSub`.
#[derive(Debug, Clone)]
pub struct ProcSub {
    pub pid: i32,
    pub parent_fd: RawFd,
    pub fifo_path: Option<PathBuf>,
}

/// `/dev/fd` is a directory on Linux (→ /proc/self/fd) and macOS (fdescfs). Checked
/// once and cached. We only ever name `/dev/fd/N`; never `/proc`.
fn dev_fd_available() -> bool {
    use std::sync::OnceLock;
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| std::path::Path::new("/dev/fd").is_dir())
}

/// Realize one process substitution. Returns the path string to substitute into the
/// word, plus the cleanup record (caller pushes it onto `shell.procsub_pending`).
pub fn realize(seq: &Sequence, dir: ProcDir, shell: &mut Shell) -> io::Result<(String, ProcSub)> {
    if dev_fd_available() {
        realize_via_devfd(seq, dir, shell)
    } else {
        // FIFO fallback (untested on this platform — verified by inspection only).
        // Used on systems where /dev/fd is not available.
        realize_via_fifo(seq, dir, shell)
    }
}

/// Primary path: use a pipe + /dev/fd/N naming.
fn realize_via_devfd(
    seq: &Sequence,
    dir: ProcDir,
    shell: &mut Shell,
) -> io::Result<(String, ProcSub)> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // Which end the PARENT keeps (parent_fd), and which end the CHILD owns as its
    // stdio (inner_end). The child owns inner_end via `ChildStdio`; the other
    // stdio slot inherits. The parent-kept end stays raw.
    let inner = Command::Subshell {
        body: Box::new(seq.clone()),
    };
    let (parent_fd, inner_end, child_stdio) = match dir {
        // <(cmd): child writes stdout to the pipe; parent reads.
        ProcDir::In => (
            read_fd,
            write_fd,
            ChildStdio::new(
                ChildFd::Inherit,
                unsafe { ChildFd::owned_raw(write_fd) },
                ChildFd::Inherit,
            ),
        ),
        // >(cmd): child reads stdin from the pipe; parent writes.
        ProcDir::Out => (
            write_fd,
            read_fd,
            ChildStdio::new(
                unsafe { ChildFd::owned_raw(read_fd) },
                ChildFd::Inherit,
                ChildFd::Inherit,
            ),
        ),
    };

    // Fork the inner sequence as a subshell. pgid_target = the shell's group so the
    // procsub child is NOT a foreground job and the terminal is never handed to it
    // (avoids the SIGTTOU / terminal-handoff deadlocks of v108/v124). No give_terminal_to.
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
        // child_stdio (owning inner_end) was already dropped on the error path;
        // close only the parent-kept end here.
        libc::close(parent_fd);
    })?;
    // inner_end was owned by child_stdio and closed in the parent by the call.
    let _ = inner_end;

    let path = format!("/dev/fd/{parent_fd}");
    Ok((
        path,
        ProcSub {
            pid,
            parent_fd,
            fifo_path: None,
        },
    ))
}

/// FIFO fallback (used only when /dev/fd is absent — unreachable on Linux/macOS).
///
/// Correct rendezvous: the parent does NOT pre-open the FIFO. Instead the inner
/// sequence is wrapped in a redirect so the CHILD opens its FIFO end (blocking until
/// the outer command opens the other end). The outer command receives the FIFO path
/// and opens it, unblocking the child. This avoids the ENXIO that a parent-side
/// O_WRONLY|O_NONBLOCK open produces when no reader exists yet.
fn realize_via_fifo(
    seq: &Sequence,
    dir: ProcDir,
    shell: &mut Shell,
) -> io::Result<(String, ProcSub)> {
    use crate::lexer::{Word, WordPart};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let pid = unsafe { libc::getpid() };
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let fifo_path = PathBuf::from(format!("{tmpdir}/huck-procsub-{pid}-{counter}"));

    let fifo_cstr = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
    // 0o600: owner read+write only
    if unsafe { libc::mkfifo(fifo_cstr.as_ptr(), 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }

    // Build a Word that holds the FIFO path as a single literal part.
    let path_word = Word(vec![WordPart::Literal {
        text: fifo_path.to_str().unwrap().to_string(),
        quoted: true,
    }]);

    // Wrap the inner sequence in a redirect so the CHILD opens its FIFO end:
    //   <(cmd) → cmd > FIFO  (child opens O_WRONLY, blocks until outer opens O_RDONLY)
    //   >(cmd) → cmd < FIFO  (child opens O_RDONLY, blocks until outer opens O_WRONLY)
    use crate::command::{FileMode, RedirFd, RedirOp, Redirection};
    let redirects: Vec<Redirection> = match dir {
        ProcDir::In => vec![Redirection {
            fd: RedirFd::Number(1),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target: path_word,
            },
        }],
        ProcDir::Out => vec![Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::File {
                mode: FileMode::ReadOnly,
                target: path_word,
            },
        }],
    };
    let inner_body = Command::Subshell {
        body: Box::new(seq.clone()),
    };
    let inner = Command::Redirected {
        inner: Box::new(inner_body),
        redirects,
    };

    // Fork: child inherits stdio (the wrapped redirect overrides the relevant one
    // inside the child). Parent holds no fd — the FIFO path is passed to the outer.
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
    .inspect_err(|_| {
        let _ = std::fs::remove_file(&fifo_path);
    })?;

    let path = fifo_path.to_str().unwrap().to_string();
    Ok((
        path,
        ProcSub {
            pid: pid_child,
            parent_fd: -1,
            fifo_path: Some(fifo_path),
        },
    ))
}

/// Tear down one realized process substitution: close the parent fd, unlink any FIFO,
/// wait for the inner process to exit (waitpid blocks until exit — the inner may still
/// be running when cleanup is called).
pub fn cleanup(ps: ProcSub) {
    if ps.parent_fd >= 0 {
        unsafe {
            libc::close(ps.parent_fd);
        }
    }
    if let Some(p) = &ps.fifo_path {
        let _ = std::fs::remove_file(p);
    }
    let mut status = 0;
    unsafe {
        libc::waitpid(ps.pid, &mut status, 0);
    }
}
