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
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self {
        let colour_enabled = std::io::IsTerminal::is_terminal(&std::io::stdout())
            && std::env::var_os("NO_COLOR").is_none();
        Self {
            shell,
            colour_enabled,
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
        let no_aliases = std::collections::HashMap::new();
        let opts = huck_engine::lexer::LexerOptions {
            record_highlight: true,
            ..Default::default()
        };
        let mut lx = huck_engine::lexer::Lexer::new(line, &no_aliases, opts);
        let _ = huck_engine::parser::parse_sequence(&mut lx);
        let rec = lx.take_highlight_record();
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
