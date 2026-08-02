//! Materialize a fresh stderr writer for deep call chains where threading
//! `err: &mut dyn Write` through every signature would be too invasive
//! (e.g. `expand()`, `param_expansion`, `Shell` methods, jobs).
//!
//! Production is one-model (#197 Stage 3): every diagnostic goes to the real
//! fd 2 via [`crate::executor::err_writer`], and a `#[cfg(test)]` thread-local
//! capture (see `capture_test_hook`) intercepts those writes when active. The
//! old sink-pointer install machinery is gone — `err_writer()` already routes
//! to the right destination on its own, so [`with_err`] just hands its closure
//! a writer built from it.

use std::io::Write;

/// Run `f` with a freshly-materialized stderr writer. Inner err sites call this
/// to obtain a `&mut dyn Write` that routes to the real fd 2 (and, under a
/// `#[cfg(test)]` capture, into the captured stderr buffer).
pub fn with_err<F, R>(f: F) -> R
where
    F: FnOnce(&mut dyn Write) -> R,
{
    let mut w = crate::executor::err_writer();
    f(&mut *w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_through_to_stderr_when_no_capture() {
        // With no capture active the writer targets the real fd 2; just verify
        // `with_err` does not panic.
        with_err(|_| {});
    }

    #[test]
    fn routes_writes_to_capture_buffer() {
        // A `#[cfg(test)]` thread-local capture intercepts the `err_writer` a
        // `with_err` builds.
        let (_out, err, ()) = crate::capture_test_hook::with_capture(false, true, || {
            with_err(|err| {
                err.write_all(b"hello").unwrap();
            });
        });
        assert_eq!(err, b"hello");
    }

    #[test]
    fn merged_capture_routes_to_stdout() {
        // Under `merge` the captured stderr folds into stdout.
        let (out, _err, ()) = crate::capture_test_hook::with_capture(true, true, || {
            with_err(|err| {
                err.write_all(b"to-merged").unwrap();
            });
        });
        assert_eq!(out, b"to-merged");
    }
}
