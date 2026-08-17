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
//! # Suppression
//!
//! Not every construct that huck pushes a frame for is a pair bash would name.
//! `suppressed` holds the one rule that differs — inside an arithmetic body a
//! `${` or a `$[` opens nothing (#627) — and `pair_delim`'s `None` arms hold the
//! frames that are never pairs at all.
//!
//! One known divergence is still reproduced on purpose: #644, marked below.

use super::{Mode, ModeFrame};
use crate::command::Delim;

/// What a lex error's raise site saw on the frame stack — captured there because
/// the parser unwinds its frames before any of it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErrorSite {
    /// The byte offset to report the error at.
    pub off: usize,
    /// The pair to name, or `None` to let the error variant name itself.
    pub delim: Option<Delim>,
    /// Whether a compound assignment (`v=(`, `declare -a v=(`) was open. bash
    /// exits **1** for any syntax error raised inside one and 2 everywhere else
    /// (#633), so this rides along with the pair rather than being recomputed
    /// from frames that no longer exist.
    pub in_compound_assign: bool,
}

/// Was a compound assignment open? Not "is the innermost pair one" — bash exits
/// 1 for `v=('abc` and `v=(${x`, where the pair NAMED is the quote or the brace,
/// so this asks about the whole stack.
pub(crate) fn in_compound_assign(frames: &[ModeFrame]) -> bool {
    frames
        .iter()
        .any(|f| matches!(f.mode, Mode::ArrayLiteral { .. }))
}

/// The delimiter an EOF should name right now, and the byte offset to report it
/// at. `None` means no open frame is a reportable pair, in which case the failing
/// atom reports itself — a top-level `'…'` run is scanned as one atom with no
/// frame at all, and that is the case this `None` exists for.
pub(crate) fn reported_pair(frames: &[ModeFrame]) -> Option<(Delim, usize)> {
    // Innermost first. The first frame that either carries an open quote span or
    // is itself a pair answers; anything interior to it (an operand, a subscript,
    // a regex) is not a pair and is skipped, as is a pair the enclosing context
    // suppresses.
    for (i, frame) in frames.iter().enumerate().rev() {
        if let Some(answer) = frame_answer(frame) {
            if suppressed(answer.0, &frames[..i]) {
                continue;
            }
            return Some(answer);
        }
    }
    None
}

/// Is a pair opened by this delimiter invisible to the EOF report, given the
/// frames enclosing it?
///
/// One rule, and it is bash's own: an arithmetic body scans without the flag
/// that makes `${` open a nested pair, so inside one a `${` or a `$[` is plain
/// text — but a `$((` still opens (#627). A quote span is never suppressed: it
/// is the innermost thing open, whatever encloses it.
fn suppressed(opener: Delim, outer: &[ModeFrame]) -> bool {
    matches!(opener, Delim::DollarBrace | Delim::DollarBracket) && inside_arith_body(outer)
}

