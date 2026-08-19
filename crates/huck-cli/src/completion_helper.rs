//! rustyline completion adapter: `HuckHelper` wires huck-engine's
//! completion dispatch into rustyline's `Completer`/`Helper` traits.

use huck_engine::completion::{self, Candidate};
use huck_engine::shell_state::Shell;
use std::cell::RefCell;
use std::rc::Rc;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Helper};

/// rustyline completion helper. Holds an `Rc<RefCell<Shell>>` so the
/// completion callback can read AND mutate shell state (required by
/// `-F func` execution during Tab). The Rust-borrow discipline is:
/// `complete()` acquires `borrow_mut()` for the duration of the call
/// and releases on return. The main loop must hold NO borrow across
/// `editor.readline()` so this acquisition succeeds.
pub struct HuckHelper {
    shell: Rc<RefCell<Shell>>,
    /// v363 (#666): whether to emit colour at all. Resolved ONCE at
    /// construction from the terminal and `NO_COLOR`, never per keystroke.
    ///
    /// This gate is why the 309-harness diff sweep stays green: those harnesses
    /// pipe huck, so stdout is not a terminal and nothing is painted. Task 7
    /// adds the shell option and the tests that pin all three conditions.
    colour_enabled: bool,
    /// v363 (#666): what has already been established about the command words on
    /// the line being typed. Cleared at every prompt (`clear_validity_cache`), so
    /// a program installed on one line is seen on the next.
    validity: RefCell<huck_engine::cmd_validity::ValidityCache>,
    /// The lines of this logical command already entered, with their joiner
    /// (#670, #673). Empty at a PS1 prompt; at a PS2 prompt it is what makes the
    /// line being typed parseable at all — `then echo hi` is a syntax error on
    /// its own, and a fragment that cannot parse can be neither coloured nor
    /// completed.
    prefix: RefCell<String>,
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self {
        let colour_enabled = std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none();
        Self {
            shell,
            colour_enabled,
            validity: RefCell::new(huck_engine::cmd_validity::ValidityCache::new()),
            prefix: RefCell::new(String::new()),
        }
    }

    /// Tell the helper what this line CONTINUES (#670, #673).
    ///
    /// Set once per prompt, not per keystroke: at a PS1 prompt it is empty, and
    /// at a PS2 prompt it is the accumulated buffer plus the joiner that the
    /// next line will be appended with — the same two values the reader itself
    /// uses, taken from the same place, so there is no second notion of how
    /// lines join.
    ///
    /// BOTH consumers need it, and for the same reason: highlighting and
    /// completion are each driven by a PARSE, and a continuation line does not
    /// parse on its own.
    pub fn set_line_prefix(&self, prefix: String) {
        *self.prefix.borrow_mut() = prefix;
    }

    /// Is colour wanted right now?
    ///
    /// Three gates, and they are deliberately answered in different places. The
    /// terminal and `NO_COLOR` cannot change while the shell runs, so they are
    /// resolved once at construction. `shopt -u syntax_highlight` CAN change —
    /// that is the point of it — so it is read live, and takes effect on the
    /// next keystroke rather than the next session.
    fn colour_now(&self) -> bool {
        self.colour_enabled
            && self
                .shell
                .borrow()
                .shopt_options
                .get("syntax_highlight")
                .unwrap_or(true)
    }

    /// Forget what was established about command names. The REPL calls this once
    /// per prompt: it is what bounds staleness to a single line.
    pub fn clear_validity_cache(&self) {
        self.validity.borrow_mut().clear();
    }

