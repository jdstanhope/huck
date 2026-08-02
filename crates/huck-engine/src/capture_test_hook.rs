//! `#[cfg(test)]` thread-local output capture for the in-process unit-test
//! capture paths (`executor::execute_capturing` and the `#[cfg(test)]`
//! `ExecBuilder::capture`). Replaces the deleted `StdoutSink::Capture` /
//! `StderrSink::Capture` / `StderrSink::Merged` sink variants (#197 Stage 3).
//!
//! Thread-local, so each libtest worker captures independently with NO
//! process-global fd redirect — the production temp-file `capture()` cannot run
//! under the multithreaded `--lib` harness (it would race libtest's own
//! reporter on the redirected descriptor). The in-process builtin writers
//! (`FdWriter` for fd 1, `CaptureStderr` for fd 2) consult `push_out`/`push_err`
//! before touching the real fd; when a capture is active on the thread the bytes
//! land in the thread-local buffer instead.
//!
//! Externals (forked children writing their own real fd 1/2) are NOT intercepted
//! by this hook — the unit tests that captured external output moved to the
//! `capture_tempfile_serial` integration binary (real fork + temp-file capture)
//! and the `comsub_merge_stderr_diff_check.sh` harness.
//!
//! In non-test builds the whole thing is a set of `#[inline] -> false` no-ops,
//! so the writer hooks compile out to a direct real-fd write.

#[cfg(test)]
mod inner {
    use std::cell::RefCell;

    struct CaptureState {
        out: Vec<u8>,
        err: Vec<u8>,
        /// `merge_stderr`: stderr writes fold into `out`; `err` stays empty.
        merge: bool,
        /// Whether stderr is captured at all. When false (`execute_capturing`),
        /// `push_err` returns false so stderr falls through to the real fd 2 —
        /// preserving the old `StderrSink::Terminal` behavior of that path.
        capture_err: bool,
    }

    thread_local! {
        static CAP: RefCell<Option<CaptureState>> = const { RefCell::new(None) };
    }

    /// Is a capture active on this thread?
    pub(crate) fn active() -> bool {
        CAP.with(|c| c.borrow().is_some())
    }

    /// Append stdout bytes to the active capture buffer; returns true if handled.
    pub(crate) fn push_out(bytes: &[u8]) -> bool {
        CAP.with(|c| {
            if let Some(s) = c.borrow_mut().as_mut() {
                s.out.extend_from_slice(bytes);
                true
            } else {
                false
            }
        })
    }

    /// Route stderr bytes: into `out` under merge, into `err` when captured,
    /// otherwise unhandled (false) so the caller writes the real fd 2.
    pub(crate) fn push_err(bytes: &[u8]) -> bool {
        CAP.with(|c| {
            if let Some(s) = c.borrow_mut().as_mut() {
                if s.merge {
                    s.out.extend_from_slice(bytes);
                    true
                } else if s.capture_err {
                    s.err.extend_from_slice(bytes);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        })
    }

    /// Run `f` with a fresh capture buffer installed on this thread; return the
    /// captured `(stdout, stderr, f_result)`. Save/restore of any enclosing
    /// capture makes nesting (an inner `$(...)` inside an outer `capture()`)
    /// safe: the inner window captures its own in-process output and the outer
    /// buffer is restored on return.
    pub(crate) fn with_capture<R>(
        merge: bool,
        capture_err: bool,
        f: impl FnOnce() -> R,
    ) -> (Vec<u8>, Vec<u8>, R) {
        let prev = CAP.with(|c| {
            c.borrow_mut().replace(CaptureState {
                out: Vec::new(),
                err: Vec::new(),
                merge,
                capture_err,
            })
        });
        let r = f();
        let state = CAP.with(|c| c.borrow_mut().take());
        CAP.with(|c| *c.borrow_mut() = prev);
        let state = state.expect("capture state present after with_capture body");
        (state.out, state.err, r)
    }
}

#[cfg(test)]
pub(crate) use inner::{active, push_err, push_out, with_capture};

#[cfg(not(test))]
#[inline]
pub(crate) fn active() -> bool {
    false
}

#[cfg(not(test))]
#[inline]
pub(crate) fn push_out(_bytes: &[u8]) -> bool {
    false
}

#[cfg(not(test))]
#[inline]
pub(crate) fn push_err(_bytes: &[u8]) -> bool {
    false
}
