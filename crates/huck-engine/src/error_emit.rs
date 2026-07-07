//! The unified error-message emitter family (v269).
//!
//! huck emits three *classes* of diagnostic, each with its own bash-parity
//! prologue (see `docs/superpowers/specs/2026-07-07-unified-error-emitter-design.md`
//! §1/§3):
//!
//! - [`emit_error`] (macro form: [`sh_error!`]) — RUNTIME errors (builtin
//!   failures, `cd`, command-not-found, arith, `set -o`). Prologue:
//!   `<name>: [line N: ][cmd: ]`.
//! - [`emit_syntax_error`] — SYNTAX/parser errors. Prologue:
//!   `<name>: [-c: ]line N: ` (the `-c:` segment iff the shell was invoked
//!   `-c` — [`Shell::is_command_string`](crate::shell_state::Shell::is_command_string)).
//! - [`emit_cli_error`] — PRE-SHELL errors (bad CLI option, line-editor init)
//!   that occur before a `Shell` exists. Prologue: `<basename>: ` — no line,
//!   no `-c:`.
//!
//! All three route through the thread-local sink ([`crate::err_thread_local::with_err`])
//! so output honors `2>&1`/capture redirection, and all three share one
//! prologue builder, `Shell::error_prefix` (now `pub(crate)`, taking a
//! [`Diag`] discriminant). Together they are the *only* code that renders
//! the invocation-name/prologue text — no call site composes
//! `prefix + tail` by hand.

use crate::err_thread_local::with_err;
use crate::shell_state::Shell;

/// Selects which bash prologue form [`Shell::error_prefix`](crate::shell_state::Shell::error_prefix)
/// composes — see the design spec's §3 matrix.
pub enum Diag<'a> {
    /// A runtime error. Prologue: `<name>: [line N: ][cmd: ]`. `cmd` is the
    /// builtin/context name (`Some("cd")`) or `None` (bare `$(( ))`/generic).
    Runtime(Option<&'a str>),
    /// A syntax/parser error, carrying its own line (from the `ParseError`
    /// location, NOT the runtime line counter). Prologue:
    /// `<name>: [-c: ]line N: ` — the `-c:` segment present iff
    /// `Shell::is_command_string`.
    Syntax { line: u32 },
}

/// Emit one bash diagnostic to the current error sink for a RUNTIME error.
/// Prologue: `<name>: [line N: ][cmd: ]`. `cmd` = builtin/context
/// (`Some("cd")`) or `None`.
///
/// Prefer the [`sh_error!`] macro at call sites — it wraps this in
/// `format_args!` so callers write an `eprintln!`-shaped invocation.
pub fn emit_error(shell: &Shell, cmd: Option<&str>, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{}", shell.error_prefix(Diag::Runtime(cmd)));
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}

/// Emit a SYNTAX/parser diagnostic. Prologue: `<name>: [-c: ]line N: `. The
/// `line` comes from the `ParseError`/`LexError`'s own location, not
/// `Shell::current_lineno`; there is never a `cmd:` segment (the parser has
/// no builtin context).
pub fn emit_syntax_error(shell: &Shell, line: u32, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{}", shell.error_prefix(Diag::Syntax { line }));
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}

/// Emit a diagnostic with no `Shell` in scope: `<prog>: <msg>` — no line, no
/// `-c:`. For failures before a `Shell` exists (bad CLI option, line-editor
/// init). `prog` is the invocation basename (bash's `get_name_for_error` for
/// this class uses the basename, unlike the verbatim `$0` used elsewhere).
pub fn emit_cli_error(prog: &str, body: std::fmt::Arguments) {
    with_err(|err| {
        let _ = write!(err, "{prog}: ");
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
    });
}

/// `eprintln!`-shaped wrapper around [`emit_error`]: `sh_error!(shell, cmd,
/// "fmt", args...)`. The first argument is any expression that borrows as
/// `&Shell` (e.g. `self` inside a `Shell` method, or the `shell` parameter
/// inside a builtin). `#[macro_export]`ed so `$crate` resolves to
/// `huck_engine` regardless of the invoking crate (huck-cli uses this too).
#[macro_export]
macro_rules! sh_error {
    ($shell:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error($shell, $cmd, format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::err_thread_local::install_err_sinks;
    use crate::executor::{StderrSink, StdoutSink};

    #[test]
    fn emit_cli_error_is_basename_no_line() {
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        install_err_sinks(&mut out, &mut err, || {
            emit_cli_error("huck", format_args!("boom"));
        });
        assert_eq!(buf, b"huck: boom\n");
    }

    #[test]
    fn emit_error_routes_through_the_sink() {
        let mut sh = Shell::new();
        sh.is_interactive = true;
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        install_err_sinks(&mut out, &mut err, || {
            emit_error(&sh, Some("cd"), format_args!("no such file"));
        });
        assert_eq!(buf, b"huck: cd: no such file\n");
    }

    #[test]
    fn emit_syntax_error_carries_its_own_line() {
        let mut sh = Shell::new();
        sh.is_interactive = false;
        sh.shell_argv0 = "s.sh".to_string();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        install_err_sinks(&mut out, &mut err, || {
            emit_syntax_error(&sh, 7, format_args!("syntax error near unexpected token"));
        });
        assert_eq!(buf, b"s.sh: line 7: syntax error near unexpected token\n");
    }

    #[test]
    fn sh_error_macro_matches_emit_error() {
        let mut sh = Shell::new();
        sh.is_interactive = true;
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        install_err_sinks(&mut out, &mut err, || {
            sh_error!(&sh, None, "readonly variable");
        });
        assert_eq!(buf, b"huck: readonly variable\n");
    }
}
