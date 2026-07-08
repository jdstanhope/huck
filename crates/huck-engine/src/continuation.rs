//! Decides whether a typed buffer forms a complete command, needs more
//! input (and why), or is a genuine syntax error. Pure — it runs the
//! real lexer and parser and classifies the outcome, so it can never
//! disagree with them.

use crate::command::ParseError;
use crate::lexer::{self, ends_with_continuation_backslash, LexError, Operator, TokenKind};
use crate::parser;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ContinuationReason {
    Backslash,
    OpenQuote,
    Operator,
    Compound,
    Heredoc,
    Subshell,
    DoubleBracket,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Completeness {
    Complete,
    Incomplete(ContinuationReason),
    Error,
}

/// The lexer errors that mean "a quote or expansion is still open" — as
/// opposed to a malformed-but-closed construct.
fn is_unterminated_lex(e: &LexError) -> bool {
    matches!(
        e,
        LexError::UnterminatedQuote
            | LexError::UnterminatedBrace
            | LexError::UnterminatedSubstitution
            | LexError::UnterminatedArith
            | LexError::UnterminatedLegacyArith
            | LexError::UnterminatedArithBlock
            | LexError::UnterminatedExtglob
            | LexError::UnterminatedArrayLiteral
    )
}

/// Classifies `buffer`. See module docs. `extglob` is the shell's
/// `shopt -s extglob` state, threaded into the lexer so a line-broken
/// extglob group (`+(a|`) requests continuation.
pub fn classify(buffer: &str, extglob: bool) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let opts = lexer::LexerOptions { extglob, ..Default::default() };
    let empty = std::collections::HashMap::new();
    let mut lx = lexer::Lexer::new_live_atoms(buffer, &empty, opts);
    let parsed = parser::parse_sequence(&mut lx);

    // Lex-level incompleteness short-circuits everything else, mirroring the old
    // two-pass model where a failed `tokenize` returned BEFORE the parser or the
    // trailing-connector check ever ran. Under the fused atom path these surface
    // as `ParseError::Lex`.
    if let Err(ParseError::Lex(e)) = &parsed {
        if matches!(**e, LexError::UnterminatedHeredoc) {
            return Completeness::Incomplete(ContinuationReason::Heredoc);
        }
        return if is_unterminated_lex(e) {
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        } else {
            Completeness::Error
        };
    }

    // A heredoc redirect whose body was never supplied — the buffer ends on the
    // redirect LINE (`cat <<EOF` with no following newline). The atom parser
    // returns Ok for this (a complete empty-body command, matching bash SCRIPT
    // mode), where the old whole-buffer `tokenize` returned UnterminatedHeredoc.
    // Interactively the REPL must keep reading the body, so surface it as an
    // incomplete heredoc — mirroring bash's `>` prompt (see
    // `Lexer::has_unattached_heredoc`).
    if lx.has_unattached_heredoc() {
        return Completeness::Incomplete(ContinuationReason::Heredoc);
    }

    // An unterminated `[[ … ]]` is detected before the trailing-connector check
    // so a buffer ending `&&`/`||` INSIDE the brackets is still a double-bracket
    // continuation, not an operator one (inside `[[ … ]]` the `&&`/`||` are part
    // of the conditional expression, so the parser — not the connector check —
    // owns them).
    if let Err(ParseError::UnterminatedDoubleBracket) = parsed {
        return Completeness::Incomplete(ContinuationReason::DoubleBracket);
    }

    // Trailing `|`/`&&`/`||` → the line continues (old `tokens.last()` check).
    // Read the connector from the token history the parser ALREADY drove — see
    // `Lexer::last_significant_kind`. Do NOT re-drive a fresh standalone lexer:
    // the atom lexer emits zero-width opener signals (`$((`, `$(`, `${`, `` ` ``)
    // that only the parser consumes, so a parser-less scan spins forever on the
    // first one (unbounded `history` growth → OOM).
    //
    // The connector must be the buffer's genuine LAST token, so also require the
    // lexer to be at EOF (`peek_kind() == None`). Otherwise a connector followed
    // by more input that failed to parse — `echo hi | | grep x` — would look
    // trailing (the parser stops at the second `|`, peeked-but-unconsumed, so it
    // is `last_significant`) and be misread as a continuation instead of the
    // genuine syntax Error it is. `peek_kind` here runs at most one `scan_step`
    // (bounded, no spin): it returns the already-buffered lookahead when the
    // parser stopped mid-buffer, or `None` at true EOF.
    if matches!(
        lx.last_significant_kind(),
        Some(TokenKind::Op(Operator::Pipe | Operator::And | Operator::Or))
    ) && matches!(lx.peek_kind(), Ok(None))
    {
        return Completeness::Incomplete(ContinuationReason::Operator);
    }

    match parsed {
        Ok(_) => Completeness::Complete,
        Err(ParseError::UnterminatedSubshell) => {
            Completeness::Incomplete(ContinuationReason::Subshell)
        }
        Err(ParseError::UnterminatedIf
            | ParseError::UnterminatedLoop
            | ParseError::UnterminatedCase
            | ParseError::UnterminatedBrace
            | ParseError::UnterminatedFunction) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
        Err(_) => Completeness::Error,
    }
}

