//! Error message rendering for huck-syntax's lex and parse stages.
//!
//! The canonical rendering lives in `lex_error_message_impl` /
//! `parse_error_message_impl` (crate-private). The error types
//! `LexError` / `ParseError` delegate their `Display` impls here.

use crate::command::ParseError;
use crate::lexer::LexError;

pub(crate) fn parse_error_message_impl(error: &ParseError) -> String {
    match error {
        ParseError::MissingCommand => "expected a command".to_string(),
        ParseError::MissingRedirectTarget => "expected a filename after redirection".to_string(),
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection".to_string(),
        ParseError::UnexpectedBackground => "'&' not allowed here".to_string(),
        ParseError::UnterminatedIf => "unterminated 'if' (expected 'then'/'fi')".to_string(),
        ParseError::UnexpectedKeyword(kw) => format!("unexpected '{kw}'"),
        ParseError::UnterminatedLoop => "unterminated loop (expected 'do'/'done')".to_string(),
        ParseError::UnexpectedToken => "unexpected token after command".to_string(),
        ParseError::ForVariable => "invalid variable name in 'for' loop".to_string(),
        ParseError::UnterminatedCase => "unterminated 'case' (expected 'esac')".to_string(),
        ParseError::UnterminatedBrace => "unterminated '{' (expected '}')".to_string(),
        ParseError::FunctionName => "invalid function name".to_string(),
        ParseError::FunctionBody => {
            "function definition: expected '()' and a compound-command body \
             (`if`/`while`/`for`/`case`/`{ … }`)"
                .to_string()
        }
        ParseError::UnterminatedFunction => {
            "unterminated function definition (expected a compound-command body)".to_string()
        }
        ParseError::EmptySubshell => "empty subshell '()' is not allowed".to_string(),
        ParseError::UnterminatedSubshell => "unterminated '(' (expected matching ')')".to_string(),
        ParseError::EmptyDoubleBracket => "'[[ ]]' with empty body is not allowed".to_string(),
        ParseError::UnterminatedDoubleBracket => "unterminated '[[ ]]' (missing ']]')".to_string(),
        ParseError::TestExprBadOperator(op) => {
            format!("unrecognised operator in '[[ ]]': '{op}'")
        }
        ParseError::TestExprMissingOperand => "missing operand in '[[ ]]'".to_string(),
        ParseError::ArithBlock(msg) => {
            format!("arithmetic '((...))': {msg}")
        }
        ParseError::ArithForHeader(msg) => {
            format!("'for ((...))' header: {msg}")
        }
        ParseError::Lex(e) => {
            let s = lex_error_message_impl(e);
            s.strip_prefix(": ").map(|t| t.to_string()).unwrap_or(s)
        }
        ParseError::UnsupportedExpansion => "unsupported expansion".to_string(),
        ParseError::UnsupportedCommand => "unsupported command".to_string(),
        ParseError::Unexpected(_) => "syntax error near unexpected token".to_string(),
    }
}

/// Renders a `LexError` into a message that includes its own leading
/// separator. Most variants start with `": "` so the caller's
/// `"syntax error"` body prefix reads naturally. Substitution-wrapper
/// variants start with `" in command substitution"` (no colon) so the
/// rendered line reads `"syntax error in command substitution: ..."`.
pub(crate) fn lex_error_message_impl(error: &LexError) -> String {
    match error {
        LexError::UnterminatedQuote { .. } => ": unterminated quote".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::UnterminatedArith => ": unterminated arithmetic expansion".to_string(),
        LexError::UnterminatedLegacyArith => {
            ": unterminated '$[' arithmetic expansion (expected ']')".to_string()
        }
        LexError::InvalidBraceModifier(c) => format!(": invalid parameter-expansion modifier: {c}"),
        LexError::EmptyParamName => ": parameter expansion with empty name".to_string(),
        LexError::Substitution(inner) => {
            format!(" in command substitution{}", lex_error_message_impl(inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(
                " in command substitution: {}",
                parse_error_message_impl(inner)
            )
        }
        LexError::UnterminatedHeredoc => ": unterminated here-document".to_string(),
        LexError::BraceExpansionLimit => ": brace expansion: too many elements".to_string(),
        LexError::UnterminatedSubscript => ": missing ']' in subscript".to_string(),
        LexError::UnterminatedArrayLiteral => ": unterminated array literal '('".to_string(),
        LexError::ArrayLiteralMissingEquals => {
            ": array element subscript requires '=' after ']'".to_string()
        }
        LexError::UnterminatedArithBlock => ": unterminated '((' arithmetic block".to_string(),
        LexError::UnterminatedExtglob => ": unterminated extglob group".to_string(),
        LexError::NoProgress => ": lexer made no forward progress".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use crate::brace_expand::BraceError;
    use crate::command::ParseError;
    use crate::lexer::LexError;

    #[test]
    fn parse_error_lex_strips_leading_colon_space() {
        // LexError::UnterminatedQuote renders as ": unterminated quote" via
        // Display. When wrapped in ParseError::Lex, ParseError's Display
        // must strip the leading ": " so callers using "syntax error: {}"
        // produce a single separator, not a double one.
        let err = ParseError::Lex(Box::new(LexError::UnterminatedQuote { double: true }));
        let msg = err.to_string();
        assert!(
            !msg.starts_with(": "),
            "ParseError::Lex's Display should NOT start with \": \", got: {msg:?}"
        );
        assert_eq!(msg, "unterminated quote");
    }

    #[test]
    fn brace_error_implements_display_and_error() {
        fn assert_traits<E: std::fmt::Display + std::error::Error>(_e: &E) {}
        let err = BraceError::TooManyElements;
        assert_traits(&err);
        // Sanity: Display produces a non-empty message.
        assert!(!format!("{err}").is_empty());
    }
}
