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

/// Emit a runtime diagnostic to a CALLER-PROVIDED writer (redirect-aware).
/// Prologue: `<name>: [line N: ][cmd: ]`, same as [`emit_error`], but written
/// directly to `w` instead of the thread-local sink. This is the builtin
/// path: builtins receive `out`/`err` writer parameters from
/// `run_builtin(program, args, out, err, shell)`, and those writers (not the
/// thread-local sink) carry the in-memory `route_err_to_out`/`route_out_to_err`
/// swap that a bare-builtin `2>&1`/`>&2` redirect performs. A site that holds
/// such a writer MUST emit to it — see the design spec's §1(a2) — otherwise
/// the diagnostic is lost under `$(builtin 2>&1)` capture.
pub fn emit_error_to(shell: &Shell, w: &mut dyn std::io::Write, cmd: Option<&str>, body: std::fmt::Arguments) {
    let _ = write!(w, "{}", shell.error_prefix(Diag::Runtime(cmd)));
    let _ = w.write_fmt(body);
    let _ = w.write_all(b"\n");
}

/// `eprintln!`-shaped wrapper around [`emit_error_to`]: `sh_error_to!(shell,
/// w, cmd, "fmt", args...)`. Use at any builtin site that holds an `out`/`err`
/// writer descending from `run_builtin` — see [`emit_error_to`].
#[macro_export]
macro_rules! sh_error_to {
    ($shell:expr, $w:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error_to($shell, $w, $cmd, format_args!($($arg)*))
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

/// The v269 enforcement invariant: every production emission source has been
/// converted onto the emitter family (`sh_error!`/`sh_error_to!`/`emit_error`/
/// `emit_error_to`/`emit_syntax_error`/`emit_cli_error`) — no call site
/// composes the interactive `"huck: "` prologue by hand as a literal string.
/// Models v268's `lexer_has_no_production_parser_dependency`
/// (`crates/huck-syntax/src/lexer.rs`): `include_str!` each source, strip
/// `#[cfg(test)]` code and comment lines, then assert the literal is gone
/// from what remains.
///
/// `Shell::error_prefix` (this crate's `shell_state.rs`) is exempt by
/// construction, not by exclusion: it builds the interactive prefix from the
/// literal `"huck"` (no colon) and appends a *formatted* `": "` at runtime
/// (`format!("{name}: ")`), so no `"huck: "` literal ever appears in its
/// source — this file (`error_emit.rs`) is itself in the source list below
/// and passing proves that.
#[cfg(test)]
mod prologue_literal_invariant {
    const SOURCES: &[(&str, &str)] = &[
        ("builtins.rs", include_str!("builtins.rs")),
        ("executor.rs", include_str!("executor.rs")),
        ("expand.rs", include_str!("expand.rs")),
        ("param_expansion.rs", include_str!("param_expansion.rs")),
        ("completion_builtins.rs", include_str!("completion_builtins.rs")),
        ("shell_state.rs", include_str!("shell_state.rs")),
        ("restricted.rs", include_str!("restricted.rs")),
        ("shell.rs", include_str!("shell.rs")),
        ("stdin_pipe.rs", include_str!("stdin_pipe.rs")),
        ("history.rs", include_str!("history.rs")),
        ("engine.rs", include_str!("engine.rs")),
        ("cwd_scope.rs", include_str!("cwd_scope.rs")),
        ("error_emit.rs", include_str!("error_emit.rs")),
        ("../huck-cli/src/repl.rs", include_str!("../../../crates/huck-cli/src/repl.rs")),
    ];

    /// Strips every `#[cfg(test)]`-guarded item (a standalone attribute line
    /// followed by a brace-delimited `fn`/`impl`/`mod`, or a semicolon-only
    /// item like `#[cfg(test)] use …;`) from `src`, brace-matching each one
    /// individually rather than assuming a single trailing test module —
    /// several of these files interleave small `#[cfg(test)]` helpers among
    /// production code before their final `mod tests { … }` block.
    fn strip_test_blocks(src: &str) -> String {
        let lines: Vec<&str> = src.lines().collect();
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim() == "#[cfg(test)]" {
                i += 1;
                let mut depth = 0i32;
                let mut opened = false;
                while i < lines.len() {
                    let l = lines[i];
                    for ch in l.chars() {
                        match ch {
                            '{' => { depth += 1; opened = true; }
                            '}' => depth -= 1,
                            _ => {}
                        }
                    }
                    i += 1;
                    if opened && depth <= 0 {
                        break;
                    }
                    if !opened && l.trim_end().ends_with(';') {
                        break; // e.g. `#[cfg(test)] use foo::Bar;`
                    }
                }
                continue;
            }
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
        }
        out
    }

    /// Drops whole-line comments (`///`, `//!`, `//`) so prose that mentions
    /// `"huck: "` (design notes, e.g. `builtins.rs`'s job-spec-resolution doc
    /// or the `restricted.rs`/`history.rs` reworked comments) doesn't
    /// false-positive. Does not attempt trailing end-of-line `//` comments —
    /// none of these files use that style for prose mentioning the prefix.
    fn strip_comment_lines(src: &str) -> String {
        src.lines()
            .filter(|l| {
                let t = l.trim_start();
                !(t.starts_with("///") || t.starts_with("//!") || t.starts_with("//"))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_production_site_composes_huck_colon_by_hand() {
        for (name, src) in SOURCES {
            let stripped = strip_comment_lines(&strip_test_blocks(src));
            assert!(
                !stripped.contains("huck: "),
                "{name}: found a literal `\"huck: \"` in production code outside \
                 the emitter family (sh_error!/sh_error_to!/emit_error/emit_error_to/\
                 emit_syntax_error/emit_cli_error) — route this site through the \
                 emitter instead of composing the prologue by hand",
            );
        }
    }
}
