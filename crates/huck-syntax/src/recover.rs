//! Error-recovery parse: parse a line truncated at the cursor and return a
//! walkable tree plus the cursor context, instead of erroring on the
//! unterminated tail. See docs/superpowers/specs/2026-07-21-parser-error-recovery-design.md.
//!
//! The caller passes `src = line[..cursor]`, so the cursor is EOF. Recovery
//! synthesizes the minimal valid completion of every open construct; the strict
//! `parse()` path is unaffected.

use crate::command::Sequence;
use crate::lexer::Mode;

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
    /// A bare redirect operand (`cat foo > whi`) — the LOWEST-priority position,
    /// reported in place of `Argument` (iteration 2). An inner expansion mode at
    /// the cursor (`> $HOM`, `> ${HOM`, `> $(whi`, `> $(( HO`) wins instead.
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

/// Map a lexer `Mode` to the enclosing `Frame` it represents at the cursor, or
/// `None` for modes with no cursor-context meaning (the `Command` floor, and the
/// internal `Regex`/`Extglob` scanners). The `${…}` operand sub-modes all fold
/// back to `ParamExpansion` — from the cursor's view they are all "inside `${`".
fn mode_to_frame(mode: &Mode) -> Option<Frame> {
    match mode {
        // `$( … )` / `<( … )` / `>( … )` bodies (the parser also opens this mode
        // for a bare `( … )` process-sub body); from the cursor's view all are a
        // command substitution / paren-command.
        Mode::CommandSub { .. } => Some(Frame::CommandSub),
        Mode::BacktickRaw => Some(Frame::Backtick),
        Mode::ParamExpansion { .. }
        | Mode::ParamWordOperand { .. }
        | Mode::ParamSubstPatternOperand { .. }
        | Mode::ParamSubstringOffsetOperand { .. }
        | Mode::ParamSubscriptOperand { .. } => Some(Frame::ParamExpansion),
        Mode::Arith { .. } => Some(Frame::Arith),
        Mode::DoubleQuote { .. } => Some(Frame::DoubleQuote),
        Mode::ArrayLiteral { .. } => Some(Frame::ArrayLiteral),
        // Not cursor-context frames.
        Mode::Command | Mode::Regex { .. } | Mode::Extglob { .. } => None,
    }
}

/// Compute the `WordPosition` of the cursor word from the captured lexer state.
/// Priority: a `${…}`/`$((…))` name context, a `$name` word, and a lone
/// trailing `$` (no name typed yet, e.g. `echo $`) all mean `VariableName`; an
/// array literal RHS (`x=(…`) is `AssignRhs`; otherwise the parser's
/// command-vs-argument flag decides.
fn position_from(
    modes: &[Mode],
    last_is_dollar_name: bool,
    trailing_lone_dollar: bool,
    cmd_word: bool,
    redirect_target: bool,
) -> WordPosition {
    // The innermost non-`Command` mode drives the mode-based positions.
    let innermost = modes.iter().rev().find(|m| !matches!(m, Mode::Command));
    match innermost {
        // Inside `${` (scanning the name) or `$((`/`$[` (an arith operand) — the
        // cursor sits in a variable name.
        Some(Mode::ParamExpansion { .. }) | Some(Mode::Arith { .. }) => WordPosition::VariableName,
        // `x=(whi` — an array literal element, an assignment RHS (NOT a command).
        Some(Mode::ArrayLiteral { .. }) => WordPosition::AssignRhs,
        // A backtick body is raw-captured by the outer lexer as a single opaque
        // `BacktickRawText` run (a separate nested lexer re-parses it only later),
        // so the outer `cmd_word` flag reflects the ENCLOSING word, not the
        // backtick's content. Iteration-1 limitation: because the body is one raw
        // atom, `word` captures the WHOLE backtick body verbatim and the position
        // is coarsely `Command` even when the cursor is really inside a variable
        // reference or an argument within the backticks; distinguishing those
        // needs re-lexing the raw body and is left to iteration 2 (cf. ba38434).
        Some(Mode::BacktickRaw) => WordPosition::Command,
        _ => {
            if last_is_dollar_name || trailing_lone_dollar {
                // A bare `$name` at the cursor (e.g. `echo $whi`), or a lone
                // trailing `$` with no name yet (`echo $`) — the degenerate
                // empty-name case, parallel to `${`.
                WordPosition::VariableName
            } else if cmd_word {
                WordPosition::Command
            } else if redirect_target {
                // A bare redirect operand with no inner expansion mode
                // (`cat foo > whi`). LOWEST priority — in place of `Argument`,
                // per bash 5.2.21. An inner `$name`/`${`/`$(`/`$((` above wins.
                WordPosition::RedirectTarget
            } else {
                WordPosition::Argument
            }
        }
    }
}

