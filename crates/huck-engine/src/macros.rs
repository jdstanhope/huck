//! Crate-local stderr macro. `e!(err, "huck: foo {}", x)` is the structured
//! analog of `eprintln!("huck: foo {}", x)`, except it writes to the threaded
//! `err: &mut dyn Write` so the active `StderrSink` (Terminal / Merged /
//! Capture) routes correctly. The write is fallible (ignored) because stderr
//! is best-effort and a write error here must not abort the shell.

macro_rules! e {
    ($err:expr, $($arg:tt)*) => {{
        let _ = ::std::io::Write::write_fmt($err, format_args!($($arg)*));
        let _ = ::std::io::Write::write_all($err, b"\n");
    }};
}

/// Render an io::Error like bash: the bare strerror string, dropping Rust's
/// ` (os error N)` Display suffix. Rust-synthesized errors (no errno) keep
/// their Display text. The Display of an OS error is the documented
/// `"{strerror} (os error {errno})"`, so stripping that exact suffix yields the
/// same text bash gets from strerror(errno).
pub(crate) fn bash_io_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let s = e.to_string();
            match s.strip_suffix(&format!(" (os error {n})")) {
                Some(stripped) => stripped.to_string(),
                None => s,
            }
        }
        None => e.to_string(),
    }
}

#[cfg(test)]
mod bash_io_error_tests {
    use super::bash_io_error;
    use std::io::{Error, ErrorKind};

    #[test]
    fn os_error_drops_the_rust_suffix() {
        // ENOENT (2): Display is "No such file or directory (os error 2)".
        let e = Error::from_raw_os_error(2);
        assert_eq!(bash_io_error(&e), "No such file or directory");
    }

    #[test]
    fn permission_denied_os_error() {
        let e = Error::from_raw_os_error(13);
        assert_eq!(bash_io_error(&e), "Permission denied");
    }

    #[test]
    fn synthesized_error_keeps_display() {
        // No raw_os_error → keep the Display text unchanged.
        let e = Error::new(ErrorKind::InvalidData, "stream did not contain valid UTF-8");
        assert_eq!(bash_io_error(&e), "stream did not contain valid UTF-8");
    }
}
