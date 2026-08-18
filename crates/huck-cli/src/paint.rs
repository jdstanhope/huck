//! Role -> SGR: the ONE place huck decides what highlighting looks like
//! (v363, #666).
//!
//! `huck-syntax` records semantic [`Role`]s and knows nothing about colour; this
//! module owns the palette and the painting. Keeping the two apart means the
//! lexer never grows a terminal dependency, and a palette change touches one
//! table.
//!
//! # The width contract
//!
//! rustyline requires the highlighted string to have the SAME DISPLAY WIDTH as
//! the input: it reasons about cursor position from the original line. So only
//! zero-width SGR sequences may be inserted — never a character added, removed
//! or substituted. `render` copies the input's characters verbatim and only
//! wraps runs of them in escapes, which makes the contract structural rather
//! than something each role has to remember.

use huck_engine::highlight::{HighlightRecord, Role};

/// The SGR parameters for a role. One table, deliberately small.
///
/// A VALID command word is absent on purpose — it stays the terminal's default
/// colour. Only the problem is signalled (fish's restraint), because a line
/// where every word is coloured stops being readable.
fn sgr(role: Role) -> &'static str {
    match role {
        // Task 4 records this only for a command that does NOT resolve.
        Role::CommandWord => "31",  // red
        Role::Keyword => "1;34",    // bold blue
        Role::QuotedSingle => "32", // green  — inert text
        Role::QuotedDouble => "33", // yellow — still expands
        Role::Expansion => "36",    // cyan
        Role::VarName => "1;36",    // bold cyan — the name is what the eye hunts for
        Role::Operator => "35",     // magenta
        Role::Redirect => "95",     // bright magenta
        Role::Comment => "90",      // grey
        Role::Glob => "34",         // blue
        Role::Escape => "96",       // bright cyan
        Role::Tilde => "34",        // blue
    }
}

/// Paint `line` according to `rec`.
///
/// `enabled == false` returns the line untouched — the single gate for
/// `NO_COLOR`, a non-tty and the shell option (Task 7). It is checked here so
/// no caller can forget it.
///
/// Overlap is resolved by a per-byte role map rather than by trying to nest
/// escapes, applying marks WIDEST FIRST so a narrower mark refines the region it
/// sits inside — a `VarName` inside a `QuotedDouble`, say.
///
/// ⚠️ Widest-first, NOT record order. A double-quoted run is only recognised
/// when it CLOSES, so its region mark is appended AFTER the marks for the
/// expansions inside it; ordering by record would let the region overwrite them
/// and `"$HOME"` would lose its bold name. Width is the property that actually
/// expresses containment.
///
/// O(line length) per keystroke — against a measured 4-18 us parse, free.
pub fn render(line: &str, rec: &HighlightRecord, enabled: bool) -> String {
    if !enabled || rec.marks.is_empty() {
        return line.to_string();
    }
    let mut ordered: Vec<&huck_engine::highlight::Mark> = rec.marks.iter().collect();
    ordered.sort_by_key(|m| std::cmp::Reverse(m.end.saturating_sub(m.start)));
    let mut role_at: Vec<Option<Role>> = vec![None; line.len()];
    for m in ordered {
        // Defensive: a zero-length or out-of-bounds mark paints nothing rather
        // than panicking. Task 1 records zero-length marks for the zero-width
        // opener signals on purpose, so this is a normal path, not an error.
        let end = m.end.min(line.len());
        for slot in role_at.iter_mut().take(end).skip(m.start) {
            *slot = Some(m.role);
        }
    }

    let mut out = String::with_capacity(line.len() + 32);
    let mut open: Option<Role> = None;
    // Group by CHARACTER, not byte: a run boundary must never fall inside a
    // multi-byte character. Marks come from token spans (char boundaries), so
    // the map is uniform within a character; taking the first byte's role makes
    // that explicit instead of assuming it.
    for (i, ch) in line.char_indices() {
        let want = role_at.get(i).copied().flatten();
        if want != open {
            if open.is_some() {
                out.push_str("\x1b[0m");
            }
            if let Some(r) = want {
                out.push_str("\x1b[");
                out.push_str(sgr(r));
                out.push('m');
            }
            open = want;
        }
        out.push(ch);
    }
    if open.is_some() {
        out.push_str("\x1b[0m");
    }
    out
}

