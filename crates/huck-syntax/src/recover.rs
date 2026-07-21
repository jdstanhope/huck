//! Error-recovery parse: parse a line truncated at the cursor and return a
//! walkable tree plus the cursor context, instead of erroring on the
//! unterminated tail. See docs/superpowers/specs/2026-07-21-parser-error-recovery-design.md.
//!
//! The caller passes `src = line[..cursor]`, so the cursor is EOF. Recovery
//! synthesizes the minimal valid completion of every open construct; the strict
//! `parse()` path is unaffected.

use crate::command::Sequence;

/// An enclosing construct at the cursor. Innermost is LAST in
/// `CursorContext::enclosing`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Frame {
    CommandSub,
    Subshell,
    ArrayLiteral,
    Arith,
    Backtick,
    DoubleQuote,
    SingleQuote,
    ParamExpansion,
    IfCondition,
    WhileCondition,
    ForList,
    CaseSubject,
    BraceGroup,
}

/// What the word at the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WordPosition {
    Command,
    Argument,
    VariableName,
    RedirectTarget,
    AssignRhs,
    Unknown,
}

/// The cursor context, captured at the recovery synthesis boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CursorContext {
    pub enclosing: Vec<Frame>,
    pub position: WordPosition,
    pub word: String,
    pub word_start: usize,
}

/// The result of a recovery parse.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RecoveredParse {
    pub tree: Option<Sequence>,
    pub cursor: CursorContext,
}

/// Parse `src` (a line truncated at the cursor) with EOF-recovery.
pub fn parse_recover(src: &str) -> RecoveredParse {
    // Task 1 skeleton: strict parse, best-effort cursor. Tasks 2-4 activate
    // recovery of incomplete input.
    let tree = crate::parser::parse(src).ok().flatten();
    RecoveredParse {
        tree,
        cursor: CursorContext {
            enclosing: Vec::new(),
            position: WordPosition::Command,
            word: String::new(),
            word_start: src.len(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_complete_input_returns_tree_and_command_cursor() {
        // Complete input parses to a tree; recovery is a no-op here.
        let r = parse_recover("echo hi");
        assert!(r.tree.is_some(), "complete input yields a tree");
    }

    #[test]
    fn recover_empty_input_is_command_position() {
        let r = parse_recover("");
        assert_eq!(r.cursor.position, WordPosition::Command);
        assert_eq!(r.cursor.word, "");
        assert_eq!(r.cursor.word_start, 0);
    }

    #[test]
    fn types_are_non_exhaustive_and_public() {
        // Compile-time surface check.
        let _f: Frame = Frame::CommandSub;
        let _p: WordPosition = WordPosition::Argument;
    }
}