/// Merge the two frame sources into one `enclosing` list ordered innermost-LAST.
///
/// `modes` is the lexer mode stack at EOF (outermost first, `Command` floor at
/// index 0). `compounds` are the parser's compound frames, each tagged with the
/// mode-stack depth (`self.modes.len()`) live when it was pushed — i.e. the depth
/// at which the compound is nested. A compound tagged `e` encloses every lexer
/// mode at stack index `>= e` and is enclosed by those at index `< e`, so it
/// belongs in the output just before the first mode frame at index `>= e`. This
/// single depth key interleaves the two sources correctly in all four nesting
/// combinations (mode-in-compound `if echo $(whi` → `[IfCondition, CommandSub]`,
/// compound-in-mode `echo $(if whi` → `[CommandSub, IfCondition]`, and the
/// mode-in-mode / compound-in-compound cases). Among compounds sharing a depth
/// (nested compounds with no intervening lexer mode, e.g. `if while whi`), the
/// later-pushed one is the OUTER one (frames are pushed inner-first as the parser
/// unwinds), so equal depths are ordered by descending push index.
fn merge_enclosing(modes: &[Mode], compounds: &[(usize, Frame)]) -> Vec<Frame> {
    // (depth, push_index, frame), sorted by depth asc then push index desc.
    let mut comps: Vec<(usize, usize, Frame)> = compounds
        .iter()
        .enumerate()
        .map(|(idx, (depth, frame))| (*depth, idx, frame.clone()))
        .collect();
    comps.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));

    let mut out = Vec::new();
    let mut ci = 0;
    // Walk the mode stack (skip the `Command` floor at index 0). Before emitting
    // the frame for mode index `i`, flush every compound whose depth `<= i` (its
    // nesting point is at or above this mode).
    for (i, mode) in modes.iter().enumerate() {
        if i == 0 {
            continue;
        }
        while ci < comps.len() && comps[ci].0 <= i {
            out.push(comps[ci].2.clone());
            ci += 1;
        }
        if let Some(frame) = mode_to_frame(mode) {
            out.push(frame);
        }
    }
    // Any compound nested deeper than the deepest mode frame trails at the end.
    while ci < comps.len() {
        out.push(comps[ci].2.clone());
        ci += 1;
    }
    out
}

