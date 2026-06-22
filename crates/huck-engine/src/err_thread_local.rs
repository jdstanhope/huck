//! Thread-local err-sink pointers used by deep call chains where threading
//! `err: &mut dyn Write` through every signature would be too invasive
//! (e.g. `expand()`, `param_expansion`, `Shell` methods, jobs).
//!
//! The executor wraps the user-facing call boundary with [`install_err_sinks`],
//! which stores raw pointers to the active stdout/stderr sinks for the
//! SYNCHRONOUS duration of the wrapped closure. Inner sites use [`with_err`]
//! to materialize a fresh `&mut dyn Write` (via `executor::err_writer`) that
//! routes to the installed sinks, or falls through to `io::stderr()` when no
//! sink is installed (preserves v204 behavior for tests and direct internal
//! calls).
//!
//! Storing the SINKS rather than a pre-materialized writer lets multiple
//! inner sites coexist with one another and with the executor's own short-
//! lived `err_writer(err_sink, sink)` borrows, since each `with_err` call
//! produces a fresh writer with a tight scope.
//!
//! # Safety
//!
//! The thread-local holds raw `NonNull<StdoutSink>` / `NonNull<StderrSink>`.
//! Soundness rests on three invariants, all enforced by the executor's
//! single-threaded contract:
//!
//! 1. `Engine` is neither `Send` nor `Sync`; only one OS thread runs the
//!    executor at a time, so the thread-locals are always observed by the
//!    same thread that installed the pointer.
//! 2. The pointers are set ONLY for the synchronous duration of the wrapping
//!    [`install_err_sinks`] closure. The `Guard` `Drop` clears the cell even
//!    on panic, so a panicking inner call cannot leave a dangling pointer
//!    for the next executor invocation.
//! 3. Nested installs save/restore via the guard's saved-prev state.
//! 4. `with_err`'s closure borrows the pointed-to sinks for a TIGHT scope
//!    (a `Box<dyn Write>` materialized via `err_writer` and dropped before
//!    return). The executor MUST NOT call any function that itself touches
//!    the sinks (e.g. nested `err_writer`) while another `with_err` is
//!    active. Since `with_err` is leaf-level (it writes a message and
//!    returns), this composability is uphold by construction.

use std::cell::Cell;
use std::io::Write;
use std::ptr::NonNull;

use crate::executor::{err_writer, StderrSink, StdoutSink};

// The thread-local stores `'static`-lifetimed NonNulls. The `'static` is a lie —
// see `install_err_sinks` and the safety comment for why this is sound
// (single-threaded, synchronous, RAII-cleared).
type StdoutPtr = NonNull<StdoutSink<'static>>;
type StderrPtr = NonNull<StderrSink<'static>>;

thread_local! {
    static OUT_SINK_PTR: Cell<Option<StdoutPtr>> = const { Cell::new(None) };
    static ERR_SINK_PTR: Cell<Option<StderrPtr>> = const { Cell::new(None) };
}

/// Run `f` with a writer that resolves to the installed sinks (if any) —
/// otherwise falls through to `io::stderr()`. Inner err sites call this to
/// obtain a writer regardless of whether the executor installed one.
pub fn with_err<F, R>(f: F) -> R
where
    F: FnOnce(&mut dyn Write) -> R,
{
    let out_ptr = OUT_SINK_PTR.with(|c| c.get());
    let err_ptr = ERR_SINK_PTR.with(|c| c.get());
    match (out_ptr, err_ptr) {
        (Some(mut o), Some(mut e)) => {
            // SAFETY: the pointers are only set for the synchronous duration
            // of the wrapping `install_err_sinks` call (see module docs). The
            // writer is materialized for a tight scope and dropped before
            // return, so the pointed-to sinks are not aliased.
            let mut w = unsafe { err_writer(e.as_mut(), o.as_mut()) };
            f(&mut *w)
        }
        _ => f(&mut std::io::stderr()),
    }
}

/// RAII guard returned by [`install_err_sinks_raw`]. On `Drop` (including
/// panic-unwind), restores the previously-installed thread-local pointers.
#[must_use = "the guard MUST be held for the synchronous duration of the call \
that materialised the sinks; dropping it immediately uninstalls the sinks"]
pub struct ErrSinkGuard {
    prev_out: Option<StdoutPtr>,
    prev_err: Option<StderrPtr>,
}

impl Drop for ErrSinkGuard {
    fn drop(&mut self) {
        OUT_SINK_PTR.with(|c| c.set(self.prev_out));
        ERR_SINK_PTR.with(|c| c.set(self.prev_err));
    }
}