    /// The roles for one line, ready to paint: parse it, then settle which
    /// command words actually resolve.
    ///
    /// Separate from `highlight` so it can be tested without a terminal — the
    /// colour gate would otherwise make every assertion vacuous.
    ///
    /// Parsed with NO aliases on purpose: expanding them would paint the
    /// expansion's structure over text the user never typed.
    /// v363 (#666): turn the pair the cursor is touching, and any dangling
    /// opener, into marks the painter can lay down.
    ///
    /// Done here rather than inside `render` so the painter stays one thing:
    /// "paint these extents". The cursor is the only input to highlighting that
    /// is not a function of the TEXT, and keeping it out of the record means the
    /// record is still a pure function of the line — which is what makes it
    /// testable without a terminal.
    ///
    /// A pair's ends are WHOLE delimiters, not single characters: `$(` and `)`,
    /// `${` and `}`, `$((` and `))`. Marking one character emphasised the `$` of
    /// `$(` and left the bracket plain, which reads as off-by-one — reported from
    /// using it.
    ///
    /// The delimiter's width comes from the mark the recorder already laid down
    /// for the opener TOKEN (`$(` is one `Expansion` mark), so nothing re-derives
    /// it from the text. A `"` has no such mark — its region mark spans the whole
    /// string, which is not a delimiter — so it falls back to one character,
    /// which is exactly right for a quote.
    fn delimiter_extent(
        rec: &huck_engine::highlight::HighlightRecord,
        at: usize,
    ) -> (usize, usize) {
        use huck_engine::highlight::Role;
        rec.marks
            .iter()
            .find(|m| m.start == at && m.end > m.start && m.role == Role::Expansion)
            .map(|m| (m.start, m.end))
            .unwrap_or((at, at + 1))
    }

    fn emphasise_pairs(rec: &mut huck_engine::highlight::HighlightRecord, cursor: usize) {
        use huck_engine::highlight::{Mark, Role};
        let mut pending: Vec<Mark> = Vec::new();
        let mut add = |(start, end): (usize, usize), role: Role| {
            pending.push(Mark { start, end, role });
        };
        if let Some(open) = rec.unterminated {
            add(Self::delimiter_extent(rec, open), Role::DanglingOpener);
        }
        // The cursor sits BETWEEN characters, so a bracket is "under" it when the
        // cursor is on it or just past it — which is where it lands the moment
        // you type the thing.
        //
        // A degenerate record (`close <= open`) is dropped: `${x:-d}` pushes an
        // OPERAND frame whose two ends are the same offset, and emphasising it
        // would light up a character with no partner.
        let touching: Vec<(usize, usize)> = rec
            .pairs
            .iter()
            .filter(|p| p.close > p.open)
            .filter(|p| {
                let (_, open_end) = Self::delimiter_extent(rec, p.open);
                let (_, close_end) = Self::delimiter_extent(rec, p.close);
                (p.open..=open_end).contains(&cursor) || (p.close..=close_end).contains(&cursor)
            })
            .map(|p| (p.open, p.close))
            .collect();
        for (open, close) in touching {
            add(Self::delimiter_extent(rec, open), Role::PairMatch);
            add(Self::delimiter_extent(rec, close), Role::PairMatch);
        }
        rec.marks.append(&mut pending);
    }

    fn highlight_record(&self, line: &str) -> huck_engine::highlight::HighlightRecord {
        let prefix = self.prefix.borrow().clone();
        let source = if prefix.is_empty() {
            line.to_string()
        } else {
            format!("{prefix}{line}")
        };
        let no_aliases = std::collections::HashMap::new();
        let opts = huck_engine::lexer::LexerOptions {
            record_highlight: true,
            ..Default::default()
        };
        let mut lx = huck_engine::lexer::Lexer::new(&source, &no_aliases, opts);
        let _ = huck_engine::parser::parse_sequence(&mut lx);
        let mut rec = lx.take_highlight_record();
        // The record indexes what was PARSED; the painter indexes what is on
        // screen. Those differ by exactly the prefix.
        Self::rebase_onto_line(&mut rec, prefix.len(), line.len());
        self.resolve_command_validity(line, &mut rec);
        rec
    }

    /// Move a record recorded over `prefix + line` onto `line` alone (#670).
    ///
    /// Three different answers, because the three things in a record mean
    /// different things when they straddle the boundary:
    ///
    /// * a MARK is a region, and a region that starts above the visible line is
    ///   still visible from column zero — a `"…"` opened on the previous line
    ///   colours this one to its close. So it is CLAMPED, not dropped.
    /// * a PAIR is a relation between two points, and emphasising one end of it
    ///   would point at a character that is not its partner. So a pair is kept
    ///   only when BOTH ends are on this line.
    /// * the DANGLING OPENER is a single position. If it is above the line there
    ///   is nothing on screen to underline, so it is dropped rather than moved
    ///   to column zero, which would accuse the wrong character.
    fn rebase_onto_line(
        rec: &mut huck_engine::highlight::HighlightRecord,
        base: usize,
        line_len: usize,
    ) {
        if base == 0 {
            return;
        }
        let end = base + line_len;
        rec.marks.retain(|m| m.end > base && m.start < end);
        for m in rec.marks.iter_mut() {
            m.start = m.start.max(base) - base;
            m.end = m.end.min(end) - base;
        }
        rec.pairs
            .retain(|p| p.open >= base && p.close >= base && p.close < end);
        for p in rec.pairs.iter_mut() {
            p.open -= base;
            p.close -= base;
        }
        rec.unterminated = rec
            .unterminated
            .filter(|&off| off >= base)
            .map(|off| off - base);
    }

