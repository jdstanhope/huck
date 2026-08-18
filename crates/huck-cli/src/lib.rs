//! `huck-cli` — huck's interactive REPL + rustyline adapters (see crate docs).
mod completion_helper;
mod paint;
mod readline_apply;
mod repl;

/// Strip SGR escapes from a rendered line.
///
/// Exported so the pty harness (`tests/highlight_render_pty.rs`, in the `huck`
/// crate) asserts the width contract against the SAME implementation the
/// painter's own tests use, rather than keeping a second copy of it (#666).
pub use paint::strip_sgr;
pub use repl::run;
