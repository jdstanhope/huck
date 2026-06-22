//! Crate-local stderr macro. `e!(err, "huck: foo {}", x)` is the structured
//! analog of `eprintln!("huck: foo {}", x)`, except it writes to the threaded
//! `err: &mut dyn Write` so the active `StderrSink` (Terminal / Merged /
//! Capture) routes correctly. The write is fallible (ignored) because stderr
//! is best-effort and a write error here must not abort the shell.

#[allow(unused_macros)]
macro_rules! e {
    ($err:expr, $($arg:tt)*) => {{
        let _ = ::std::io::Write::write_fmt($err, format_args!($($arg)*));
        let _ = ::std::io::Write::write_all($err, b"\n");
    }};
}