/// Install `sink` / `err_sink` as the active thread-local sinks for the
/// synchronous duration of the returned [`ErrSinkGuard`]. Each `with_err`
/// call within that scope materialises a fresh writer routed to the active
/// sinks; outside the scope `with_err` falls through to `io::stderr()`.
///
/// # Safety
///
/// The caller MUST guarantee:
/// - The returned guard is dropped before either `sink` or `err_sink` is
///   moved or invalidated (storing it in a local at the same scope as the
///   sinks suffices).
/// - No other code path violates the single-threaded execution contract
///   documented at the module level.
/// - The thread-local sinks are NOT accessed concurrently with the caller's
///   own `&mut sink`/`&mut err_sink` borrows; in practice this is upheld
///   because `with_err`'s closure runs to completion before returning to the
///   executor body, and the writer it materialises is dropped before return.
pub unsafe fn install_err_sinks_raw(
    sink: &mut StdoutSink<'_>,
    err_sink: &mut StderrSink<'_>,
) -> ErrSinkGuard {
    // Erase the borrows' lifetimes to `'static` so they can sit in
    // thread-locals. The guard's `Drop` restores the prior pointers.
    let out_raw: StdoutPtr = unsafe {
        std::mem::transmute::<NonNull<StdoutSink<'_>>, NonNull<StdoutSink<'static>>>(
            NonNull::from(sink),
        )
    };
    let err_raw: StderrPtr = unsafe {
        std::mem::transmute::<NonNull<StderrSink<'_>>, NonNull<StderrSink<'static>>>(
            NonNull::from(err_sink),
        )
    };
    let prev_out = OUT_SINK_PTR.with(|c| c.replace(Some(out_raw)));
    let prev_err = ERR_SINK_PTR.with(|c| c.replace(Some(err_raw)));
    ErrSinkGuard {
        prev_out,
        prev_err,
    }
}

/// Convenience closure-style wrapper around [`install_err_sinks_raw`] for
/// callers that want a scoped install without managing the guard explicitly.
/// Mostly used by tests.
pub fn install_err_sinks<F, R>(
    sink: &mut StdoutSink<'_>,
    err_sink: &mut StderrSink<'_>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    // SAFETY: the guard is dropped before this function returns; the install
    // is scoped to `f` only.
    let _guard = unsafe { install_err_sinks_raw(sink, err_sink) };
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_through_to_stderr_when_no_install() {
        // We can't easily intercept io::stderr() here; just verify with_err
        // does not panic when no sink is installed.
        with_err(|_| {});
    }

    #[test]
    fn install_routes_writes_to_capture_buffer() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut sink = StdoutSink::Terminal;
            let mut err_sink = StderrSink::Capture(&mut buf);
            install_err_sinks(&mut sink, &mut err_sink, || {
                with_err(|err| {
                    err.write_all(b"hello").unwrap();
                });
            });
        }
        assert_eq!(buf, b"hello");
    }

    #[test]
    fn install_is_cleared_after_call() {
        {
            let mut buf: Vec<u8> = Vec::new();
            let mut sink = StdoutSink::Terminal;
            let mut err_sink = StderrSink::Capture(&mut buf);
            install_err_sinks(&mut sink, &mut err_sink, || {});
        }
        let still_installed = ERR_SINK_PTR.with(|c| c.get().is_some())
            || OUT_SINK_PTR.with(|c| c.get().is_some());
        assert!(!still_installed);
    }

    #[test]
    fn merged_sink_routes_to_stdout_capture() {
        let mut buf_out: Vec<u8> = Vec::new();
        {
            let mut sink = StdoutSink::Capture(&mut buf_out);
            let mut err_sink = StderrSink::Merged;
            install_err_sinks(&mut sink, &mut err_sink, || {
                with_err(|err| {
                    err.write_all(b"to-merged").unwrap();
                });
            });
        }
        assert_eq!(buf_out, b"to-merged");
    }

    #[test]
    fn install_clears_on_panic() {
        let mut buf: Vec<u8> = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut sink = StdoutSink::Terminal;
            let mut err_sink = StderrSink::Capture(&mut buf);
            install_err_sinks(&mut sink, &mut err_sink, || {
                panic!("boom");
            });
        }));
        assert!(result.is_err());
        let still_installed = ERR_SINK_PTR.with(|c| c.get().is_some())
            || OUT_SINK_PTR.with(|c| c.get().is_some());
        assert!(!still_installed);
    }
}
