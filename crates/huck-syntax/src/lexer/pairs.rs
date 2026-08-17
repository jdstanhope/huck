//! Which delimiter an unexpected EOF names (v362, #643).
//!
//! bash reports an EOF at the innermost still-open MATCHED PAIR, and at that
//! pair's line. huck used to answer from whichever `LexError` variant happened to
//! be raised, which is a different question — every divergence in this area is a
//! place where the two answers differ.
//!
//! This module answers it from the frame stack instead. That is possible without
//! any new storage because v361 (#641) put each construct's opening offset ON its
//! frame: `reported_pair` is a pure function of the frames.
//!
//! The division of labour: the error VARIANT still decides the message SHAPE —
//! Shape 3 (`unexpected EOF while looking for matching X`) versus Shape 2
//! (`syntax error: unexpected end of file`) — and this module decides the
//! delimiter. A non-Shape-3 error never consults it.
//!
//! # This module is deliberately answer-preserving for now
//!
//! v362 Task 2 introduces it returning exactly what huck answered before, so the
//! switch of authority can be proved inert before any rule changes it. The
//! suppression table that makes it match bash — `${` and `$[` opening no pair
//! inside an arithmetic body, and so on — lands in Task 3. Two known divergences
//! are therefore reproduced on purpose here, each marked below: #627 and #644.

use super::{Mode, ModeFrame};
use crate::command::Delim;

/// The delimiter an EOF should name right now, and the byte offset to report it
/// at. `None` means no open frame is a reportable pair, in which case the failing
/// atom reports itself — a top-level `'…'` run is scanned as one atom with no
/// frame at all, and that is the case this `None` exists for.
pub(crate) fn reported_pair(frames: &[ModeFrame]) -> Option<(Delim, usize)> {
    // Innermost first. The first frame that either carries an open quote span or
    // is itself a pair answers; anything interior to it (an operand, a subscript,
    // a regex) is not a pair and is skipped.
    for frame in frames.iter().rev() {
        if let Some(answer) = frame_answer(frame) {
            return Some(answer);
        }
    }
    None
}

/// What one frame reports, or `None` if it is not a pair and carries no span.
fn frame_answer(frame: &ModeFrame) -> Option<(Delim, usize)> {
    // A quote span opened INSIDE a frame outranks the frame itself: it is the
    // innermost thing still open.
    if let Some(answer) = open_quote_span(frame) {
        return Some(answer);
    }
    pair_delim(&frame.mode).map(|delim| (delim, frame.open_off))
}

/// The quote span a frame has open inside itself, and where it opened.
///
/// For `Mode::Arith` the RECORDED OFFSET is the signal, not the flag. huck opens
/// a span for a backslash-escaped quote where bash opens none, and
/// `span_opener_off` declines to record an opener in that case (#624) — so
/// `quote_open_off: None` with the flag set means "a span huck invented", and the
/// arith delimiter must keep answering. Reading the flag alone reports the quote
/// and regresses every `\"`/`\'` cell.
///
/// The operand frames track their span as a bare flag with nowhere to record its
/// start, so the frame's own offset is used — the operand's start, not the
/// quote's. That is #644, reproduced deliberately to keep this task inert.
fn open_quote_span(frame: &ModeFrame) -> Option<(Delim, usize)> {
    match frame.mode {
        Mode::Arith {
            in_squote: true,
            quote_open_off: Some(off),
            ..
        } => Some((Delim::SQuote, off)),
        Mode::Arith {
            in_dquote: true,
            quote_open_off: Some(off),
            ..
        } => Some((Delim::DQuote, off)),
        Mode::ParamWordOperand {
            in_dquote: true, ..
        }
        | Mode::ParamSubstPatternOperand {
            in_dquote: true, ..
        }
        | Mode::ParamSubstringOffsetOperand {
            in_dquote: true, ..
        }
        | Mode::ParamSubscriptOperand {
            in_dquote: true, ..
        } => Some((Delim::DQuote, frame.open_off)),
        _ => None,
    }
}