    /// v363 (#666): decide which command words are worth painting.
    ///
    /// The record marks command position — a fact about the grammar. Whether the
    /// command EXISTS is a fact about this machine, and it is asked here, where
    /// the shell is to hand. fish's restraint is the design: a command that
    /// resolves is left alone, so the only colour on an ordinary line is the one
    /// that means "this will not run".
    ///
    /// `Unknown` (the search budget was blown) is treated as valid. Painting a
    /// name we declined to look up would be a guess shown in red.
    fn resolve_command_validity(
        &self,
        line: &str,
        rec: &mut huck_engine::highlight::HighlightRecord,
    ) {
        use huck_engine::cmd_validity::Validity;
        use huck_engine::highlight::Role;
        if !rec.marks.iter().any(|m| m.role == Role::CommandWord) {
            return;
        }
        let mut shell = self.shell.borrow_mut();
        let mut cache = self.validity.borrow_mut();
        for m in rec.marks.iter_mut() {
            if m.role != Role::CommandWord {
                continue;
            }
            let end = m.end.min(line.len());
            let name = line.get(m.start..end).unwrap_or("");
            if cache.lookup(name, &mut shell) != Validity::Invalid {
                m.role = Role::Word;
            }
        }
    }
}

impl Completer for HuckHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        // #673: resolve over the whole command being typed. huck's completion is
        // parser-driven (#248), and a continuation line is a syntax error on its
        // own — `then whil` fails at `then`, so there is no cursor context and
        // nothing to complete. With the prefix it parses, and the offset comes
        // back into the visible line's coordinates, which is what rustyline
        // indexes.
        let prefix = self.prefix.borrow().clone();
        let mut shell = self.shell.borrow_mut();
        let (start, candidates) = if prefix.is_empty() {
            completion::dispatch::resolve(line, pos, &mut shell)
        } else {
            let source = format!("{prefix}{line}");
            let (start, candidates) =
                completion::dispatch::resolve(&source, prefix.len() + pos, &mut shell);
            if start < prefix.len() {
                // The word being completed STARTED on an earlier line — a
                // backslash continuation joins with no newline, so `echo /usr/b\`
                // then `in/l` is one word across two. rustyline can only replace
                // within the line it is showing, so there is no honest answer
                // here: replacing from column zero would drop the half above.
                // Offer nothing rather than mangle the line.
                (pos, Vec::new())
            } else {
                (start - prefix.len(), candidates)
            }
        };
        let pairs = candidates
            .into_iter()
            .map(|c: Candidate| Pair {
                display: c.display,
                replacement: c.replacement,
                // c.kind dropped — rustyline doesn't model completion kinds.
            })
            .collect();
        Ok((start, pairs))
    }
}

impl Hinter for HuckHelper {
    type Hint = String;
}

