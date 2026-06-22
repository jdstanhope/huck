//! Shared test-only synchronization (the cwd-changing tests must not race).
use std::sync::Mutex;
pub static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Gate for tests that swap process-global fd 0 via `stdin_pipe::with_stdin_fd0`.
/// Tests using fd 0 redirection in parallel would clobber each other.
pub static STDIN_LOCK: Mutex<()> = Mutex::new(());
