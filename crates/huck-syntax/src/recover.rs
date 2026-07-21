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
    // Drive a recovery lexer (`recover_at_eof`): at genuine EOF with open lexer
    // modes it emits each frame's synthetic close atom (innermost-out), so the
    // parser recovers the nesting constructs instead of erroring on the
    // unterminated tail. The strict `parse()` path is unaffected (the option
    // defaults `false`). Cursor context is still the Task-1 best-effort stub;
    // Tasks 3-4 populate it.
    let opts = crate::lexer::LexerOptions {
        recover_at_eof: true,
        ..Default::default()
    };
    let mut lx = crate::lexer::Lexer::new(src, &Default::default(), opts);
    let tree = crate::parser::parse_sequence(&mut lx).ok().flatten();
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
    fn recover_unterminated_cmdsub_yields_tree() {
        // `echo $(whi` — recovery closes the `$(` so the whole thing parses.
        let r = parse_recover("echo $(whi");
        assert!(r.tree.is_some(), "unterminated $( should recover to a tree");
    }

    #[test]
    fn recover_cmdsub_in_double_quotes_yields_tree() {
        let r = parse_recover("echo \"$(whi");
        assert!(r.tree.is_some(), "quoted unterminated $( should recover");
    }

    #[test]
    fn recover_unterminated_param_expansion_yields_tree() {
        let r = parse_recover("echo ${whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_unterminated_arith_yields_tree() {
        let r = parse_recover("echo $(( x + ");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_unterminated_backtick_yields_tree() {
        let r = parse_recover("echo `whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_unterminated_double_quote_yields_tree() {
        let r = parse_recover("echo \"hello");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_never_panics_on_truncated_param_expansion() {
        // Half-typed ${…} with an invalid/absent separator must recover, not crash.
        // Regression: the bad-substitution reconstruction slices `${…}` inclusive to
        // the close atom's offset; under recover_at_eof the synthetic closer sits at
        // EOF, so an inclusive `..=len` panicked out of bounds.
        for s in [
            "echo ${baz ",
            "echo ${x@",
            "echo ${x@Z",
            "echo ${V!",
            "echo ${-3",
            "echo ${x;",
            "echo ${x =",
            "echo ${",
            "echo ${x",
        ] {
            let _ = parse_recover(s); // must return without panicking
        }
        // And the reconstruction actually recovers a tree rather than bailing.
        assert!(parse_recover("echo ${baz ").tree.is_some());
    }

    #[test]
    fn recover_if_without_then_yields_tree() {
        let r = parse_recover("if whi");
        assert!(
            r.tree.is_some(),
            "`if COND` should recover (synthesize then/fi)"
        );
    }

    #[test]
    fn recover_while_without_do_yields_tree() {
        let r = parse_recover("while whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_for_in_without_do_yields_tree() {
        let r = parse_recover("for x in whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_case_without_esac_yields_tree() {
        let r = parse_recover("case whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_brace_group_yields_tree() {
        let r = parse_recover("{ whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn recover_subshell_yields_tree() {
        let r = parse_recover("( whi");
        assert!(r.tree.is_some());
    }

    #[test]
    fn types_are_non_exhaustive_and_public() {
        // Compile-time surface check.
        let _f: Frame = Frame::CommandSub;
        let _p: WordPosition = WordPosition::Argument;
    }
}