impl Highlighter for HuckHelper {
    /// v363 (#666): colour the edit buffer.
    ///
    /// Re-parses the line on every call. Measured at 4-18 us for a realistic
    /// line — against a 16 ms frame, ~800x headroom — so this is synchronous
    /// with no debounce and no incremental reparse. An INCOMPLETE line, which is
    /// what this sees on almost every keystroke, is the cheapest case of all
    /// (1.1-5.3 us) because parsing stops at the error.
    ///
    /// ⚠️ Aliases are passed EMPTY on purpose. Read-time alias expansion would
    /// splice in tokens whose spans point into the alias BODY rather than the
    /// typed line, so every offset after an alias would be wrong. Highlighting
    /// shows what was typed.
    ///
    /// The parse result is discarded; only the recorded marks are used. A parse
    /// ERROR is the normal case here, not a failure.
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> std::borrow::Cow<'l, str> {
        if line.is_empty() || !self.colour_now() {
            return std::borrow::Cow::Borrowed(line);
        }
        let mut rec = self.highlight_record(line);
        Self::emphasise_pairs(&mut rec, pos);
        std::borrow::Cow::Owned(crate::paint::render(line, &rec, true))
    }

    /// ⚠️ Defaults to FALSE in the trait — without this override rustyline never
    /// calls `highlight` again after the first render, so nothing updates as you
    /// type. Returning true unconditionally is what a 4-18 us parse buys.
    fn highlight_char(&self, line: &str, _pos: usize, kind: CmdKind) -> bool {
        // Only when the TEXT may have changed. Syntax colour is a function of
        // the line, not of where the cursor sits, so a bare cursor move needs no
        // repaint — and asking for one is not free: `true` here makes rustyline
        // do a FULL-LINE refresh, which on a 1-core box took the interactive pty
        // suite from 50 s to 92 s and timed four of its multiline rows out.
        //
        // Bracket matching (#666, Task 6) is the one thing that DOES depend on
        // the cursor, so `MoveCursor` is answered by asking whether this line
        // could possibly have a pair in it. That byte scan is a conservative
        // SUPERSET — it says yes to `echo "hi"` and to a lone `(` alike — and it
        // is not a second notion of what a pair is, because nothing is decided
        // from it: a line that passes still gets the real parse, and a line that
        // fails has no pair for the cursor to touch.
        if !self.colour_now() {
            return false;
        }
        if kind != CmdKind::MoveCursor {
            return true;
        }
        line.bytes().any(|b| {
            matches!(
                b,
                b'(' | b')' | b'{' | b'}' | b'[' | b']' | b'"' | b'\'' | b'`'
            )
        })
    }
}

impl Validator for HuckHelper {}

impl Helper for HuckHelper {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The roles a line ends up with, as `(text, role)` pairs.
    fn roles(line: &str) -> Vec<(String, huck_engine::highlight::Role)> {
        let helper = HuckHelper::new(Rc::new(RefCell::new(Shell::new())));
        helper
            .highlight_record(line)
            .marks
            .into_iter()
            .map(|m| (line[m.start..m.end.min(line.len())].to_string(), m.role))
            .collect()
    }

    #[test]
    fn a_command_that_resolves_is_left_alone() {
        use huck_engine::highlight::Role;
        // fish's restraint: an ordinary working line carries no command colour
        // at all, so the one command that will NOT run stands out.
        let r = roles("echo hi");
        assert!(
            !r.iter().any(|(_, role)| *role == Role::CommandWord),
            "a resolvable command must not keep the paint-me role: {r:?}"
        );
    }

    #[test]
    fn a_command_that_does_not_resolve_keeps_the_role() {
        use huck_engine::highlight::Role;
        let r = roles("nosuchcmd_xyz hi");
        assert!(
            r.iter()
                .any(|(t, role)| t == "nosuchcmd_xyz" && *role == Role::CommandWord),
            "an unresolvable command must be marked: {r:?}"
        );
        // ...and the same inside a substitution, which is where it is easiest to
        // miss a typo.
        let r = roles("echo $(nosuchcmd_xyz)");
        assert!(
            r.iter()
                .any(|(t, role)| t == "nosuchcmd_xyz" && *role == Role::CommandWord),
            "a command inside a substitution is checked too: {r:?}"
        );
        assert!(
            !r.iter()
                .any(|(t, role)| t == "echo" && *role == Role::CommandWord),
            "...while the outer, valid command stays plain: {r:?}"
        );
    }

    #[test]
    fn a_partly_typed_command_reads_as_invalid() {
        use huck_engine::highlight::Role;
        // Pinned deliberately, because it is what a user SEES: `ech` is not a
        // command, so it is red until the `o` lands. fish behaves the same way,
        // and the alternative — waiting for a word boundary — means the signal
        // arrives after the mistake is already made.
        let r = roles("ech");
        assert!(
            r.iter()
                .any(|(t, role)| t == "ech" && *role == Role::CommandWord),
            "{r:?}"
        );
    }

    /// The roles a line ends up with when the cursor sits at `pos`.
    fn roles_at(line: &str, pos: usize) -> Vec<(String, huck_engine::highlight::Role)> {
        let helper = HuckHelper::new(Rc::new(RefCell::new(Shell::new())));
        let mut rec = helper.highlight_record(line);
        HuckHelper::emphasise_pairs(&mut rec, pos);
        rec.marks
            .into_iter()
            .map(|m| (line[m.start..m.end.min(line.len())].to_string(), m.role))
            .collect()
    }

