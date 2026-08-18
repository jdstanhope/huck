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
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self {
        let colour_enabled = std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none();
        Self {
            shell,
            colour_enabled,
            validity: RefCell::new(huck_engine::cmd_validity::ValidityCache::new()),
        }
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
    fn highlight_record(&self, line: &str) -> huck_engine::highlight::HighlightRecord {
        let no_aliases = std::collections::HashMap::new();
        let opts = huck_engine::lexer::LexerOptions {
            record_highlight: true,
            ..Default::default()
        };
        let mut lx = huck_engine::lexer::Lexer::new(line, &no_aliases, opts);
        let _ = huck_engine::parser::parse_sequence(&mut lx);
        let mut rec = lx.take_highlight_record();
        self.resolve_command_validity(line, &mut rec);
        rec
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
        let mut shell = self.shell.borrow_mut();
        let (start, candidates) = completion::dispatch::resolve(line, pos, &mut shell);
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
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        if !self.colour_enabled || line.is_empty() {
            return std::borrow::Cow::Borrowed(line);
        }
        let rec = self.highlight_record(line);
        std::borrow::Cow::Owned(crate::paint::render(line, &rec, true))
    }

    /// ⚠️ Defaults to FALSE in the trait — without this override rustyline never
    /// calls `highlight` again after the first render, so nothing updates as you
    /// type. Returning true unconditionally is what a 4-18 us parse buys.
    fn highlight_char(&self, _line: &str, _pos: usize, kind: CmdKind) -> bool {
        // Only when the TEXT may have changed. Syntax colour is a function of
        // the line, not of where the cursor sits, so a bare cursor move needs no
        // repaint — and asking for one is not free: `true` here makes rustyline
        // do a FULL-LINE refresh, which on a 1-core box took the interactive pty
        // suite from 50 s to 92 s and timed four of its multiline rows out.
        //
        // Task 6 (bracket matching) is the one thing that DOES depend on the
        // cursor; it will re-enable `MoveCursor` for that case specifically
        // rather than blanket-refreshing here.
        self.colour_enabled && kind != CmdKind::MoveCursor
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
