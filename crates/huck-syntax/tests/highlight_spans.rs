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
fn every_mark_is_ordered_and_inside_the_line() {
    // The extent invariant: every mark is ordered and in bounds.
    //
    // NOT non-overlapping, and the old name of this test claimed otherwise.
    // Marks nest ON PURPOSE — a `VarName` sits inside a `QuotedDouble` region, a
    // `Glob` sits inside the word that holds it — and the painter resolves that
    // by applying the widest first. A non-overlap assertion here would have been
    // a false claim about the design.
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
    // recorder to observe. The dangling opener is reported by `unterminated`
    // instead, fed by the same walk v362 built for EOF diagnostics — a span with
    // no end cannot be a REGION, but its start is exactly what a reader needs.
    let rec = record("echo 'unterminated");
    assert!(
        !rec.marks.iter().any(|m| m.role == Role::QuotedSingle),
        "no QuotedSingle mark: {:?}",
        rec.marks
    );
    assert_eq!(
        rec.unterminated,
        Some(5),
        "...but the opener it is waiting on is named"
    );
}

/// Every `(text, role)` pair the record holds, in source order — the shape the
/// command-position tests below assert against.
fn roles(src: &str) -> Vec<(&str, Role)> {
    let mut v: Vec<_> = record(src)
        .marks
        .into_iter()
        .map(|m| (&src[m.start..m.end.min(src.len())], m.role))
        .collect();
    v.retain(|(t, _)| !t.is_empty());
    v
}

#[test]
fn only_the_command_word_of_a_command_is_a_command_word() {
    // The measurement that drove Task 3. Every row here was WRONG before it,
    // and each is wrong in its own way:
    //
    //   * `for f in …` painted the loop VARIABLE as a command;
    //   * `case … in a)` painted the PATTERN as one;
    //   * `!` was a command rather than the reserved word it is;
    //   * `FOO=bar cmd` painted the assignment red and missed `cmd` entirely.
    //
    // They share a root: command position was read when a token was SCANNED,
    // and the parser's lookahead scans a token before it has said what it is.
    // The role is settled at CONSUME time now, which is always after the
    // declaration.
    assert_eq!(
        roles("for f in a; do echo $f; done"),
        vec![
            ("for", Role::Keyword),
            ("f", Role::Word),
            ("in", Role::Keyword),
            ("a", Role::Word),
            (";", Role::Operator),
            ("do", Role::Keyword),
            ("echo", Role::CommandWord),
            ("$f", Role::VarName),
            (";", Role::Operator),
            ("done", Role::Keyword),
        ],
    );
    assert_eq!(
        roles("case $x in a) echo hi;; esac"),
        vec![
            ("case", Role::Keyword),
            ("$x", Role::VarName),
            ("in", Role::Keyword),
            ("a", Role::Word),
            (")", Role::Operator),
            ("echo", Role::CommandWord),
            ("hi", Role::Word),
            (";;", Role::Operator),
            ("esac", Role::Keyword),
        ],
    );
    assert_eq!(
        roles("! false"),
        vec![("!", Role::Keyword), ("false", Role::CommandWord)],
    );
}

#[test]
fn an_assignment_prefix_is_a_variable_and_the_command_follows_it() {
    // `FOO=bar cmd` — three separate claims:
    //   the NAME is a variable name, the VALUE is plain text, and `cmd` is
    //   still the command word even though it is the second word.
    assert_eq!(
        roles("FOO=bar cmd arg"),
        vec![
            ("FOO=", Role::VarName),
            ("bar", Role::Word),
            ("cmd", Role::CommandWord),
            ("arg", Role::Word),
        ],
    );
    // A bare assignment with no command at all.
    assert_eq!(roles("FOO="), vec![("FOO=", Role::VarName)]);
    // ⚠️ The row that keeps the claim honest: an assignment VALUE that is a
    // command substitution still marks the command inside it. A claim that
    // swept the whole word would paint `nosuch` as plain text.
    assert_eq!(
        roles("x=$(nosuch)")
            .into_iter()
            .filter(|(_, r)| *r != Role::Expansion)
            .collect::<Vec<_>>(),
        vec![
            ("x=", Role::VarName),
            ("nosuch", Role::CommandWord),
            (")", Role::Operator),
        ],
    );
}