    #[test]
    fn both_ends_of_the_pair_under_the_cursor_are_emphasised() {
        // `echo $(date)` — the `$(` is at 5..7, the `)` at 11.
        let ends = |pos: usize| matched("echo $(date)", pos);
        // On the closer, and just past it — both ends light up either way.
        assert_eq!(ends(11), vec!["$(".to_string(), ")".to_string()]);
        assert_eq!(ends(12), vec!["$(".to_string(), ")".to_string()]);
        // On the opener, and on its second character.
        assert_eq!(ends(5), vec!["$(".to_string(), ")".to_string()]);
        assert_eq!(ends(6), vec!["$(".to_string(), ")".to_string()]);
        // Anywhere else, nothing — this is the half a pty test can actually see
        // move when an arrow key is pressed.
        assert!(
            ends(9).is_empty(),
            "cursor in the middle emphasises nothing"
        );
        assert!(ends(0).is_empty());
    }

    /// The text of every `PairMatch` mark, in record order.
    fn matched(line: &str, pos: usize) -> Vec<String> {
        use huck_engine::highlight::Role;
        roles_at(line, pos)
            .into_iter()
            .filter(|(_, r)| *r == Role::PairMatch)
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn a_whole_delimiter_is_emphasised_not_its_first_character() {
        // ⚠️ Reported from USING the shell. Emphasising ONE character lit the `$`
        // of `$(` and left the bracket plain — and `${x}` was worse, lighting the
        // `x`, because a parameter expansion's frame is pushed AFTER its opener
        // is consumed and so records where the BODY starts, not the construct.
        assert_eq!(
            matched("echo ${x}", 8),
            vec!["${".to_string(), "}".to_string()]
        );
        assert_eq!(
            matched("echo $((1+2))", 12),
            vec!["$((".to_string(), "))".to_string()],
            "a two-character closer is emphasised whole too"
        );
        assert_eq!(
            matched("echo `date`", 10),
            vec!["`".to_string(), "`".to_string()]
        );
        // A quote is one character and stays one: its region mark spans the whole
        // string, which is not a delimiter.
        assert_eq!(
            matched("echo \"dq\"", 8),
            vec!["\"".to_string(), "\"".to_string()]
        );
    }

    #[test]
    fn a_pair_with_no_width_is_not_emphasised() {
        // `${x:-d}` pushes an OPERAND frame whose two ends are the same offset;
        // emphasising it lit a lone character with no partner.
        for pos in 0..12 {
            let m = matched("echo ${x:-d}", pos);
            assert!(
                m.is_empty() || m == vec!["${".to_string(), "}".to_string()],
                "pos {pos} emphasised {m:?}"
            );
        }
    }

    #[test]
    fn the_inner_pair_is_the_one_that_matches() {
        // `echo "$(date)"` — quotes at 5 and 13, `$(` at 6..8, `)` at 12. With
        // the cursor on the inner closer only the inner pair answers, which is
        // what makes nesting readable.
        assert_eq!(
            matched("echo \"$(date)\"", 12),
            vec!["$(".to_string(), ")".to_string()]
        );
    }

    #[test]
    fn a_dangling_opener_is_marked_wherever_the_cursor_is() {
        use huck_engine::highlight::Role;
        // Unlike the pair match this does not depend on the cursor: a construct
        // left open is worth showing whether or not you are looking at it.
        for pos in [0, 6, 9] {
            let marked: Vec<String> = roles_at("echo \"abc", pos)
                .into_iter()
                .filter(|(_, r)| *r == Role::DanglingOpener)
                .map(|(t, _)| t)
                .collect();
            assert_eq!(marked, vec!["\"".to_string()], "cursor at {pos}");
        }
        // A finished line has none.
        assert!(
            !roles_at("echo \"abc\"", 0)
                .iter()
                .any(|(_, r)| *r == Role::DanglingOpener)
        );
    }

    /// Roles for `line` typed at a PS2 prompt, continuing `prefix`.
    fn roles_after(prefix: &str, line: &str) -> Vec<(String, huck_engine::highlight::Role)> {
        let helper = HuckHelper::new(Rc::new(RefCell::new(Shell::new())));
        helper.set_line_prefix(prefix.to_string());
        helper
            .highlight_record(line)
            .marks
            .into_iter()
            .map(|m| (line[m.start..m.end.min(line.len())].to_string(), m.role))
            .collect()
    }

    #[test]
    fn a_continuation_line_is_parsed_with_what_it_continues() {
        use huck_engine::highlight::Role;
        // #670: `then nosuchcmd_xyz` is a syntax error ON ITS OWN — the parse
        // fails at `then` and nothing after it is ever scanned, so the line came
        // back almost entirely unpainted. With the prefix it parses, and every
        // word gets the role it deserves.
        let plain = roles("then nosuchcmd_xyz");
        assert!(
            !plain
                .iter()
                .any(|(t, r)| t == "nosuchcmd_xyz" && *r == Role::CommandWord),
            "without the prefix the line cannot be parsed this far: {plain:?}"
        );
        let cont = roles_after("if true\n", "then nosuchcmd_xyz");
        assert_eq!(
            cont,
            vec![
                ("then".to_string(), Role::Keyword),
                ("nosuchcmd_xyz".to_string(), Role::CommandWord),
            ],
        );
    }

    #[test]
    fn marks_are_rebased_onto_the_visible_line_only() {
        use huck_engine::highlight::Role;
        // Offsets in the record index what was PARSED; the painter indexes what
        // is on SCREEN. Every mark here must fall inside the second line.
        let line = "then echo \"dq\" $HOME";
        for (text, _) in roles_after("if true\n", line) {
            assert!(
                line.contains(&text),
                "{text:?} is not from the visible line"
            );
        }
        assert!(
            roles_after("if true\n", line)
                .iter()
                .any(|(t, r)| t == "$HOME" && *r == Role::VarName),
            "the variable on the continuation line is marked"
        );
    }

    #[test]
    fn a_region_opened_on_an_earlier_line_paints_from_column_zero() {
        use huck_engine::highlight::Role;
        // A `"` opened on the previous line is still open here, so this line is
        // INSIDE the string up to its close. The mark straddles the boundary and
        // must be CLAMPED, not dropped — dropping it would leave the string
        // unpainted on exactly the line where the closing quote is.
        let line = "end\" after";
        let r = roles_after("echo \"start\n", line);
        let quoted: Vec<_> = r
            .iter()
            .filter(|(_, role)| *role == Role::QuotedDouble)
            .collect();
        assert_eq!(
            quoted.len(),
            1,
            "the run that closes on this line is painted: {r:?}"
        );
        assert_eq!(
            quoted[0].0, "end\"",
            "from column zero to the closing quote"
        );
    }

    #[test]
    fn a_pair_with_an_end_above_the_line_is_not_emphasised() {
        use huck_engine::highlight::Role;
        // A pair is a RELATION between two points. Emphasising the end that is
        // on screen would point the eye at a character whose partner it cannot
        // see, so a pair is kept only when both ends are visible.
        let line = "end\" after";
        for pos in 0..line.len() {
            let helper = HuckHelper::new(Rc::new(RefCell::new(Shell::new())));
            helper.set_line_prefix("echo \"start\n".to_string());
            let mut rec = helper.highlight_record(line);
            HuckHelper::emphasise_pairs(&mut rec, pos);
            assert!(
                !rec.marks.iter().any(|m| m.role == Role::PairMatch),
                "cursor {pos} emphasised half a pair"
            );
        }
    }

    #[test]
    fn helper_holds_rc_refcell_shell() {
        use std::cell::RefCell;
        let shell = Rc::new(RefCell::new(Shell::new()));
        let helper = HuckHelper::new(Rc::clone(&shell));
        // Mutate shell through the cell; helper must see the change live.
        shell.borrow_mut().set("MY_VAR", "hello".to_string());
        let history = rustyline::history::FileHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, pairs) =
            rustyline::completion::Completer::complete(&helper, "echo $MY_V", 10, &ctx).unwrap();
        assert_eq!(start, 6);
        let replacements: Vec<&str> = pairs.iter().map(|p| p.replacement.as_str()).collect();
        // The replacement carries bash's post-completion trailing space (#42);
        // the point of this test is that the live var is visible to the helper.
        assert!(
            pairs.iter().any(|p| p.replacement == "MY_VAR "),
            "live var not visible to helper: {replacements:?}"
        );
    }
}
