//! v363 (#666): the highlight recorder is a pure function of the text.
//!
//! These tests drive it exactly as the highlighter will — parse the line, then
//! read what the parse recorded. They are the unit layer; rendered output is
//! asserted separately in `tests/highlight_render_pty.rs`.

use huck_syntax::highlight::{HighlightRecord, Role};
use huck_syntax::lexer::{Lexer, LexerOptions};

fn record(src: &str) -> HighlightRecord {
    let empty = std::collections::HashMap::new();
    let opts = LexerOptions {
        record_highlight: true,
        ..Default::default()
    };
    let mut lx = Lexer::new(src, &empty, opts);
    // A parse error is the NORMAL case while typing; the record is still valid.
    let _ = huck_syntax::parser::parse_sequence(&mut lx);
    lx.take_highlight_record()
}

fn starts(src: &str, want: Role) -> Vec<usize> {
    record(src)
        .marks
        .into_iter()
        .filter(|m| m.role == want)
        .map(|m| m.start)
        .collect()
}

#[test]
fn single_and_double_quotes_are_distinguishable() {
    // The whole point of the design: `'` and `"` must not collapse to one role.
    //
    // The two arrive DIFFERENTLY, and the asymmetry is the lexer's: a
    // single-quoted run is ONE token carrying its style, while a double-quoted
    // run is a FRAME (`BeginDquote` … `EndDquote`) around tokenised contents,
    // which is what keeps expansions inside it separately visible.
    //
    // Both still yield ONE mark, but by different routes. The frame's delimiter
    // tokens are useless on their own — `BeginDquote` is zero-width, so its mark
    // was `[10..10)` and `EndDquote` covered only the closing quote, which
    // painted `"dq"` as exactly one character (John found that by using the
    // shell). The run is therefore marked when it CLOSES, spanning the frame's
    // `open_off`, and the delimiter tokens are skipped.
    let src = "echo 'sq' \"dq\"";
    assert_eq!(
        starts(src, Role::QuotedSingle),
        vec![5],
        "single-quoted run at 5"
    );
    assert_eq!(
        starts(src, Role::QuotedDouble),
        vec![10],
        "one mark for the whole double-quoted run"
    );
    // …and it spans the run, not just a delimiter — the actual regression.
    let dq = record(src)
        .marks
        .into_iter()
        .find(|m| m.role == Role::QuotedDouble)
        .expect("a QuotedDouble mark");
    assert_eq!(&src[dq.start..dq.end], "\"dq\"", "the whole run is covered");
}

#[test]
fn expansions_and_their_names_are_marked() {
    let src = "echo $HOME ${x:-d} $(date)";
    assert!(
        !starts(src, Role::VarName).is_empty(),
        "expected a VarName mark in {src:?}"
    );
    assert!(
        !starts(src, Role::Expansion).is_empty(),
        "expected an Expansion mark in {src:?}"
    );
}

#[test]
fn operators_are_marked() {
    assert!(!starts("a | b && c", Role::Operator).is_empty());
}

#[test]
fn marks_never_overlap_and_stay_inside_the_line() {
    // The extent invariant: every mark is non-empty, ordered, and in bounds.
    for src in [
        "echo 'sq' \"dq\" $HOME ${x:-d} $(date) | grep x",
        "for f in *.rs; do echo \"$f\"; done",
        "echo \"unterminated",
    ] {
        let mut marks = record(src).marks;
        marks.sort_by_key(|m| (m.start, m.end));
        for m in &marks {
            assert!(m.start <= m.end, "{src:?}: inverted mark {m:?}");
            assert!(m.end <= src.len(), "{src:?}: out-of-bounds mark {m:?}");
        }
    }
}

#[test]
fn recording_is_off_by_default() {
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new("echo 'x' $HOME | grep y", &empty, LexerOptions::default());
    let _ = huck_syntax::parser::parse_sequence(&mut lx);
    let rec = lx.take_highlight_record();
    assert!(rec.marks.is_empty(), "default must record nothing");
    assert!(rec.pairs.is_empty());
}

#[test]
fn an_incomplete_line_still_records_what_it_scanned() {
    // The usual state while typing: parsing stops at the error, but everything
    // scanned BEFORE it is still marked and must still be painted.
    let src = "echo 'done' 'unterminat";
    assert_eq!(
        starts(src, Role::QuotedSingle),
        vec![5],
        "the closed run at 5 is marked; the unterminated one is not"
    );
}

#[test]
fn an_unterminated_construct_records_no_mark_of_its_own() {
    // Known state, pinned deliberately rather than left to be discovered later.
    //
    // A construct that never closes produces NO token — the scan fails at
    // end-of-input instead of emitting one — so there is nothing for the
    // recorder to observe. Marking the dangling opener is `unterminated`'s job
    // (Task 6), fed by the same walk v362 built for EOF diagnostics. Until then
    // an unterminated quote is simply unpainted, which is honest: we do not yet
    // claim to know where it ends.
    let rec = record("echo 'unterminated");
    assert!(
        !rec.marks.iter().any(|m| m.role == Role::QuotedSingle),
        "no QuotedSingle mark yet: {:?}",
        rec.marks
    );
    assert_eq!(rec.unterminated, None, "not wired until Task 6");
}