/// Strip SGR sequences — used by tests to assert the width contract, and by the
/// pty harness to compare visible text.
pub fn strip_sgr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // ESC [ … m
        for c2 in chars.by_ref() {
            if c2 == 'm' {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use huck_engine::highlight::Mark;
    use huck_engine::lexer::{Lexer, LexerOptions};

    fn rec_for(src: &str) -> HighlightRecord {
        let empty = std::collections::HashMap::new();
        let opts = LexerOptions {
            record_highlight: true,
            ..Default::default()
        };
        let mut lx = Lexer::new(src, &empty, opts);
        let _ = huck_engine::parser::parse_sequence(&mut lx);
        lx.take_highlight_record()
    }

    #[test]
    fn visible_text_is_never_altered() {
        // The width contract. If this fails, rustyline's cursor arithmetic is
        // wrong and the editor misbehaves in ways no unit test would explain.
        for src in [
            "echo 'sq' \"dq\"",
            "echo $HOME ${x:-d} $(date) | grep x",
            "for f in *.rs; do echo \"$f\"; done",
            "echo \"unterminated",
            "echo 'ünïcödé' $HÖME",
        ] {
            let out = render(src, &rec_for(src), true);
            assert_eq!(strip_sgr(&out), src, "visible text changed for {src:?}");
        }
    }

    #[test]
    fn disabled_returns_the_line_untouched() {
        let rec = HighlightRecord {
            marks: vec![Mark {
                start: 0,
                end: 4,
                role: Role::CommandWord,
            }],
            ..Default::default()
        };
        assert_eq!(render("echo hi", &rec, false), "echo hi");
    }

    #[test]
    fn quotes_get_different_colours() {
        let src = "echo 'sq' \"dq\"";
        let out = render(src, &rec_for(src), true);
        assert!(out.contains("\x1b[32m"), "single-quoted run green: {out:?}");
        assert!(
            out.contains("\x1b[33m"),
            "double-quoted run yellow: {out:?}"
        );
    }

    #[test]
    fn a_variable_name_is_bold() {
        let src = "echo $HOME";
        let out = render(src, &rec_for(src), true);
        assert!(out.contains("\x1b[1;36m"), "bold cyan name: {out:?}");
    }

    #[test]
    fn a_narrower_mark_refines_the_region_it_sits_in() {
        // Containment is expressed by WIDTH, not by record order — and the order
        // is genuinely adverse: a double-quoted region is only recognised when it
        // CLOSES, so it is recorded AFTER the expansions inside it.
        let rec = HighlightRecord {
            marks: vec![
                Mark {
                    start: 0,
                    end: 10,
                    role: Role::QuotedDouble,
                },
                Mark {
                    start: 3,
                    end: 6,
                    role: Role::VarName,
                },
            ],
            ..Default::default()
        };
        let out = render("0123456789", &rec, true);
        assert_eq!(strip_sgr(&out), "0123456789");
        assert!(
            out.contains("\x1b[1;36m345\x1b[0m"),
            "inner run painted: {out:?}"
        );
    }

    #[test]
    fn no_marks_means_no_escapes() {
        assert_eq!(render("plain", &HighlightRecord::default(), true), "plain");
    }

    #[test]
    fn nesting_survives_either_record_order() {
        // The adverse order is the real one, so assert both.
        for marks in [
            vec![
                Mark {
                    start: 0,
                    end: 10,
                    role: Role::QuotedDouble,
                },
                Mark {
                    start: 3,
                    end: 6,
                    role: Role::VarName,
                },
            ],
            vec![
                Mark {
                    start: 3,
                    end: 6,
                    role: Role::VarName,
                },
                Mark {
                    start: 0,
                    end: 10,
                    role: Role::QuotedDouble,
                },
            ],
        ] {
            let rec = HighlightRecord {
                marks,
                ..Default::default()
            };
            let out = render("0123456789", &rec, true);
            assert_eq!(strip_sgr(&out), "0123456789");
            assert!(
                out.contains("\x1b[1;36m345\x1b[0m"),
                "inner run must survive whichever order it was recorded in: {out:?}"
            );
        }
    }

    #[test]
    fn a_double_quoted_run_paints_whole_and_keeps_its_variable_bold() {
        // The defect John found by USING the shell: `"dq"` painted exactly one
        // character, because `BeginDquote` is zero-width and `EndDquote` covers
        // only the closing quote. The run is now marked from its frame.
        let src = "echo \"a $B c\"";
        let out = render(src, &rec_for(src), true);
        assert_eq!(strip_sgr(&out), src);
        assert!(out.contains("\x1b[33m"), "the run is yellow: {out:?}");
        assert!(
            out.contains("\x1b[1;36m$B\x1b[0m"),
            "the variable inside it stays bold: {out:?}"
        );
    }
}
