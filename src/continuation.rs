//! Decides whether a typed buffer forms a complete command, needs more
//! input (and why), or is a genuine syntax error. Pure — it runs the
//! real lexer and parser and classifies the outcome, so it can never
//! disagree with them.

use crate::command::{self, ParseError};
use crate::lexer::{self, LexError, Operator, Token};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ContinuationReason {
    Backslash,
    OpenQuote,
    Operator,
    Compound,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Completeness {
    Complete,
    Incomplete(ContinuationReason),
    Error,
}

/// True when `s` ends with an odd-length run of backslashes — the final
/// backslash is an unescaped line-continuation marker.
fn ends_with_continuation_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
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
    )
}

/// Classifies `buffer`. See module docs.
pub fn classify(buffer: &str) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let tokens = match lexer::tokenize(buffer) {
        Ok(tokens) => tokens,
        Err(e) => {
            return if is_unterminated_lex(&e) {
                Completeness::Incomplete(ContinuationReason::OpenQuote)
            } else {
                Completeness::Error
            };
        }
    };
    if matches!(
        tokens.last(),
        Some(Token::Op(Operator::Pipe | Operator::And | Operator::Or))
    ) {
        return Completeness::Incomplete(ContinuationReason::Operator);
    }
    match command::parse(tokens) {
        Ok(_) => Completeness::Complete,
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
        Some("if" | "while" | "until" | "then" | "do" | "else" | "elif" | "for" | "in" | "case" | "{")
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_simple_command() {
        assert_eq!(classify("echo hi"), Completeness::Complete);
    }

    #[test]
    fn complete_multiline_if() {
        assert_eq!(classify("if true\nthen echo hi\nfi"), Completeness::Complete);
    }

    #[test]
    fn empty_buffer_is_complete() {
        assert_eq!(classify(""), Completeness::Complete);
    }

    #[test]
    fn open_double_quote_is_incomplete() {
        assert_eq!(
            classify("echo \"hello"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_command_substitution_is_incomplete() {
        assert_eq!(
            classify("echo $(date"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_brace_expansion_is_incomplete() {
        assert_eq!(
            classify("echo ${FOO"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_arithmetic_expansion_is_incomplete() {
        assert_eq!(
            classify("echo $((1 + 2"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn trailing_pipe_is_incomplete() {
        assert_eq!(
            classify("echo hi |"),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn trailing_andand_is_incomplete() {
        assert_eq!(
            classify("echo hi &&"),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn trailing_oror_is_incomplete() {
        assert_eq!(
            classify("echo hi ||"),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn unterminated_if_is_incomplete() {
        assert_eq!(
            classify("if true"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_while_is_incomplete() {
        assert_eq!(
            classify("while true\ndo echo hi"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_for_is_incomplete() {
        assert_eq!(
            classify("for x in a b c"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_until_is_incomplete() {
        assert_eq!(
            classify("until false\ndo echo hi"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn if_awaiting_body_is_incomplete() {
        assert_eq!(
            classify("if true\nthen"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn trailing_backslash_is_incomplete() {
        assert_eq!(
            classify("echo hi \\"),
            Completeness::Incomplete(ContinuationReason::Backslash)
        );
    }

    #[test]
    fn even_trailing_backslashes_are_not_a_continuation() {
        // `\\` is an escaped backslash — the line is complete.
        assert_eq!(classify("echo hi\\\\"), Completeness::Complete);
    }

    #[test]
    fn genuine_syntax_error_is_error() {
        // A doubled `|` is a parser error, not an incompletion.
        assert_eq!(classify("echo hi | | grep x"), Completeness::Error);
    }

    #[test]
    fn stray_word_after_fi_is_error() {
        assert_eq!(classify("if true; then echo; fi extra"), Completeness::Error);
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
            classify("case x in a) echo hi"),
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
            classify("{ echo hi"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn complete_brace_group_is_complete() {
        assert_eq!(classify("{ echo hi; }"), Completeness::Complete);
    }

    #[test]
    fn joiner_compound_is_space_after_open_brace() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "{"), " ");
    }

    #[test]
    fn unterminated_function_def_is_incomplete() {
        assert_eq!(
            classify("foo()"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }
}
