//! Timer thread that, after a deadline elapses, sets a shared atomic flag and
//! sends SIGTERM to all currently-live external children. Cancelled via a
//! channel send (so a script that finishes before the deadline doesn't leave
//! a dangling sleeping thread).
//!
//! The public surface (`spawn_timer` / `TimerHandle::cancel`) is consumed by
//! the `ExecBuilder::timeout` epilogue.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct TimerHandle {
    handle: JoinHandle<()>,
    cancel_tx: Sender<()>,
}

impl TimerHandle {
    /// Cancel the timer (if it hasn't fired yet) and join the thread.
    pub fn cancel(self) {
        let _ = self.cancel_tx.send(());
        let _ = self.handle.join();
    }
}

/// Spawn a timer thread. When `dur` elapses without a cancel, sets `flag` to
/// true and sends SIGTERM to every pid currently in `pids`.
///
/// `Duration::ZERO` is handled specially: the flag is set synchronously
/// before the thread spawns, so callers that immediately invoke a fast
/// builtin (with no command-boundary checkpoints) still see the timeout.
pub fn spawn_timer(
    dur: Duration,
    flag: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<libc::pid_t>>>,
) -> TimerHandle {
    // ZERO duration: latch the flag immediately so the builder's epilogue
    // observes it even if the timer thread loses the race with `cancel()`.
    if dur.is_zero() {
        flag.store(true, Ordering::Relaxed);
    }
    let (cancel_tx, cancel_rx) = channel::<()>();
    let handle = std::thread::spawn(move || {
        match cancel_rx.recv_timeout(dur) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {
                // Cancelled before the deadline.
            }
            Err(RecvTimeoutError::Timeout) => {
                flag.store(true, Ordering::Relaxed);
                if let Ok(guard) = pids.lock() {
                    for &pid in guard.iter() {
                        unsafe {
                            libc::kill(pid, libc::SIGTERM);
                        }
                    }
                }
            }
        }
    });
    TimerHandle { handle, cancel_tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn timer_fires_after_deadline() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::from_millis(50), Arc::clone(&flag), Arc::clone(&pids));
        std::thread::sleep(Duration::from_millis(150));
        assert!(flag.load(Ordering::Relaxed), "flag should be set");
        h.cancel();
    }

    #[test]
    fn timer_cancel_prevents_fire() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::from_secs(60), Arc::clone(&flag), Arc::clone(&pids));
        let start = Instant::now();
        h.cancel();
        assert!(start.elapsed() < Duration::from_secs(1), "cancel should return immediately");
        std::thread::sleep(Duration::from_millis(50));
        assert!(!flag.load(Ordering::Relaxed), "flag should NOT be set after cancel");
    }

    #[test]
    fn timer_zero_duration_fires_immediately() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::ZERO, Arc::clone(&flag), Arc::clone(&pids));
        std::thread::sleep(Duration::from_millis(50));
        assert!(flag.load(Ordering::Relaxed));
        h.cancel();
    }
}