/// Parse `src` (a line truncated at the cursor) with EOF-recovery.
pub fn parse_recover(src: &str) -> RecoveredParse {
    // Drive a recovery lexer (`recover_at_eof`): at genuine EOF with open lexer
    // modes it emits each frame's synthetic close atom (innermost-out), so the
    // parser recovers the nesting constructs instead of erroring on the
    // unterminated tail. The strict `parse()` path is unaffected (the option
    // defaults `false`).
    let opts = crate::lexer::LexerOptions {
        recover_at_eof: true,
        ..Default::default()
    };
    let mut lx = crate::lexer::Lexer::new(src, &Default::default(), opts);
    let tree = crate::parser::parse_sequence(&mut lx).ok().flatten();

    // Assemble the cursor context from state captured at the synthesis boundary
    // (real EOF) — NOT by walking `tree`, which may be `None` even though the
    // cursor context is well-defined (e.g. `case $x in a`). The lexer snapshot is
    // taken before frames are popped; the parser's compound frames are merged into
    // the mode-derived ones by nesting depth (see `merge_enclosing`).
    let cursor = match lx.recovery_capture() {
        Some(cap) => {
            let enclosing = merge_enclosing(&cap.modes, lx.recovery_frames());
            CursorContext {
                enclosing,
                position: position_from(
                    &cap.modes,
                    cap.last_is_dollar_name,
                    cap.trailing_lone_dollar,
                    cap.cmd_word,
                    cap.redirect_target,
                ),
                word: cap.word.clone(),
                word_start: cap.word_start,
            }
        }
        // EOF was never reached under recovery (defensive — the parser peeks at
        // EOF on essentially every input). Fall back to a fresh command boundary.
        None => CursorContext {
            enclosing: Vec::new(),
            position: WordPosition::Command,
            word: String::new(),
            word_start: src.len(),
        },
    };

    RecoveredParse { tree, cursor }
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

    fn ctx(src: &str) -> CursorContext {
        parse_recover(src).cursor
    }

    #[test]
    fn cursor_command_position_cases() {
        for src in [
            "whi",
            "if whi",
            "while whi",
            "echo $(whi",
            "echo `whi",
            "(whi",
            "echo <(whi",
        ] {
            assert_eq!(ctx(src).position, WordPosition::Command, "{src:?}");
        }
    }

    #[test]
    fn cursor_argument_position_cases() {
        for src in ["echo whi", "for x in whi", "ls -l whi"] {
            assert_eq!(ctx(src).position, WordPosition::Argument, "{src:?}");
        }
    }

    #[test]
    fn cursor_variable_position_cases() {
        assert_eq!(ctx("echo ${whi").position, WordPosition::VariableName);
        assert_eq!(ctx("echo $whi").position, WordPosition::VariableName);
        assert_eq!(ctx("echo $(( whi").position, WordPosition::VariableName);
    }

    #[test]
    fn cursor_bare_redirect_target_is_redirect_target() {
        for src in [
            "cat foo > whi",
            "echo >whi",
            "cat < whi",
            "echo 2> whi",
            "echo >> whi",
        ] {
            assert_eq!(
                parse_recover(src).cursor.position,
                WordPosition::RedirectTarget,
                "{src:?}"
            );
        }
    }

    #[test]
    fn cursor_redirect_target_with_inner_expansion_keeps_inner_position() {
        // The inner expansion wins — a redirect target is a WORD.
        assert_eq!(
            parse_recover("cat foo > $HOM").cursor.position,
            WordPosition::VariableName
        );
        assert_eq!(
            parse_recover("cat foo > ${HOM").cursor.position,
            WordPosition::VariableName
        );
        assert_eq!(
            parse_recover("cat foo > $(whi").cursor.position,
            WordPosition::Command
        );
        assert_eq!(
            parse_recover("cat foo > $(( HO").cursor.position,
            WordPosition::VariableName
        );
    }

    #[test]
    fn cursor_empty_trailing_word_reports_expected_next_slot() {
        // Gap A (iteration 2): at a whitespace boundary with an EMPTY cursor word,
        // the position reflects the grammar slot the NEXT word would occupy, not
        // the last consumed word's slot.
        // After a command word → the next word is an argument.
        for src in ["echo ", "ls ", "echo hi ", "for x in "] {
            let c = ctx(src);
            assert_eq!(c.position, WordPosition::Argument, "{src:?}");
            assert_eq!(c.word, "", "{src:?} empty word");
        }
        // After a command separator → a new command begins.
        for src in ["echo hi; ", "echo hi |", "echo hi | ", "echo hi && "] {
            let c = ctx(src);
            assert_eq!(c.position, WordPosition::Command, "{src:?}");
            assert_eq!(c.word, "", "{src:?} empty word");
        }
        // Already-correct empty-boundary slots stay put.
        assert_eq!(ctx("if ").position, WordPosition::Command);
        assert_eq!(ctx("cat foo > ").position, WordPosition::RedirectTarget);
    }

    #[test]
    fn cursor_nonempty_word_slot_unchanged_by_gap_a() {
        // The non-empty cases must NOT regress.
        assert_eq!(ctx("whi").position, WordPosition::Command);
        assert_eq!(ctx("if whi").position, WordPosition::Command);
        assert_eq!(ctx("echo $(whi").position, WordPosition::Command);
        assert_eq!(ctx("echo whi").position, WordPosition::Argument);
        assert_eq!(ctx("for x in whi").position, WordPosition::Argument);
    }

    #[test]
    fn cursor_variable_word_start_skips_sigil() {
        // Gap B (iteration 2): `word_start` anchors at the first NAME char, past the
        // `$` / `${` / arith-operand sigil — consistent with `word` (which excludes
        // the sigil).
        let c = ctx("echo $HO");
        assert_eq!(c.word, "HO");
        assert_eq!(c.word_start, 6, "past `$`");
        let c = ctx("echo ${HO");
        assert_eq!(c.word, "HO");
        assert_eq!(c.word_start, 7, "past the braced sigil");
        let c = ctx("echo $(( HO");
        assert_eq!(c.word, "HO");
        assert_eq!(c.word_start, 9, "at the `HO` operand");
    }

    #[test]
    fn cursor_word_and_start() {
        let c = ctx("echo $(whi");
        assert_eq!(c.word, "whi");
        assert_eq!(c.word_start, 7, "anchor right after `$(`");
    }

    #[test]
    fn cursor_enclosing_frames() {
        assert_eq!(
            ctx("echo \"$(whi").enclosing.last(),
            Some(&Frame::CommandSub)
        );
        assert_eq!(ctx("echo $(( whi").enclosing.last(), Some(&Frame::Arith));
        assert!(ctx("echo whi").enclosing.is_empty());
    }

    #[test]
    fn cursor_enclosing_nesting_order_is_innermost_last() {
        // lexer-mode nested in a compound: innermost is the `$(`.
        assert_eq!(
            parse_recover("if echo $(whi").cursor.enclosing.last(),
            Some(&Frame::CommandSub)
        );
        // compound nested in a lexer-mode: innermost is the if-condition.
        assert_eq!(
            parse_recover("echo $(if whi").cursor.enclosing.last(),
            Some(&Frame::IfCondition)
        );
    }

    #[test]
    fn cursor_enclosing_full_nesting_order() {
        // Both frames present, outer-first: `if echo $(whi`.
        assert_eq!(
            parse_recover("if echo $(whi").cursor.enclosing,
            vec![Frame::IfCondition, Frame::CommandSub]
        );
        // Reversed nesting: `echo $(if whi`.
        assert_eq!(
            parse_recover("echo $(if whi").cursor.enclosing,
            vec![Frame::CommandSub, Frame::IfCondition]
        );
    }

    #[test]
    fn cursor_arith_word_is_bare_identifier() {
        let c = parse_recover("echo $(( a + whi").cursor;
        assert_eq!(c.position, WordPosition::VariableName);
        assert_eq!(c.word, "whi");
        assert_eq!(c.word_start, 13);
    }

    #[test]
    fn cursor_array_literal_not_command() {
        // `x=(whi` is an array literal, not a subshell command.
        assert_ne!(ctx("x=(whi").position, WordPosition::Command);
        assert_eq!(ctx("x=$(whi").position, WordPosition::Command);
    }

    #[test]
    fn cursor_for_list_frame_survives_inside_lexer_mode() {
        // for/select word-list inside a `$(` must not drop the ForList frame.
        assert_eq!(
            parse_recover("echo $(for x in y").cursor.enclosing.last(),
            Some(&Frame::ForList)
        );
        assert_eq!(
            parse_recover("echo $(select y in z")
                .cursor
                .enclosing
                .last(),
            Some(&Frame::ForList)
        );
        // Regression guard: the already-working cases stay correct.
        assert_eq!(
            parse_recover("echo $(for x").cursor.enclosing.last(),
            Some(&Frame::ForList)
        );
        assert_eq!(
            parse_recover("if for x in").cursor.enclosing.last(),
            Some(&Frame::ForList)
        );
    }

    #[test]
    fn cursor_lone_dollar_is_variable_position() {
        // `echo $` — a bare `$` at a word start begins a variable expansion; the
        // cursor completes variable names (empty prefix), like `${`. #248.
        for src in ["echo $", "$", "cat foo > $"] {
            let c = parse_recover(src).cursor;
            assert_eq!(c.position, WordPosition::VariableName, "{src:?}");
            assert_eq!(c.word, "", "{src:?}");
            assert_eq!(c.word_start, src.len(), "word_start past the `$`: {src:?}");
        }
        // Regression guards: these must NOT change.
        assert_eq!(
            parse_recover("echo a$").cursor.position,
            WordPosition::Argument
        );
        assert_eq!(
            parse_recover("echo $H").cursor.position,
            WordPosition::VariableName
        );
        assert_eq!(
            parse_recover("echo ${").cursor.position,
            WordPosition::VariableName
        );
    }

    #[test]
    fn cursor_dollar_name_glued_after_segment_isolates_the_name() {
        // A `$name` glued after a preceding word segment completes the NAME only,
        // anchored at the name (not clobbering the segment). #248 whole-branch
        // review. `word_start` is the first name char (past the `$` sigil), so it
        // matches the single-segment `$name` anchor rather than the `$`.
        for (src, word, start) in [
            ("echo foo$MYVAR", "MYVAR", 9usize),
            ("echo a:$MYVAR", "MYVAR", 8),
            ("echo --opt=$MYVAR", "MYVAR", 12),
            ("echo ${HOME}$MYVAR", "MYVAR", 13),
        ] {
            let c = parse_recover(src).cursor;
            assert_eq!(c.position, WordPosition::VariableName, "{src:?}");
            assert_eq!(c.word, word, "{src:?}");
            assert_eq!(c.word_start, start, "{src:?}");
        }
        // Regression guards — single-segment / lone-$ unchanged.
        let c = parse_recover("echo $MYVAR").cursor;
        assert_eq!((c.word.as_str(), c.word_start), ("MYVAR", 6));
        let c = parse_recover("echo $").cursor;
        assert_eq!((c.word.as_str(), c.word_start), ("", 6));
    }

    #[test]
    fn types_are_non_exhaustive_and_public() {
        // Compile-time surface check.
        let _f: Frame = Frame::CommandSub;
        let _p: WordPosition = WordPosition::Argument;
    }

    #[test]
    fn recovered_tree_is_walkable() {
        use crate::command::{Command, SimpleCommand};
        use crate::lexer::WordPart;

        // `echo $(whi` — recovery closes the `$(`, so the tree should be the
        // simple command `echo` with one argument: a command substitution
        // whose (recovered) inner body is the simple command `whi`.
        let r = parse_recover("echo $(whi");
        let seq = r.tree.expect("tree");

        // The top-level command is a one-stage `Pipeline` wrapping the simple
        // `echo` command (the parser's normal shape for a bare simple command).
        let Command::Pipeline(outer_pipeline) = &seq.first else {
            panic!("expected a top-level pipeline, got {:?}", seq.first);
        };
        assert_eq!(outer_pipeline.commands.len(), 1, "single-stage pipeline");
        let Command::Simple(SimpleCommand::Exec(outer)) = &outer_pipeline.commands[0] else {
            panic!(
                "expected a simple exec command, got {:?}",
                outer_pipeline.commands[0]
            );
        };
        assert_eq!(
            outer.program_static_text().as_deref(),
            Some("echo"),
            "outer program is `echo`"
        );
        assert_eq!(
            outer.args.len(),
            1,
            "one argument: the command substitution"
        );

        let arg_parts = &outer.args[0].0;
        assert_eq!(
            arg_parts.len(),
            1,
            "the argument is exactly one command-substitution word part"
        );
        let WordPart::CommandSub {
            sequence: inner, ..
        } = &arg_parts[0]
        else {
            panic!("expected a CommandSub word part, got {:?}", arg_parts[0]);
        };

        let Command::Pipeline(inner_pipeline) = &inner.first else {
            panic!(
                "expected the command-sub body's top command to be a pipeline, got {:?}",
                inner.first
            );
        };
        assert_eq!(
            inner_pipeline.commands.len(),
            1,
            "single-stage inner pipeline"
        );
        let Command::Simple(SimpleCommand::Exec(inner_cmd)) = &inner_pipeline.commands[0] else {
            panic!(
                "expected the command-sub body to be a simple exec command, got {:?}",
                inner_pipeline.commands[0]
            );
        };
        assert_eq!(
            inner_cmd.program_static_text().as_deref(),
            Some("whi"),
            "inner command-sub body is the command `whi`"
        );
    }

    #[test]
    fn recover_never_panics_on_any_truncation() {
        // A spread of inputs exercising every construct; truncate each at every
        // byte offset (including inside multi-byte-free ASCII here) and assert
        // parse_recover returns without panicking.
        let corpus = [
            "echo hi",
            "echo $(whi)",
            "echo \"$(ls) $x\"",
            "if a; then b; fi",
            "for x in a b; do echo $x; done",
            "case $x in a) b;; esac",
            "a=(1 2 3)",
            "echo ${x:-def}",
            "echo $(( 1 + 2 ))",
            "f() { echo `date`; }",
            "while read l; do :; done < f",
            "{ a; b; }",
        ];
        for s in corpus {
            for i in 0..=s.len() {
                if !s.is_char_boundary(i) {
                    continue;
                }
                // Must not panic. Result content is not asserted here.
                let _ = parse_recover(&s[..i]);
            }
        }
    }
}
