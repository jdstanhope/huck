//! rustyline completion adapter: `HuckHelper` wires huck-engine's
//! completion dispatch into rustyline's `Completer`/`Helper` traits.

use huck_engine::completion::{self, Candidate};
use huck_engine::shell_state::Shell;
use std::cell::RefCell;
use std::rc::Rc;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
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
}

impl HuckHelper {
    pub fn new(shell: Rc<RefCell<Shell>>) -> Self {
        Self { shell }
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

impl Highlighter for HuckHelper {}

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
        assert!(
            pairs.iter().any(|p| p.replacement == "MY_VAR"),
            "live var not visible to helper: {replacements:?}"
        );
    }
}