/// True when `line`'s last whitespace-delimited word is a control
/// keyword after which a `;` would be invalid.
fn ends_with_control_keyword(line: &str) -> bool {
    matches!(
        line.split_whitespace().next_back(),
        Some("if" | "while" | "until" | "then" | "do" | "else" | "elif" | "for" | "select" | "in" | "case" | "{")
    )
}

/// The separator to splice before a continuation line when collapsing a
/// multi-line command into its single-line history form. `last_line` is
/// the line that triggered the continuation.
pub fn joiner_for(reason: ContinuationReason, last_line: &str) -> &'static str {
    match reason {
        ContinuationReason::Backslash => "",
        ContinuationReason::Operator => " ",
        ContinuationReason::OpenQuote => "; ",
        ContinuationReason::Compound => {
            if ends_with_control_keyword(last_line) {
                " "
            } else {
                "; "
            }
        }
        ContinuationReason::Heredoc => "\n",
        ContinuationReason::Subshell => "; ",
        // Unconditional space: `[[ ]]` has no keyword positions where a `;`
        // would be needed (unlike `Compound`), and a space is valid in every
        // bash-allowed break position (after `[[`, after `&&`/`||`, before `]]`).
        ContinuationReason::DoubleBracket => " ",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_simple_command() {
        assert_eq!(classify("echo hi", false), Completeness::Complete);
    }

    #[test]
    fn complete_multiline_if() {
        assert_eq!(classify("if true\nthen echo hi\nfi", false), Completeness::Complete);
    }

    #[test]
    fn empty_buffer_is_complete() {
        assert_eq!(classify("", false), Completeness::Complete);
    }

    #[test]
    fn open_double_quote_is_incomplete() {
        assert_eq!(
            classify("echo \"hello", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_command_substitution_is_incomplete() {
        // v265: the atom parser drives INTO the `$(` command sub and hits EOF,
        // reporting the unterminated construct structurally as
        // `UnterminatedSubshell` → `Subshell`, where the old oracle surfaced a
        // lexer `UnterminatedSubstitution` → `OpenQuote`. Both are Incomplete and
        // both use the same "; " continuation joiner (see `joiner_for`), so the
        // REPL behavior is identical — only the reason variant changed.
        assert_eq!(
            classify("echo $(date", false),
            Completeness::Incomplete(ContinuationReason::Subshell)
        );
    }

    #[test]
    fn open_array_literal_is_incomplete() {
        // v183: `a=(1 2` — the array literal `(` isn't closed yet. Classify as
        // Incomplete (continuation), not Error, so the REPL / piped stdin keeps
        // reading until `)`. Regression: is_unterminated_lex omitted
        // UnterminatedArrayLiteral, so multi-line arrays mis-parsed.
        assert_eq!(
            classify("a=(1 2", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_brace_expansion_is_incomplete() {
        assert_eq!(
            classify("echo ${FOO", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_arithmetic_expansion_is_incomplete() {
        assert_eq!(
            classify("echo $((1 + 2", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn unterminated_arith_block_requests_more_input() {
        // `((1+2` — no closing `))`. v265: the atom parser speculatively parses
        // the command-position `((` as an arithmetic command (`parse_arith_command`)
        // and hits EOF INSIDE the arith body — with no depth-0 `)` there is no
        // `ArithBail`, so the `( (` nested-subshell backoff never fires; the scan
        // returns `UnterminatedArith` → `OpenQuote`. The old oracle instead
        // eagerly fell back to `( (` and reported `UnterminatedSubshell` →
        // `Subshell`. Both are Incomplete and share the same "; " continuation
        // joiner (see `joiner_for`), so the REPL still prompts `>` for the unclosed
        // parens — only the reason variant changed.
        assert_eq!(
            classify("((1+2", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn unterminated_legacy_arith_requests_more_input() {
        // `$[ 1 +` — no closing `]`. The lexer signals UnterminatedLegacyArith,
        // which is_unterminated_lex treats as incomplete so the REPL prompts for
        // continuation (via the generic OpenQuote reason, like other unterminated
        // lex spans).
        assert_eq!(
            classify("echo $[ 1 +", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn unterminated_arith_for_header_requests_more_input() {
        // `for ((;;` — the arith-for header isn't closed yet. As of v184 an
        // unterminated `((` no longer lex-errors; it falls back to two LParens,
        // so the parser sees `for ( (` and reports an unclosed arith-for header
        // as UnterminatedLoop → Incomplete(Compound). Still Incomplete (the REPL
        // prompts for continuation), matching bash which prompts `>` here.
        assert_eq!(
            classify("for ((;;", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn trailing_pipe_is_incomplete() {
        assert_eq!(
            classify("echo hi |", false),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn trailing_andand_is_incomplete() {
        assert_eq!(
            classify("echo hi &&", false),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn trailing_oror_is_incomplete() {
        assert_eq!(
            classify("echo hi ||", false),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn unterminated_if_is_incomplete() {
        assert_eq!(
            classify("if true", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_while_is_incomplete() {
        assert_eq!(
            classify("while true\ndo echo hi", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_for_is_incomplete() {
        assert_eq!(
            classify("for x in a b c", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_until_is_incomplete() {
        assert_eq!(
            classify("until false\ndo echo hi", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn if_awaiting_body_is_incomplete() {
        assert_eq!(
            classify("if true\nthen", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn trailing_backslash_is_incomplete() {
        assert_eq!(
            classify("echo hi \\", false),
            Completeness::Incomplete(ContinuationReason::Backslash)
        );
    }

    #[test]
    fn even_trailing_backslashes_are_not_a_continuation() {
        // `\\` is an escaped backslash — the line is complete.
        assert_eq!(classify("echo hi\\\\", false), Completeness::Complete);
    }

    #[test]
    fn genuine_syntax_error_is_error() {
        // A doubled `|` is a parser error, not an incompletion.
        assert_eq!(classify("echo hi | | grep x", false), Completeness::Error);
    }

    #[test]
    fn stray_word_after_fi_is_error() {
        assert_eq!(classify("if true; then echo; fi extra", false), Completeness::Error);
    }

    #[test]
    fn joiner_backslash_is_empty() {
        assert_eq!(joiner_for(ContinuationReason::Backslash, "echo a"), "");
    }

    #[test]
    fn joiner_operator_is_space() {
        assert_eq!(joiner_for(ContinuationReason::Operator, "echo a |"), " ");
    }

    #[test]
    fn joiner_open_quote_is_semicolon() {
        assert_eq!(joiner_for(ContinuationReason::OpenQuote, "echo \"a"), "; ");
    }

    #[test]
    fn joiner_compound_is_semicolon_after_a_command() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "if true"), "; ");
    }

    #[test]
    fn joiner_compound_is_space_after_a_bare_keyword() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "then"), " ");
        assert_eq!(joiner_for(ContinuationReason::Compound, "  do  "), " ");
    }

    #[test]
    fn joiner_compound_is_space_after_for_keyword() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "for"), " ");
        assert_eq!(joiner_for(ContinuationReason::Compound, "for x in"), " ");
    }

    #[test]
    fn unterminated_case_is_incomplete() {
        assert_eq!(
            classify("case x in a) echo hi", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn joiner_compound_is_space_after_case_keyword() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "case"), " ");
    }

    #[test]
    fn unterminated_brace_is_incomplete() {
        assert_eq!(
            classify("{ echo hi", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn complete_brace_group_is_complete() {
        assert_eq!(classify("{ echo hi; }", false), Completeness::Complete);
    }

    #[test]
    fn joiner_compound_is_space_after_open_brace() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "{"), " ");
    }

    #[test]
    fn unterminated_function_def_is_incomplete() {
        assert_eq!(
            classify("foo()", false),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn classify_heredoc_unclosed_is_incomplete() {
        assert_eq!(
            classify("cat <<EOF\nhello", false),
            Completeness::Incomplete(ContinuationReason::Heredoc)
        );
    }

    #[test]
    fn classify_heredoc_open_at_eof_still_incomplete() {
        // Invariant (b) of the EOF-closes-heredoc work: `classify` builds its
        // lexer with `eof_closes_heredoc=false` (the default), so an open
        // here-document at end-of-input STILL reports Incomplete(Heredoc) — the
        // interactive REPL must keep prompting (PS2), never terminate the body at
        // a line boundary. Only the top-level BATCH parse closes it at EOF.
        assert_eq!(
            classify("cat <<EOF\nhi", false),
            Completeness::Incomplete(ContinuationReason::Heredoc)
        );
    }

    #[test]
    fn classify_heredoc_bare_first_line_is_incomplete() {
        // The REPL feeds one physical line at a time, so the FIRST classify call
        // for `cat <<EOF\n…\nEOF` is on the bare redirect line `cat <<EOF` — no
        // newline, no body yet. This MUST report Incomplete(Heredoc) or the REPL
        // executes `cat <<EOF` alone and runs the body lines as commands.
        assert_eq!(
            classify("cat <<EOF", false),
            Completeness::Incomplete(ContinuationReason::Heredoc)
        );
    }

    #[test]
    fn classify_heredoc_closed_is_complete() {
        assert_eq!(
            classify("cat <<EOF\nhello\nEOF\n", false),
            Completeness::Complete
        );
    }

    #[test]
    fn joiner_for_heredoc_is_newline() {
        assert_eq!(joiner_for(ContinuationReason::Heredoc, ""), "\n");
    }

    #[test]
    fn classify_subshell_unclosed_is_incomplete() {
        assert_eq!(
            classify("(echo hi", false),
            Completeness::Incomplete(ContinuationReason::Subshell)
        );
    }

    #[test]
    fn classify_subshell_closed_is_complete() {
        assert_eq!(classify("(echo hi)", false), Completeness::Complete);
    }

    #[test]
    fn joiner_for_subshell_is_semi_space() {
        assert_eq!(joiner_for(ContinuationReason::Subshell, ""), "; ");
    }

    #[test]
    fn classify_unclosed_double_bracket_is_incomplete() {
        assert_eq!(
            classify("[[ -f /etc/passwd", false),
            Completeness::Incomplete(ContinuationReason::DoubleBracket)
        );
    }

    #[test]
    fn classify_double_bracket_trailing_and_is_incomplete() {
        assert_eq!(
            classify("[[ -f /a &&", false),
            Completeness::Incomplete(ContinuationReason::DoubleBracket)
        );
    }

    #[test]
    fn classify_closed_double_bracket_is_complete() {
        assert_eq!(classify("[[ a == b ]]", false), Completeness::Complete);
    }

    #[test]
    fn classify_double_bracket_missing_operand_is_error() {
        // `]]` present, operand absent → genuine error; must NOT request continuation.
        assert_eq!(classify("[[ a == ]]", false), Completeness::Error);
    }

    #[test]
    fn classify_bare_double_bracket_token_is_complete() {
        // `echo [[` — `[[` is an ordinary argument, not a conditional opener.
        assert_eq!(classify("echo [[", false), Completeness::Complete);
    }

    #[test]
    fn joiner_for_double_bracket_is_space() {
        assert_eq!(joiner_for(ContinuationReason::DoubleBracket, "[[ -f a &&"), " ");
    }
}