/// The delimiter a frame names when input runs out inside it, or `None` for a
/// frame that is not a reportable pair.
///
/// The `None`s are as load-bearing as the `Some`s:
///
///   * the operand and subscript modes are interior to a `${…}` that is already
///     on the stack — bash names the `${`;
///   * `ArrayLiteral` IS a pair in bash (`v=(` names `)`) but not yet here: it
///     changes message shape and exit status, which is #633's own step;
///   * `Regex` and `Extglob` are not EOF-reportable shapes;
///   * `Command` is the floor.
fn pair_delim(mode: &Mode) -> Option<Delim> {
    match *mode {
        Mode::DoubleQuote => Some(Delim::DQuote),
        Mode::BacktickRaw => Some(Delim::Backtick),
        Mode::ParamExpansion { .. } => Some(Delim::DollarBrace),
        Mode::CommandSub => Some(Delim::DollarParen),
        Mode::Arith { delim, .. } => Some(match delim {
            super::ArithDelim::Paren => Delim::DollarDParen,
            super::ArithDelim::Bracket => Delim::DollarBracket,
        }),
        Mode::Command
        | Mode::ParamWordOperand { .. }
        | Mode::ParamSubstPatternOperand { .. }
        | Mode::ParamSubstringOffsetOperand { .. }
        | Mode::ParamSubscriptOperand { .. }
        | Mode::ArrayLiteral { .. }
        | Mode::Regex { .. }
        | Mode::Extglob { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{ArithDelim, Entry};

    fn frame(mode: Mode, open_off: usize) -> ModeFrame {
        ModeFrame {
            mode,
            open_off,
            entry: Entry::Body,
        }
    }

    fn floor() -> ModeFrame {
        frame(Mode::Command, 0)
    }

    fn arith(in_squote: bool, in_dquote: bool, quote_open_off: Option<usize>) -> Mode {
        Mode::Arith {
            paren_depth: 0,
            in_squote,
            in_dquote,
            quote_open_off,
            for_header: false,
            delim: ArithDelim::Paren,
        }
    }

    #[test]
    fn the_innermost_pair_bearing_frame_wins() {
        let frames = [
            floor(),
            frame(Mode::DoubleQuote, 5),
            frame(Mode::CommandSub, 9),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarParen, 9)));
    }

    #[test]
    fn an_interior_frame_defers_to_the_pair_enclosing_it() {
        // An operand is interior to its `${…}`: bash names the `${`.
        let frames = [
            floor(),
            frame(
                Mode::ParamExpansion {
                    seen_name: false,
                    indirect: false,
                    length: false,
                    start_off: 4,
                },
                4,
            ),
            frame(
                Mode::ParamWordOperand {
                    in_dquote: false,
                    enclosing_dquote: false,
                    is_pattern: false,
                },
                8,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarBrace, 4)));
    }

    #[test]
    fn a_live_quote_span_outranks_the_frame_holding_it() {
        // `echo $((1+"` — the span has no frame of its own; the arith frame
        // records where it opened (#621), so that offset is reported.
        let frames = [floor(), frame(arith(false, true, Some(11)), 5)];
        assert_eq!(reported_pair(&frames), Some((Delim::DQuote, 11)));

        let frames = [floor(), frame(arith(true, false, Some(11)), 5)];
        assert_eq!(reported_pair(&frames), Some((Delim::SQuote, 11)));
    }

    #[test]
    fn an_escaped_quote_left_no_offset_so_the_arith_answers() {
        // `span_opener_off` declines to record a quote preceded by an odd
        // backslash run (#624), which is how `echo $((1+\"` keeps naming `)`.
        let frames = [floor(), frame(arith(false, false, None), 5)];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarDParen, 5)));
    }

    #[test]
    fn an_operand_span_reports_the_operands_offset_not_the_quotes() {
        // Deliberate: reproduces #644 so this task stays inert. The operand
        // frames track their span as a flag with nowhere to record its start.
        let frames = [
            floor(),
            frame(
                Mode::ParamExpansion {
                    seen_name: false,
                    indirect: false,
                    length: false,
                    start_off: 4,
                },
                4,
            ),
            frame(
                Mode::ParamWordOperand {
                    in_dquote: true,
                    enclosing_dquote: false,
                    is_pattern: false,
                },
                8,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DQuote, 8)));
    }

    #[test]
    fn a_frame_that_is_not_a_pair_is_skipped_entirely() {
        // `v=(` is a pair in bash but not yet here (#633), and a regex is not an
        // EOF-reportable shape at all — both fall through to the floor.
        let frames = [
            floor(),
            frame(
                Mode::ArrayLiteral {
                    expect_subscript_eq: false,
                    at_element_start: true,
                    subscript_append: false,
                },
                3,
            ),
        ];
        assert_eq!(reported_pair(&frames), None);

        let frames = [
            floor(),
            frame(
                Mode::Regex {
                    paren_depth: 0,
                    has_content: false,
                },
                3,
            ),
        ];
        assert_eq!(reported_pair(&frames), None);
    }

    #[test]
    fn no_frame_at_all_means_the_atom_reports_itself() {
        // A top-level `'…'` run is one atom with no frame; the caller falls back
        // to the failing step's own start.
        assert_eq!(reported_pair(&[floor()]), None);
    }

    #[test]
    fn arith_still_names_the_brace_inside_it_until_task_3() {
        // `echo $((1+${x` — bash names `)`, huck names `}`. That is #627, and it
        // is reproduced here on purpose: Task 2 must be inert, Task 3 adds the
        // suppression table that fixes it.
        let frames = [
            floor(),
            frame(arith(false, false, None), 5),
            frame(
                Mode::ParamExpansion {
                    seen_name: false,
                    indirect: false,
                    length: false,
                    start_off: 11,
                },
                11,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarBrace, 11)));
    }
}
