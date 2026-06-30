//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code, unused_imports)]

use crate::command::ParseError;
use crate::lexer::{Lexer, Mode, TokenKind, Word, WordPart};

/// Assemble a single `WordPart::ParamExpansion` starting at a `${`. Pushes
/// `Mode::ParamExpansion` itself, so the caller passes a lexer positioned at `${`
/// (under any mode — the push ensures `${` is lexed as atoms, not a pre-built Word).
pub(crate) fn parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    let _ = (iter, quoted);
    unimplemented!("parse_param_expansion: Task 4/5")
}

/// Assemble a `Word` (Vec<WordPart>) from atoms in the CURRENT mode, stopping at a
/// boundary atom for that mode (`}` / `ParamSep` / `]`). Used for operands.
pub(crate) fn parse_word(iter: &mut Lexer) -> Result<Word, ParseError> {
    let _ = iter;
    unimplemented!("parse_word: Task 4/5")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolding_types_exist() {
        use crate::lexer::{TokenKind, ParamOpKind, SubstKind, Mode};
        let _ = TokenKind::ParamOpen;
        let _ = TokenKind::Lit { text: "x".into(), quoted: false };
        let _ = ParamOpKind::Substitute(SubstKind::All);
        let _ = Mode::ParamWordOperand { in_dquote: false };
        let _ = crate::command::ParseError::UnsupportedExpansion;
    }
}
