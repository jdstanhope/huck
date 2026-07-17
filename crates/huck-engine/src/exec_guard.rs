//! Single-threaded-execution invariant (issue #184).
//!
//! huck runs subshells, background jobs, and in-process pipeline stages by
//! forking WITHOUT a following `exec`: the child continues in the same address
//! space through `run_command` (malloc, `Vec`/`String`, Rust stdio). POSIX
//! permits only async-signal-safe calls between `fork` and `exec` in a
//! MULTITHREADED process, so this is memory-safe only while the process is
//! single-threaded — which huck is, in production. (The only production fork
//! whose child runs shell code is `executor::fork_and_run_in_subshell`; the
//! `spawn_*_stage` forks do `write`+`_exit` and are async-signal-safe.)
//!
//! This module makes that invariant explicit and enforced. `ExecActive` counts
//! how many executions are in flight — globally, and on this thread.
//! `assert_single_threaded_fork` runs just before the in-process fork; if
//! another thread is executing (`GLOBAL_ACTIVE > LOCAL_DEPTH`), it PANICS with a
//! clear message instead of letting the forked child deadlock on a lock the
//! other thread holds. In a single-threaded process the two counts are equal and
//! the check is a no-op.
//!
//! See `docs/architecture.md` ("Single-threaded execution").

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

static GLOBAL_ACTIVE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static LOCAL_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// RAII marker: an execution is active on this thread while it lives. Construct
/// one at the top of `execute_with_sink`. Re-entrant — nested executions on the
/// same thread each hold their own, and both counters move together, so
/// `GLOBAL_ACTIVE` and this thread's `LOCAL_DEPTH` stay equal for a lone thread
/// regardless of nesting.
pub(crate) struct ExecActive {
    _priv: (),
}

impl ExecActive {
    pub(crate) fn enter() -> Self {
        GLOBAL_ACTIVE.fetch_add(1, Ordering::SeqCst);
        LOCAL_DEPTH.with(|d| d.set(d.get() + 1));
        ExecActive { _priv: () }
    }
}

impl Drop for ExecActive {
    fn drop(&mut self) {
        LOCAL_DEPTH.with(|d| d.set(d.get() - 1));
        GLOBAL_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Panic if an in-process subshell fork is about to happen while ANOTHER thread
/// is executing shell code. Call immediately before `libc::fork()` in
/// `fork_and_run_in_subshell`. No-op in a single-threaded process (production,
/// and any correctly-isolated test): `GLOBAL_ACTIVE == LOCAL_DEPTH`.
pub(crate) fn assert_single_threaded_fork() {
    let global = GLOBAL_ACTIVE.load(Ordering::SeqCst);
    let local = LOCAL_DEPTH.with(|d| d.get());
    if global > local {
        panic!(
            "huck: an Engine is executing on another thread while this thread \
             forks an in-process subshell. huck runs subshells by forking \
             without exec, which is memory-unsafe unless the process is \
             single-threaded. Run each Engine on its own thread only when no \
             other Engine is executing concurrently. (issue #184)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier};
    use std::thread;

    // NOTE: a "no panic when lone" assertion is NOT reliable in this
    // multithreaded lib binary — a concurrent test's `ExecActive` inflates
    // `GLOBAL_ACTIVE`. That direction is verified in the single-threaded
    // integration binary (`tests/forking_execution_serial.rs`), where a real subshell
    // forks without panicking. Here we test only the PANIC direction, which is
    // robust: forcing another thread to hold an `ExecActive` guarantees
    // `GLOBAL_ACTIVE > LOCAL_DEPTH` no matter what else runs.
    #[test]
    fn fork_while_another_thread_executes_panics() {
        let start = Arc::new(Barrier::new(2));
        let release = Arc::new(AtomicBool::new(false));
        let (s2, r2) = (start.clone(), release.clone());
        let other = thread::spawn(move || {
            let _active = ExecActive::enter();
            s2.wait(); // both threads meet here; the other now holds _active
            while !r2.load(Ordering::SeqCst) {
                thread::yield_now();
            }
        });
        start.wait();
        // This thread is executing too (LOCAL_DEPTH = 1); GLOBAL_ACTIVE >= 2.
        let _mine = ExecActive::enter();
        // Expected: one panic message printed to stderr; it is caught here.
        let caught = std::panic::catch_unwind(assert_single_threaded_fork);
        assert!(
            caught.is_err(),
            "a fork while another thread executes must panic"
        );
        release.store(true, Ordering::SeqCst);
        other.join().unwrap();
    }
}