/// Is the governing context an arithmetic body?
///
/// Every clause here was MEASURED against bash 5.2.21 rather than reasoned out,
/// and two of them are the reason this is not a function of "the innermost
/// enclosing pair":
///
/// | input | bash names | why |
/// | --- | --- | --- |
/// | `$((1+${x` | `)` | the plain case |
/// | `$((1+${x:-$[2+` | `)` | a `${…}` is TRANSPARENT — the `$[` is suppressed too |
/// | `$(($(echo ${x` | `}` | a command substitution resets |
/// | `$(("${x` | `}` | an OPEN quote inside the arithmetic resets |
/// | `$(("a"+${x` | `)` | a CLOSED one does not |
/// | `${x:-$[1+${y` | `]` | the `$[` is not suppressed and then governs |
fn inside_arith_body(outer: &[ModeFrame]) -> bool {
    for frame in outer.iter().rev() {
        match frame.mode {
            // A `${…}` and its interior operands leave the context alone.
            Mode::ParamExpansion { .. }
            | Mode::ParamWordOperand { .. }
            | Mode::ParamSubstPatternOperand { .. }
            | Mode::ParamSubstringOffsetOperand { .. }
            | Mode::ParamSubscriptOperand { .. } => continue,
            // An arithmetic body governs — unless a quote is open inside it right
            // now, which is a quoting context of its own. huck keeps that span as
            // fields ON this frame rather than as a nested frame, which is why it
            // is read here and not caught by the catch-all below.
            Mode::Arith {
                in_squote,
                in_dquote,
                ..
            } => return !in_squote && !in_dquote,
            // Anything else resets: inside a `"…"` or a `$( …` a `${` is a pair
            // again. (A backtick body never gets here — it is scanned raw, so no
            // frame is pushed inside it at all.)
            _ => return false,
        }
    }
    false
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
///   * `Regex` and `Extglob` are not EOF-reportable shapes;
///   * `Command` is the floor.
fn pair_delim(mode: &Mode) -> Option<Delim> {
    match *mode {
        Mode::DoubleQuote => Some(Delim::DQuote),
        Mode::BacktickRaw => Some(Delim::Backtick),
        Mode::ParamExpansion { .. } => Some(Delim::DollarBrace),
        // A frame marked `from_arith_reread` names the `$((` it really came
        // from, which is also how it inherits the opening-line rule (#629).
        Mode::CommandSub {
            from_arith_reread: true,
        } => Some(Delim::DollarDParen),
        Mode::CommandSub {
            from_arith_reread: false,
        } => Some(Delim::DollarParen),
        Mode::Arith { delim, .. } => Some(match delim {
            super::ArithDelim::Paren => Delim::DollarDParen,
            super::ArithDelim::Bracket => Delim::DollarBracket,
        }),
        // `Delim::ArrayParen`, not `Delim::Paren`: a compound assignment's `(`
        // is reported at the line it OPENED on, where a subshell's is reported
        // at the EOF line (#633).
        Mode::ArrayLiteral { .. } => Some(Delim::ArrayParen),
        Mode::Command
        | Mode::ParamWordOperand { .. }
        | Mode::ParamSubstPatternOperand { .. }
        | Mode::ParamSubstringOffsetOperand { .. }
        | Mode::ParamSubscriptOperand { .. }
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
            frame(
                Mode::CommandSub {
                    from_arith_reread: false,
                },
                9,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarParen, 9)));
    }

    #[test]
    fn a_re_read_command_sub_names_the_arith_it_came_from() {
        // #629. `DollarDParen` rather than `DollarParen` is not cosmetic — the two
        // spell the same `)` and take DIFFERENT line rules, and this frame wants
        // the arithmetic's (report where it opened, not where input ran out).
        //
        // Reached, and it was worth checking rather than assuming: arming this arm
        // as `unreachable!()` and running the probes found `echo $((1+2) 'abc` —
        // an atom-scanned quote inside the re-read body — hits it.
        let frames = [
            floor(),
            frame(
                Mode::CommandSub {
                    from_arith_reread: true,
                },
                5,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarDParen, 5)));
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
    fn an_array_literal_names_its_own_paren() {
        // `v=(a` — `Delim::ArrayParen`, which spells `)` like `Delim::Paren` but
        // is Shape 3 and reports the OPENING line (#633).
        let frames = [
            floor(),
            frame(
                Mode::ArrayLiteral {
                    expect_subscript_eq: false,
                    at_element_start: true,
                    subscript_append: false,
                },
                2,
            ),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::ArrayParen, 2)));
    }

    #[test]
    fn a_frame_that_is_not_a_pair_is_skipped_entirely() {
        // A regex is not an EOF-reportable shape at all, so it falls through to
        // the floor.
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

    fn param(off: usize) -> ModeFrame {
        frame(
            Mode::ParamExpansion {
                seen_name: false,
                indirect: false,
                length: false,
                start_off: off,
            },
            off,
        )
    }

    #[test]
    fn an_arith_body_swallows_a_brace_opened_inside_it() {
        // `echo $((1+${x` — bash names `)`, at the arithmetic's own offset (#627).
        let frames = [floor(), frame(arith(false, false, None), 5), param(11)];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarDParen, 5)));
    }

    #[test]
    fn a_brace_is_transparent_so_an_inner_legacy_arith_is_swallowed_too() {
        // `echo $((1+${x:-$[2+` — bash names the OUTER `)`, so neither the `${`
        // nor the `$[` nested inside it opens a pair.
        let frames = [
            floor(),
            frame(arith(false, false, None), 5),
            param(11),
            frame(
                Mode::ParamWordOperand {
                    in_dquote: false,
                    enclosing_dquote: false,
                    is_pattern: false,
                },
                14,
            ),
            frame(
                Mode::Arith {
                    paren_depth: 0,
                    in_squote: false,
                    in_dquote: false,
                    quote_open_off: None,
                    for_header: false,
                    delim: ArithDelim::Bracket,
                },
                16,
            ),
            param(19),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarDParen, 5)));
    }

    #[test]
    fn a_nested_arith_inside_an_arith_still_opens() {
        // `echo $[1+$((2+` — bash names the INNER `)`: only `${`/`$[` are
        // swallowed, `$((` is not.
        let frames = [
            floor(),
            frame(
                Mode::Arith {
                    paren_depth: 0,
                    in_squote: false,
                    in_dquote: false,
                    quote_open_off: None,
                    for_header: false,
                    delim: ArithDelim::Bracket,
                },
                5,
            ),
            frame(arith(false, false, None), 9),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarDParen, 9)));
    }

    #[test]
    fn a_command_substitution_inside_an_arith_resets_the_context() {
        // `echo $(($(echo ${x` — bash names `}`.
        let frames = [
            floor(),
            frame(arith(false, false, None), 5),
            frame(
                Mode::CommandSub {
                    from_arith_reread: false,
                },
                8,
            ),
            param(15),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarBrace, 15)));
    }

    #[test]
    fn only_a_quote_still_open_inside_an_arith_resets_the_context() {
        // `echo $(("${x` names `}` — but `echo $(("a"+${x` names `)`, so it is the
        // LIVE span that resets, not the fact that a quote appeared.
        let open = [floor(), frame(arith(false, true, Some(8)), 5), param(9)];
        assert_eq!(reported_pair(&open), Some((Delim::DollarBrace, 9)));

        let closed = [floor(), frame(arith(false, false, None), 5), param(12)];
        assert_eq!(reported_pair(&closed), Some((Delim::DollarDParen, 5)));
    }

    #[test]
    fn a_legacy_arith_outside_any_arith_opens_and_then_governs() {
        // `echo ${x:-$[1+${y` — bash names `]`: the `$[` is not swallowed (its
        // context is a `${`, which is transparent, then the floor), and it then
        // swallows the `${` inside it.
        let frames = [
            floor(),
            param(5),
            frame(
                Mode::ParamWordOperand {
                    in_dquote: false,
                    enclosing_dquote: false,
                    is_pattern: false,
                },
                8,
            ),
            frame(
                Mode::Arith {
                    paren_depth: 0,
                    in_squote: false,
                    in_dquote: false,
                    quote_open_off: None,
                    for_header: false,
                    delim: ArithDelim::Bracket,
                },
                10,
            ),
            param(14),
        ];
        assert_eq!(reported_pair(&frames), Some((Delim::DollarBracket, 10)));
    }
}
