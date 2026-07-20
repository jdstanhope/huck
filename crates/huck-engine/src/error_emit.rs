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
use huck_syntax::command::{Delim, ExpectFailure, Found, ParseError};
use huck_syntax::spell::{spell_delim, spell_token};

/// Selects which nested-context marker (if any) `render_diag_inner`/
/// `emit_syntax_error_ex` prints for a syntax error (v316, #213). `Default`
/// is top-level (no nested context — the ordinary `-c:`/script-name
/// prologue). `Eval` routes through `Diag::Syntax`'s existing `eval_frame`
/// logic unchanged. `CommandSub` routes through `Diag::SyntaxNested` and
/// always prints `command substitution: line N:` (Task 2, #213).
#[derive(Clone, Copy)]
pub(crate) enum Marker {
    Default,
    Eval,
    CommandSub,
}

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
    /// A runtime error that bash emits WITHOUT a `line N:` segment (bash's
    /// `redirection_error()` class, e.g. `redirection error: cannot duplicate
    /// fd: …`). Prologue: `<name>: [cmd: ]` — identical to [`Diag::Runtime`]
    /// minus the line. `cmd` = builtin/context (`Some("cd")`) or `None`.
    RuntimeNoLine(Option<&'a str>),
    /// A syntax error carrying an explicit nested-context marker
    /// (`command substitution:`), which REPLACES `-c:` and ignores
    /// `eval_frame`. Prologue: `<name>: <marker>: line N: `. v316 (#213).
    SyntaxNested { line: u32, marker: &'static str },
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
/// no builtin context). Thin delegate to [`emit_syntax_error_ex`] with no
/// source-echo line.
pub fn emit_syntax_error(shell: &Shell, line: u32, body: std::fmt::Arguments) {
    emit_syntax_error_ex(shell, line, body, None, Marker::Default);
}

/// General syntax-error emitter (v314, #211). `echo` = the source line to
/// reproduce on a second `<prefix>`-line (bash's Shape 1: `near unexpected
/// token` followed by the offending source line quoted in backticks). The
/// `<name>: [-c: ]line N: ` prologue is always built by
/// [`Shell::error_prefix`](crate::shell_state::Shell::error_prefix) and is
/// reused verbatim for the echo line (bash repeats the identical prologue).
pub(crate) fn emit_syntax_error_ex(
    shell: &Shell,
    line: u32,
    body: std::fmt::Arguments,
    echo: Option<&str>,
    marker: Marker,
) {
    with_err(|err| {
        let prefix = match marker {
            Marker::CommandSub => shell.error_prefix(Diag::SyntaxNested {
                line,
                marker: "command substitution",
            }),
            // Default/Eval: eval_frame logic unchanged (`Diag::Syntax`).
            _ => shell.error_prefix(Diag::Syntax { line }),
        };
        let _ = write!(err, "{prefix}");
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
        if let Some(src) = echo {
            let _ = write!(err, "{prefix}`{src}'\n");
        }
    });
}

/// Classify a `ParseError` into bash's three syntax-error shapes and emit it
/// (v314, #211):
///
/// 1. **Near-token**: a real token is present but misplaced —
///    `syntax error near unexpected token `X'` followed by the offending
///    source line quoted in backticks.
/// 2. **Unexpected EOF**: a keyword/paren construct (`if`, `while`, `case`,
///    `(`, `{`, `function`) is still open when input runs out —
///    `syntax error: unexpected end of file`.
/// 3. **Unterminated quote/expansion**: EOF inside an open quote or
///    expansion delimiter (`"`, `'`, `` ` ``, `$(`, `$((`, `${`, `[[`) —
///    `unexpected EOF while looking for matching `X'`.
///
/// `source` is the full input text; `token_line` is the pre-computed 1-based
/// line of the error position — used for Shape 1's echoed source line AND
/// (Task 4) as the delimiter's opening line for the quote/`` ` ``/`$((`/`${`
/// family of Shape 3 errors (`$(`/`(` still use the EOF line instead — see
/// `emit_matching`). v314 Task 4 wires this into both top-level drivers
/// (`shell::process_line_in_sinks`, `builtins::run_sourced_contents_in_sinks`).
/// A nested `eval` context is handled (v315, #209): `eval`'s parse driver
/// pushes an `eval_frame` so `shell.line_base()` is non-zero for the
/// duration, and the `Diag::Syntax` arm consults it to print an `eval: line
/// N:` marker (suppressing the `-c:`/script-name prefix) with the outer
/// line number. A nested command-substitution context is handled too (v316,
/// #213): a backtick body's syntax error surfaces as
/// `ParseError::InCommandSub`, whose render arm recurses `render_diag_inner`
/// against the body with `Marker::CommandSub`, producing bash's `command
/// substitution: line N:` marker. `$()` bodies parse at the top level (not
/// through this nested-reparse path), so they keep the `-c:`/script-name
/// prefix — matching bash.
pub fn render_syntax_diag(shell: &Shell, err: &ParseError, source: &str, token_line: u32) {
    // `token_line` is already offset by `shell.line_base()` (v315, #209: the
    // outer line an `eval` sits on) for DISPLAY purposes, but `source` here is
    // always the LOCAL text being parsed (e.g. the eval string alone) — so
    // indexing into it (the echoed source line) must subtract the base back
    // out first. The entry derives the marker + base from `eval_frame`
    // (byte-identical to pre-v316) and hands the worker the RAW local line;
    // the worker re-derives `display_line = line_base + local_line`, which
    // round-trips exactly back to `token_line`.
    let base = shell.line_base();
    let (marker, line_base) = match shell.eval_frame {
        Some(_) => (Marker::Eval, base),
        None => (Marker::Default, base),
    };
    render_diag_inner(
        shell,
        err,
        source,
        token_line.saturating_sub(base),
        marker,
        line_base,
    );
}

/// Worker for [`render_syntax_diag`]: classifies `err` into bash's three
/// syntax-error shapes (see `render_syntax_diag`'s doc comment) and emits it,
/// given an already-resolved `local_line`/`marker`/`line_base` triple. Split
/// out in v316 (#213) so a nested command-substitution reparse (Task 2) can
/// call this directly with `Marker::CommandSub` and its own `line_base`,
/// bypassing the `eval_frame`-derived entry above.
fn render_diag_inner(
    shell: &Shell,
    err: &ParseError,
    source: &str,
    local_line: u32,
    marker: Marker,
    line_base: u32,
) {
    let display_line = line_base + local_line;
    // bash's EOF line counter is "one past the last line read", regardless
    // of whether the source ends in a trailing newline (verified against
    // real bash 5.2.21: `bash -c 'if true'` and `bash -c $'if true\n'` both
    // report `line 2`) — so this counts LOGICAL lines via `str::lines()`,
    // not raw `\n` bytes (a byte count under-counts by one for the common
    // no-trailing-newline case).
    let eof_line = line_base + 1 + source.lines().count() as u32;
    let echo_line = source_logical_line(source, local_line);
    match err {
        // Shape 1: a real token is present but misplaced.
        ParseError::Unexpected(f) if matches!(f.found, Found::Token(_)) => {
            let Found::Token(k) = &f.found else {
                unreachable!()
            };
            let tok = spell_token(k);
            emit_syntax_error_ex(
                shell,
                display_line,
                format_args!("syntax error near unexpected token `{tok}'"),
                Some(&echo_line),
                marker,
            );
        }
        // Shape 3: EOF inside an open quote/delimiter.
        ParseError::Unexpected(ExpectFailure {
            found: Found::Eof,
            matching: Some(d),
            ..
        }) if is_matching_delim(*d) => {
            emit_matching(shell, *d, source, local_line, marker, line_base);
        }
        ParseError::Lex(le) => match lex_is_shape3(le) {
            Some(d) => emit_matching(shell, d, source, local_line, marker, line_base),
            None => {
                emit_syntax_error_ex(
                    shell,
                    display_line,
                    format_args!("syntax error: {err}"),
                    None,
                    marker,
                );
            }
        },
        // Shape 2: EOF while a keyword/paren construct is open.
        ParseError::UnterminatedIf
        | ParseError::UnterminatedLoop
        | ParseError::UnterminatedCase
        | ParseError::UnterminatedSubshell
        | ParseError::UnterminatedBrace
        | ParseError::UnterminatedFunction
        | ParseError::Unexpected(ExpectFailure {
            found: Found::Eof, ..
        }) => {
            emit_syntax_error_ex(
                shell,
                eof_line,
                format_args!("syntax error: unexpected end of file"),
                None,
                marker,
            );
        }
        // v316 (#213): a nested backtick command-substitution reparse error.
        // Re-derive the body-local line from `err_pos` (the fresh sub-lexer's
        // cursor into `body`, not `source`) and recurse with `source = body`
        // so the near-token echo quotes the backtick BODY, not the outer
        // line — and with `Marker::CommandSub` so the prologue always reads
        // `command substitution: line N:` regardless of the outer marker.
        ParseError::InCommandSub {
            inner,
            body,
            err_pos,
        } => {
            // The backtick sits at display line `line_base + local_line`; the
            // body numbers from 1, so offset it by that line minus one.
            let comsub_base = line_base + local_line.saturating_sub(1);
            let body_local = 1 + body.as_bytes()[..(*err_pos).min(body.len())]
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u32;
            render_diag_inner(
                shell,
                inner,
                body,
                body_local,
                Marker::CommandSub,
                comsub_base,
            );
        }
        // Fallback: keep the descriptive message (unmigrated / non-top-level).
        other => {
            emit_syntax_error_ex(
                shell,
                display_line,
                format_args!("syntax error: {other}"),
                None,
                marker,
            );
        }
    }
}

/// Emits Shape 3 (`unexpected EOF while looking for matching `X'`) for
/// delimiter `d`. Bash's line number for this shape depends on the
/// delimiter: a quote/`$((`/`${`/backtick is reported at the line the
/// delimiter itself OPENED on — `line_base + local_line`, i.e. the caller's
/// local (body-relative) line re-based onto the display line, same as every
/// other shape (verified against real bash 5.2.21: a multi-line script with
/// `'unterminated` on line 3 reports `line 3:`, not an EOF-relative line).
/// `$(` — an unterminated command substitution — is reported at the EOF
/// line instead (bash keeps scanning to the end of input before giving up
/// on a `$(`; verified the same way: `line 5:` for a 4-physical-line file).
fn emit_matching(
    shell: &Shell,
    d: Delim,
    source: &str,
    local_line: u32,
    marker: Marker,
    line_base: u32,
) {
    let eof_line = line_base + 1 + source.lines().count() as u32;
    let line = match d {
        Delim::DollarParen | Delim::Paren => eof_line,
        _ => line_base + local_line,
    };
    let spelled = spell_delim(d);
    // DBracket renders as `]]`.
    let matchtxt = if matches!(d, Delim::DBracket) {
        "]]".to_string()
    } else {
        spelled.to_string()
    };
    emit_syntax_error_ex(
        shell,
        line,
        format_args!("unexpected EOF while looking for matching `{matchtxt}'"),
        None,
        marker,
    );
}

/// Returns the `line`-th (1-based) logical line of `source`, trailing `\n`
/// stripped (`str::lines()` already excludes it). Out-of-range lines return
/// `""`.
fn source_logical_line(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .unwrap_or("")
        .to_string()
}

/// True for delimiters bash reports as `unexpected EOF while looking for
/// matching `X'` (Shape 3: quotes, backtick, `$(`, `$((`, `${`, `[[`).
/// False for `Paren`/`Brace` — those are keyword/subshell constructs bash
/// reports as the generic `unexpected end of file` (Shape 2) instead.
fn is_matching_delim(d: Delim) -> bool {
    !matches!(d, Delim::Paren | Delim::Brace)
}

/// Maps a lex-level "ran out of input" error to the Shape-3 delimiter it was
/// looking for, if any (`None` for lex errors that aren't an open-delimiter
/// EOF, e.g. `InvalidVarName`).
///
/// v314 Task 5 (#211): `LexError::UnterminatedQuote` carries a `double` flag
/// (set at each lexer raise site) so an unterminated `'…'`/`$'…'` reports
/// `Delim::SQuote` (`` `'' ``) and an unterminated `"…"` reports
/// `Delim::DQuote` (`` `"' ``), matching bash's per-quote-char wording.
fn lex_is_shape3(le: &huck_syntax::lexer::LexError) -> Option<Delim> {
    use huck_syntax::lexer::LexError;
    match le {
        LexError::UnterminatedQuote { double: true } => Some(Delim::DQuote),
        LexError::UnterminatedQuote { double: false } => Some(Delim::SQuote),
        LexError::UnterminatedSubstitution => Some(Delim::DollarParen),
        LexError::UnterminatedArith
        | LexError::UnterminatedLegacyArith
        | LexError::UnterminatedArithBlock => Some(Delim::DollarDParen),
        LexError::UnterminatedBrace => Some(Delim::DollarBrace),
        _ => None,
    }
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
pub fn emit_error_to(
    shell: &Shell,
    w: &mut dyn std::io::Write,
    cmd: Option<&str>,
    body: std::fmt::Arguments,
) {
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

/// Like [`emit_error_to`] but WITHOUT the `line N:` segment — for the bash
/// `redirection_error()` class that bash prints as `<name>: [cmd: ]body` with no
/// line number (e.g. the leading `redirection error: cannot duplicate fd: …`
/// line of a `{var}>&<badfd>` double message). Uses [`Diag::RuntimeNoLine`].
pub fn emit_error_noline_to(
    shell: &Shell,
    w: &mut dyn std::io::Write,
    cmd: Option<&str>,
    body: std::fmt::Arguments,
) {
    let _ = write!(w, "{}", shell.error_prefix(Diag::RuntimeNoLine(cmd)));
    let _ = w.write_fmt(body);
    let _ = w.write_all(b"\n");
}

/// `eprintln!`-shaped wrapper around [`emit_error_noline_to`]:
/// `sh_error_noline_to!(shell, w, cmd, "fmt", args...)`.
#[macro_export]
macro_rules! sh_error_noline_to {
    ($shell:expr, $w:expr, $cmd:expr, $($arg:tt)*) => {
        $crate::emit_error_noline_to($shell, $w, $cmd, format_args!($($arg)*))
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

    /// Builds a non-interactive `-c`-mode `Shell` mirroring
    /// `emit_syntax_error_carries_its_own_line`'s setup, but with
    /// `is_command_string = true` / `shell_argv0 = "huck"` so
    /// `Shell::error_prefix` emits the `-c: ` segment the v314 renderer
    /// shape tests assert against.
    fn shape_test_shell() -> Shell {
        let mut sh = Shell::new();
        sh.is_interactive = false;
        sh.is_command_string = true;
        sh.shell_argv0 = "huck".to_string();
        sh
    }

    #[test]
    fn shape1_near_token_with_echo() {
        use huck_syntax::command::{ExpectFailure, Found, ParseError};
        use huck_syntax::lexer::{Operator, TokenKind};

        let sh = shape_test_shell();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        let e = ParseError::Unexpected(ExpectFailure {
            found: Found::Token(TokenKind::Op(Operator::RParen)),
            matching: None,
            pos: 5,
        });
        install_err_sinks(&mut out, &mut err, || {
            render_syntax_diag(&sh, &e, "echo )", 1);
        });
        assert_eq!(
            buf,
            b"huck: -c: line 1: syntax error near unexpected token `)'\n\
              huck: -c: line 1: `echo )'\n"
                .to_vec()
        );
    }

    #[test]
    fn shape2_unexpected_eof() {
        use huck_syntax::command::ParseError;

        let sh = shape_test_shell();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        install_err_sinks(&mut out, &mut err, || {
            render_syntax_diag(&sh, &ParseError::UnterminatedIf, "if true", 1);
        });
        assert_eq!(
            buf,
            b"huck: -c: line 2: syntax error: unexpected end of file\n".to_vec()
        );
    }

    #[test]
    fn shape3_matching_dquote() {
        use huck_syntax::command::ParseError;
        use huck_syntax::lexer::LexError;

        let sh = shape_test_shell();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        let e = ParseError::Lex(Box::new(LexError::UnterminatedQuote { double: true }));
        install_err_sinks(&mut out, &mut err, || {
            render_syntax_diag(&sh, &e, "echo \"hi", 1);
        });
        assert_eq!(
            buf,
            b"huck: -c: line 1: unexpected EOF while looking for matching `\"'\n".to_vec()
        );
    }

    #[test]
    fn shape3_matching_squote() {
        use huck_syntax::command::ParseError;
        use huck_syntax::lexer::LexError;

        let sh = shape_test_shell();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        let e = ParseError::Lex(Box::new(LexError::UnterminatedQuote { double: false }));
        install_err_sinks(&mut out, &mut err, || {
            render_syntax_diag(&sh, &e, "echo 'hi", 1);
        });
        // Verified against real bash 5.2.21: `bash -c "echo 'hi"` →
        // `unexpected EOF while looking for matching `'''` (the SQuote
        // delimiter char, not DQuote's `"`).
        assert_eq!(
            buf,
            b"huck: -c: line 1: unexpected EOF while looking for matching `''\n".to_vec()
        );
    }

    #[test]
    fn shape3_matching_backtick() {
        use huck_syntax::command::{ExpectFailure, Found, ParseError};

        let sh = shape_test_shell();
        let mut out = StdoutSink::Terminal;
        let mut buf: Vec<u8> = Vec::new();
        let mut err = StderrSink::Capture(&mut buf);
        // Mirrors `parser::unterminated_backtick`'s shape: `Found::Eof` +
        // `matching: Some(Delim::Backtick)`.
        let e = ParseError::Unexpected(ExpectFailure {
            found: Found::Eof,
            matching: Some(Delim::Backtick),
            pos: 9,
        });
        install_err_sinks(&mut out, &mut err, || {
            render_syntax_diag(&sh, &e, "echo `foo", 1);
        });
        // Verified against real bash 5.2.21: `bash -c 'echo `foo'` →
        // `unexpected EOF while looking for matching ```'`.
        assert_eq!(
            buf,
            b"huck: -c: line 1: unexpected EOF while looking for matching `\x60'\n".to_vec()
        );
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
        (
            "completion_builtins.rs",
            include_str!("completion_builtins.rs"),
        ),
        ("shell_state.rs", include_str!("shell_state.rs")),
        ("policy.rs", include_str!("policy.rs")),
        ("shell.rs", include_str!("shell.rs")),
        ("stdin_pipe.rs", include_str!("stdin_pipe.rs")),
        ("history.rs", include_str!("history.rs")),
        ("engine.rs", include_str!("engine.rs")),
        ("cwd_scope.rs", include_str!("cwd_scope.rs")),
        ("error_emit.rs", include_str!("error_emit.rs")),
        (
            "../huck-cli/src/repl.rs",
            include_str!("../../../crates/huck-cli/src/repl.rs"),
        ),
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
                            '{' => {
                                depth += 1;
                                opened = true;
                            }
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
