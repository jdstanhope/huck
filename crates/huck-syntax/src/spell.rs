//! Bash spellings for tokens/delimiters named in syntax-error diagnostics.
//!
//! v314 (#211): pure spelling helpers consumed by the engine's error
//! renderer (Task 3+) to produce bash-shaped `near unexpected token '...'`
//! / `unexpected EOF while looking for matching '...'` messages from
//! `command::ExpectFailure`. No error-site migration happens here — see
//! `docs/superpowers/plans/` for the v314 task breakdown.

use crate::lexer::{Operator, TokenKind, Word};

/// The string bash prints inside `near unexpected token `...'`.
///
/// Command words reach a cursor-driven caller (`Lexer::expect_next_kind` /
/// `unexpected_here`, THE atom-driven front end — see `docs/architecture.md`)
/// as a bare `TokenKind::Lit { quoted: false, .. }` atom, NOT as
/// `TokenKind::Word` — the atom-command stream has no `Word` token at command
/// position (mirrors `parser::keyword_of_consumed`'s exact same two-arm
/// match). `TokenKind::Word` is kept as a second arm for the legacy
/// whole-tokenize path (`Lexer::tokenize`) and for callers that assembled a
/// `Word` themselves.
pub fn spell_token(k: &TokenKind) -> String {
    match k {
        TokenKind::Newline => "newline".to_string(),
        TokenKind::Op(op) => spell_op(*op).to_string(),
        TokenKind::Word(w) => reserved_or_word(w),
        TokenKind::Lit {
            text,
            quoted: false,
        } => text.clone(),
        _ => "newline".to_string(), // word-position EOF/other -> bash says `newline`
    }
}

fn spell_op(op: Operator) -> &'static str {
    match op {
        Operator::Pipe => "|",
        Operator::And => "&&",
        Operator::Or => "||",
        Operator::Semi => ";",
        Operator::Background => "&",
        Operator::LParen => "(",
        Operator::RParen => ")",
        Operator::DoubleSemi => ";;",
        Operator::SemiAmp => ";&",
        Operator::DoubleSemiAmp => ";;&",
        Operator::RedirReadWrite => "<>",
        Operator::RedirOut => ">",
        Operator::RedirAppend => ">>",
        Operator::RedirIn => "<",
        _ => "newline",
    }
}

/// A bare reserved word in command position (`done`, `esac`, `fi`, `then`,
/// `do`, `in`, `elif`, `else`) is named by bash literally.
fn reserved_or_word(w: &Word) -> String {
    // Word's flat literal text, if it is a single unquoted literal.
    match w.as_reserved_literal() {
        Some(s) => s.to_string(),
        None => "word".to_string(),
    }
}

pub fn spell_delim(d: crate::command::Delim) -> char {
    use crate::command::Delim::*;
    match d {
        Paren | DollarParen | DollarDParen => ')',
        Brace | DollarBrace => '}',
        DQuote => '"',
        SQuote => '\'',
        Backtick => '`',
        DBracket => ']', // rendered as `]]` by the caller
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_spelled_like_bash() {
        assert_eq!(spell_op(Operator::RParen), ")");
        assert_eq!(spell_op(Operator::DoubleSemi), ";;");
        assert_eq!(spell_op(Operator::Background), "&");
        assert_eq!(spell_op(Operator::Pipe), "|");
    }

    #[test]
    fn newline_token_spelled_newline() {
        assert_eq!(spell_token(&TokenKind::Newline), "newline");
    }

    #[test]
    fn delims_spelled_like_bash() {
        use crate::command::Delim;
        assert_eq!(spell_delim(Delim::DQuote), '"');
        assert_eq!(spell_delim(Delim::Backtick), '`');
        assert_eq!(spell_delim(Delim::DollarParen), ')');
    }

    #[test]
    fn reserved_word_spelled_literally() {
        use crate::lexer::{Word, WordPart};
        let w = Word(vec![WordPart::Literal {
            text: "done".to_string(),
            quoted: false,
        }]);
        assert_eq!(spell_token(&TokenKind::Word(w)), "done".to_string());
    }

    #[test]
    fn plain_word_spelled_literally() {
        // Bash echoes back the literal word text, not just reserved words —
        // `as_reserved_literal` matches on SHAPE (single unquoted literal),
        // not on membership in the keyword set.
        use crate::lexer::{Word, WordPart};
        let w = Word(vec![WordPart::Literal {
            text: "foo".to_string(),
            quoted: false,
        }]);
        assert_eq!(spell_token(&TokenKind::Word(w)), "foo".to_string());
    }

    #[test]
    fn atom_lit_reserved_word_spelled_literally() {
        // The REALISTIC shape a reserved word takes at the atom cursor
        // (`Lexer::expect_next_kind` / `unexpected_here`'s Found::Token
        // payload): a bare unquoted `Lit` atom, not a `Word`. See
        // `parser::keyword_of_consumed` for the same two-shape distinction.
        assert_eq!(
            spell_token(&TokenKind::Lit {
                text: "done".to_string(),
                quoted: false,
            }),
            "done".to_string()
        );
    }

    #[test]
    fn multi_part_word_falls_back_to_word() {
        // A Word with more than one part (e.g. `foo$bar`) has no single
        // flat literal to echo — falls back to the "word" placeholder.
        use crate::lexer::{Word, WordPart};
        let w = Word(vec![
            WordPart::Literal {
                text: "foo".to_string(),
                quoted: false,
            },
            WordPart::Var {
                name: "bar".to_string(),
                quoted: false,
            },
        ]);
        assert_eq!(spell_token(&TokenKind::Word(w)), "word".to_string());
    }
}