#[test]
fn a_comment_is_marked_even_though_it_produces_no_token() {
    // The recorder reads the tokens a step produced, and a comment produces
    // none — the scanner just skips it. So the mark is made at the skip, which
    // is the only place the comment's extent exists.
    assert_eq!(
        roles("echo hi # a note"),
        vec![
            ("echo", Role::CommandWord),
            ("hi", Role::Word),
            ("# a note", Role::Comment),
        ],
    );
    // A `#` that does NOT begin a word is literal, not a comment.
    assert!(
        !roles("echo a#b").iter().any(|(_, r)| *r == Role::Comment),
        "a mid-word `#` is not a comment"
    );
}

#[test]
fn a_command_inside_a_substitution_is_a_command_too() {
    // Command position is nested, not a property of the line: the thing inside
    // `$(…)` is checked in its own right, which is what makes an invalid command
    // inside a substitution visible (Task 4).
    let inner: Vec<_> = roles("echo $(nosuch)")
        .into_iter()
        .filter(|(_, r)| *r == Role::CommandWord)
        .collect();
    assert_eq!(
        inner,
        vec![("echo", Role::CommandWord), ("nosuch", Role::CommandWord)],
    );
}

#[test]
fn keywords_are_distinguished_from_command_words() {
    assert_eq!(
        roles("if true; then :; fi")
            .into_iter()
            .filter(|(_, r)| matches!(r, Role::Keyword | Role::CommandWord))
            .collect::<Vec<_>>(),
        vec![
            ("if", Role::Keyword),
            ("true", Role::CommandWord),
            ("then", Role::Keyword),
            (":", Role::CommandWord),
            ("fi", Role::Keyword),
        ],
    );
    // An option is an argument, not a command word — `ls -la` marks only `ls`.
    assert_eq!(
        roles("ls -la"),
        vec![("ls", Role::CommandWord), ("-la", Role::Word)],
    );
}

#[test]
fn glob_metacharacters_are_marked_but_not_the_word_around_them() {
    // `*` is the whole signal: the reader wants to see WHICH characters will be
    // expanded, not that the word contains one somewhere.
    assert_eq!(
        roles("ls *.rs")
            .into_iter()
            .filter(|(_, r)| *r == Role::Glob)
            .collect::<Vec<_>>(),
        vec![("*", Role::Glob)],
    );
    assert_eq!(
        roles("ls a?c")
            .into_iter()
            .filter(|(_, r)| *r == Role::Glob)
            .collect::<Vec<_>>(),
        vec![("?", Role::Glob)],
    );
    // ⚠️ A bracket expression is deliberately NOT marked (#668). It is not
    // reliably one literal run — `f[abc].rs` lexes as `f[`, `abc`, `.rs`,
    // because the subscript scanner peels the brackets — so marking it would
    // colour some bracket globs and silently skip others.
    for src in ["ls f[abc].rs", "ls [ab]"] {
        assert!(
            !roles(src).iter().any(|(_, r)| *r == Role::Glob),
            "{src}: bracket expressions are not marked yet"
        );
    }
}

#[test]
fn a_quoted_glob_is_not_a_glob() {
    // The distinction the colour is FOR: `'a*b'` matches a literal asterisk.
    for src in ["ls 'a*b'", "ls \"a*b\"", "ls a\\*b"] {
        assert!(
            !roles(src).iter().any(|(_, r)| *r == Role::Glob),
            "{src} must have no Glob mark: {:?}",
            roles(src)
        );
    }
    // A `*` inside a bracket expression is literal too — but see #668: brackets
    // are not marked at all yet, so this only pins that the `*` is not claimed.
    assert!(
        !roles("ls 'f[a*b]'").iter().any(|(_, r)| *r == Role::Glob),
        "a quoted bracket expression has no glob at all"
    );
}

#[test]
fn an_escape_is_marked_with_the_character_it_escapes() {
    // Unquoted: `\$` is two source characters and one literal `$`.
    assert_eq!(
        roles("echo a\\$b")
            .into_iter()
            .filter(|(_, r)| *r == Role::Escape)
            .collect::<Vec<_>>(),
        vec![("\\$", Role::Escape)],
    );
    // Inside double quotes the scanner DROPS the backslash, so the token that
    // survives says nothing about it — the mark is made where the drop happens.
    assert_eq!(
        roles("echo \"a\\$b\"")
            .into_iter()
            .filter(|(_, r)| *r == Role::Escape)
            .collect::<Vec<_>>(),
        vec![("\\$", Role::Escape)],
    );
    // A backslash inside double quotes that escapes NOTHING is not an escape:
    // bash keeps both characters, and `\d` is literally `\d`.
    assert!(
        !roles("echo \"a\\db\"")
            .iter()
            .any(|(_, r)| *r == Role::Escape),
        "a non-escaping backslash inside quotes is literal text"
    );
}

