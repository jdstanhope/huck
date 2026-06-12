//! Process substitution `<(cmd)` / `>(cmd)` runtime support (v150).
//!
//! `realize` creates a pipe (or FIFO fallback), forks the inner command via
//! `fork_and_run_in_subshell`, and returns the `/dev/fd/N` (or FIFO) path plus a
//! `ProcSub` cleanup record. `cleanup` closes the parent fd, unlinks any FIFO, and
//! reaps the inner pid. POSIX-only; macOS-portable (no `/proc`).

use std::io;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use crate::lexer::ProcDir;
use crate::command::{Command, Sequence};
use crate::shell_state::Shell;

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
fn realize_via_devfd(seq: &Sequence, dir: ProcDir, shell: &mut Shell) -> io::Result<(String, ProcSub)> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // Which end the PARENT keeps, and which fds the INNER gets on 0/1.
    // child_closes: the parent's retained end that the child must close for EOF to work.
    let (parent_fd, inner_stdin, inner_stdout, child_closes) = match dir {
        // <(cmd): parent reads cmd's stdout; inner writes to the pipe on fd 1.
        ProcDir::In  => (read_fd,  libc::STDIN_FILENO,  write_fd, read_fd),
        // >(cmd): parent writes cmd's stdin; inner reads from the pipe on fd 0.
        ProcDir::Out => (write_fd, read_fd, libc::STDOUT_FILENO, write_fd),
    };

    // Fork the inner sequence as a subshell. pgid_target = the shell's group so the
    // procsub child is NOT a foreground job and the terminal is never handed to it
    // (avoids the SIGTTOU / terminal-handoff deadlocks of v108/v124). No give_terminal_to.
    let inner = Command::Subshell { body: Box::new(seq.clone()) };
    let child_close_list = [child_closes];
    let pid = crate::executor::fork_and_run_in_subshell(
        &inner, shell,
        inner_stdin, inner_stdout, libc::STDERR_FILENO,
        shell.shell_pgid, &child_close_list, None, None,
    )?;

    // Parent closes the end the inner owns (the inner has its own copy via dup2).
    let inner_end = match dir { ProcDir::In => write_fd, ProcDir::Out => read_fd };
    unsafe { libc::close(inner_end); }

    let path = format!("/dev/fd/{parent_fd}");
    Ok((path, ProcSub { pid, parent_fd, fifo_path: None }))
}

/// FIFO fallback (untested on this platform — used only when /dev/fd is absent).
///
/// When `/dev/fd` is not available (some embedded/minimal Linux setups), we create
/// a named FIFO under TMPDIR instead. The open-ordering is carefully chosen to avoid
/// deadlock: the inner process opens its FIFO end inside its forked subshell (which
/// runs concurrently with the parent), while the parent returns the FIFO path for the
/// outer command to open later. Because the inner process and the outer command both
/// open the FIFO (from opposite ends) independently — not in lockstep here — there is
/// still a potential deadlock if only one end ever opens; this is inherent in FIFO
/// semantics and matches the limitation of all other shells on such platforms.
fn realize_via_fifo(seq: &Sequence, dir: ProcDir, shell: &mut Shell) -> io::Result<(String, ProcSub)> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let pid = unsafe { libc::getpid() };
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let fifo_path = PathBuf::from(format!("{tmpdir}/huck-procsub-{pid}-{counter}"));

    let fifo_cstr = std::ffi::CString::new(fifo_path.to_str().unwrap()).unwrap();
    // 0o600: owner read+write only
    if unsafe { libc::mkfifo(fifo_cstr.as_ptr(), 0o600) } != 0 {
        return Err(io::Error::last_os_error());
    }

    // For <(cmd): inner writes to the FIFO on stdout; parent reads the FIFO path.
    // For >(cmd): inner reads from the FIFO on stdin; parent writes the FIFO path.
    //
    // The inner process opens the FIFO inside the forked subshell. The open will
    // block until the outer command opens the other end, which is correct: the outer
    // command receives the FIFO path and opens it, unblocking the inner open.
    let (inner_flags, _inner_stdio_fd) = match dir {
        ProcDir::In  => (libc::O_WRONLY, libc::STDOUT_FILENO),
        ProcDir::Out => (libc::O_RDONLY, libc::STDIN_FILENO),
    };

    // Build a synthetic sequence that opens the FIFO on the appropriate stdio fd and
    // then runs the user's sequence. We do this by constructing a wrapper Command that
    // opens the FIFO, dup2s it, then execs the body. Since we cannot express
    // "open FIFO then exec" directly in our Command AST, we pass the already-opened
    // FIFO fd as the stdio replacement. Open the FIFO here in the parent (non-blocking
    // to avoid blocking the parent), then pass it as the stdio fd to fork_and_run.
    //
    // O_NONBLOCK prevents the parent's open from blocking. The child will block on its
    // own FIFO open (see note above) — but since we're passing an already-opened fd,
    // the child actually inherits the parent's fd via dup2, sidestepping child-open.
    let open_flags = inner_flags | libc::O_NONBLOCK;
    let fifo_fd = unsafe { libc::open(fifo_cstr.as_ptr(), open_flags) };
    if fifo_fd < 0 {
        let _ = std::fs::remove_file(&fifo_path);
        return Err(io::Error::last_os_error());
    }
    // Re-enable blocking on the fd now that open succeeded (for correct pipe semantics).
    unsafe {
        let flags = libc::fcntl(fifo_fd, libc::F_GETFL);
        libc::fcntl(fifo_fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
    }

    let inner = Command::Subshell { body: Box::new(seq.clone()) };
    let child_close_list: &[RawFd] = &[];
    let (inner_stdin, inner_stdout) = match dir {
        ProcDir::In  => (libc::STDIN_FILENO, fifo_fd),
        ProcDir::Out => (fifo_fd, libc::STDOUT_FILENO),
    };
    let pid_child = crate::executor::fork_and_run_in_subshell(
        &inner, shell,
        inner_stdin, inner_stdout, libc::STDERR_FILENO,
        shell.shell_pgid, child_close_list, None, None,
    )?;

    // Parent closes the fifo_fd — the child has inherited a dup2'd copy.
    unsafe { libc::close(fifo_fd); }

    let path = fifo_path.to_str().unwrap().to_string();
    Ok((path, ProcSub { pid: pid_child, parent_fd: -1, fifo_path: Some(fifo_path) }))
}

/// Tear down one realized process substitution: close the parent fd, unlink any FIFO,
/// reap the inner pid (finished once the pipe is closed).
pub fn cleanup(ps: ProcSub) {
    if ps.parent_fd >= 0 {
        unsafe { libc::close(ps.parent_fd); }
    }
    if let Some(p) = &ps.fifo_path {
        let _ = std::fs::remove_file(p);
    }
    let mut status = 0;
    unsafe { libc::waitpid(ps.pid, &mut status, 0); }
}
