//! PTY regression test: Ctrl-Z (SIGTSTP) on a foreground command/pipeline that
//! contains a process substitution must NOT hang the shell.
//!
//! Bug: when a foreground job was stopped, huck blocking-`waitpid`'d the process
//! substitution's child to drain it. But a stopped job's procsub child is still
//! alive (its consumer is stopped too), so the blocking wait deadlocked and the
//! shell never returned to the prompt (`find … | tee >(awk …)` + Ctrl-Z wedged
//! huck). Fix: drain procsubs NON-blocking on the stopped path. These tests send
//! Ctrl-Z (`\x1a`) and verify the prompt comes back (the next `echo` runs); a
//! per-`wait_for` timeout turns a regression-hang into a failed assertion.
//!
//! The harness drains the PTY master CONTINUOUSLY from a dedicated reader thread
//! — exactly what a real terminal emulator does. This matters on macOS: huck's
//! line editor (`rustyline`) flips raw mode with `tcsetattr(TCSADRAIN)`, and a
//! BSD/XNU pty's `TCSADRAIN` blocks until the master *reader* consumes pending
//! output. A harness that stalls its reader (the old `expectrl` +
//! `thread::sleep` shape) therefore deadlocked huck in `tcsetattr` on macOS even
//! though huck's job control is correct — see issue #97. Draining continuously
//! removes that artifact while still catching the original Linux blocking-
//! `waitpid` deadlock (which hangs regardless of terminal draining).
//!
//! Skips (passes) if no PTY can be allocated (e.g. sandboxed CI).

use std::io::Result as IoResult;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ptyprocess::PtyProcess;

/// A `huck` child on a PTY whose master is drained continuously by a background
/// thread into a shared buffer (a real-terminal model).
struct DrainedPty {
    process: PtyProcess,
    /// A `dup` of the master fd, used for writes. Held for the fd's lifetime.
    writer: std::fs::File,
    buf: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl DrainedPty {
    /// Spawn `huck --norc` on a PTY and start draining. Returns `None` if no PTY
    /// can be allocated (sandboxed CI) or the interactive prompt never appears.
    fn spawn() -> Option<Self> {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
        // Hermetic: never source the developer's ~/.huckrc (#239).
        cmd.arg("--norc");

        let process = match PtyProcess::spawn(cmd) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("procsub_stop_pty: skipping — no PTY: {e}");
                return None;
            }
        };

        // Two independent `dup`s of the master: one for the reader thread, one
        // for writes. Reading and writing the same pty master from two threads
        // is safe at the syscall level.
        let writer = match process.get_raw_handle() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("procsub_stop_pty: skipping — master handle: {e}");
                return None;
            }
        };
        let reader_file = match process.get_raw_handle() {
            Ok(f) => f,
            Err(e) => {
                eprintln!("procsub_stop_pty: skipping — master handle: {e}");
                return None;
            }
        };

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let buf = Arc::clone(&buf);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let fd = reader_file.as_raw_fd();
                let mut tmp = [0u8; 4096];
                loop {
                    let n =
                        unsafe { libc::read(fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len()) };
                    if n > 0 {
                        buf.lock().unwrap().extend_from_slice(&tmp[..n as usize]);
                    } else if n == 0 {
                        break; // EOF: slave fully closed (child gone)
                    } else {
                        match std::io::Error::last_os_error().raw_os_error() {
                            Some(libc::EINTR) => {}
                            // EIO on a pty master means the slave side closed.
                            _ => break,
                        }
                    }
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
                // Keep the dup'd fd open until the thread ends.
                drop(reader_file);
            })
        };

        let mut pty = DrainedPty {
            process,
            writer,
            buf,
            stop,
            reader: Some(reader),
        };

        // Confirm the interactive prompt is alive before starting.
        if pty.send("echo READY_$((6*7))\r").is_err() {
            eprintln!("procsub_stop_pty: skipping — could not write to pty");
            return None;
        }
        if !pty.wait_for("READY_42", Duration::from_secs(8)) {
            eprintln!("procsub_stop_pty: skipping — interactive marker not seen");
            return None;
        }
        Some(pty)
    }

    fn send(&mut self, s: &str) -> IoResult<()> {
        self.writer.write_all(s.as_bytes())?;
        self.writer.flush()
    }

    /// Poll the drained buffer for `needle` until `timeout` elapses.
    fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let needle = needle.as_bytes();
        loop {
            if contains(&self.buf.lock().unwrap(), needle) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for DrainedPty {
    fn drop(&mut self) {
        // Kill the shell so the reader hits EOF on the master, then reap + join.
        let pid = self.process.pid().as_raw();
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        let _ = self.process.wait();
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.reader.take() {
            let _ = h.join();
        }
    }
}

/// True iff `hay` contains `needle` as a contiguous subslice.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn ctrl_z_on_pipeline_with_procsub_does_not_hang() {
    let Some(mut pty) = DrainedPty::spawn() else {
        return;
    };

    // A foreground pipeline whose last stage feeds a process substitution.
    // `sleep 30` produces nothing, so `tee` blocks reading and the `>(cat)`
    // child blocks reading from `tee` — exactly the stopped-but-alive shape.
    let _ = pty.send("sleep 30 | tee >(cat >/dev/null)\r");
    // Let the pipeline + procsub fully set up before stopping it.
    std::thread::sleep(Duration::from_millis(500));
    // Ctrl-Z (SUB): stops the foreground job's process group.
    let _ = pty.send("\x1a");
    // The shell must return to the prompt and run the next line.
    let _ = pty.send("echo AFTER_$((1+1))\r");
    let responsive = pty.wait_for("AFTER_2", Duration::from_secs(8));

    // Best-effort cleanup of the stopped job.
    let _ = pty.send("kill -9 %1 2>/dev/null\r");

    assert!(
        responsive,
        "Ctrl-Z on a pipeline containing a process substitution hung the shell \
         (no prompt back / next command did not run)"
    );
}

#[test]
fn ctrl_z_on_command_with_output_procsub_does_not_hang() {
    let Some(mut pty) = DrainedPty::spawn() else {
        return;
    };

    // A single foreground command with an OUTPUT process-substitution redirect.
    // `sleep 30` runs with its stdout going to `>(cat)`, which blocks reading.
    let _ = pty.send("sleep 30 > >(cat >/dev/null)\r");
    std::thread::sleep(Duration::from_millis(500));
    let _ = pty.send("\x1a");
    let _ = pty.send("echo BACK_$((2+2))\r");
    let responsive = pty.wait_for("BACK_4", Duration::from_secs(8));

    let _ = pty.send("kill -9 %1 2>/dev/null\r");

    assert!(
        responsive,
        "Ctrl-Z on a command with an output process substitution hung the shell \
         (no prompt back / next command did not run)"
    );
}