/// The `(open, close)` offsets of every pair the parse closed.
fn pairs(src: &str) -> Vec<(usize, usize)> {
    record(src)
        .pairs
        .into_iter()
        .map(|p| (p.open, p.close))
        .collect()
}

#[test]
fn a_closed_construct_records_both_of_its_ends() {
    // Both ends are known at the pop: the frame carries where it opened, and the
    // parser is popping BECAUSE it just consumed the closer.
    let src = "echo $(date)";
    assert_eq!(
        pairs(src),
        vec![(5, 11)],
        "the `$(` at 5 pairs with the `)` at 11"
    );
    let src = "echo \"dq\"";
    assert_eq!(pairs(src), vec![(5, 8)], "the two quotes of `\"dq\"`");
    // Nested pairs are all recorded — the inner one is what a cursor inside it
    // needs to find.
    let src = "echo \"$(date)\"";
    let mut got = pairs(src);
    got.sort();
    assert_eq!(got, vec![(5, 13), (6, 12)]);
}

#[test]
fn an_unterminated_construct_names_where_it_opened() {
    // The dangling opener, from the same walk v362 built for EOF diagnostics:
    // which pair is still open, and where did it start.
    assert_eq!(record("echo \"abc").unterminated, Some(5));
    assert_eq!(record("echo $(date").unterminated, Some(5));
    assert_eq!(record("echo 'abc").unterminated, Some(5));
    // A complete line has nothing dangling.
    assert_eq!(record("echo \"abc\"").unterminated, None);
    assert_eq!(record("echo hi").unterminated, None);
}

#[test]
fn an_unterminated_construct_claims_no_pair() {
    // ⚠️ Found by dumping a real pty stream, not by a test: the parser pops the
    // same frames on the way OUT of a failed parse, and the "closer" is then
    // whatever token happened to be last — so `echo $(d` claimed a pair and the
    // editor reverse-videoed the `d`. A half-typed construct is the normal state
    // while typing, so this was visible constantly.
    assert_eq!(pairs("echo $(d"), Vec::new());
    assert_eq!(pairs("echo \"abc"), Vec::new());
    assert_eq!(pairs("echo ${x"), Vec::new());
    // A pair that DID close before the line ran out is still recorded.
    assert_eq!(pairs("echo $(date) \"abc"), vec![(5, 11)]);
}

#[test]
fn a_heredoc_body_is_text_not_commands() {
    // #670: an EXPANDING heredoc body's words were classified as commands, so
    // `cat <<EOF` / `body $HOME text` painted `body` and `text` red. A body is
    // text; the only thing in it that is a command is one inside a substitution.
    let src = "cat <<EOF\nbody $HOME text\nEOF\n";
    let r = roles(src);
    assert!(
        !r.iter()
            .any(|(t, role)| (*t == "body " || *t == " text") && *role == Role::CommandWord),
        "body words must not be command words: {r:?}"
    );
    assert!(
        r.iter()
            .any(|(t, role)| *t == "$HOME" && *role == Role::VarName),
        "...but an expansion in the body is still an expansion: {r:?}"
    );
    // The command inside a substitution in the body IS one.
    let src = "cat <<EOF\nx $(nosuch) y\nEOF\n";
    assert!(
        roles(src)
            .iter()
            .any(|(t, role)| *t == "nosuch" && *role == Role::CommandWord),
        "a command inside the body's substitution is still a command"
    );
    // A LITERAL body never had the bug — it arrives as one `Lit{quoted:true}`,
    // and only UNQUOTED literals are classified — but pin it anyway, since the
    // reason is a property of the lexer that could change.
    let src = "cat <<'EOF'\nbody text\nEOF\n";
    let r = roles(src);
    assert_eq!(
        r.iter()
            .filter(|(_, role)| *role == Role::CommandWord)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>(),
        vec!["cat"],
        "only the `cat` is a command: {r:?}"
    );
}
