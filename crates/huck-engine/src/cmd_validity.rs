//! Is this name a command the shell could run? (v363, #666)
//!
//! The highlighter asks this on every keystroke, for every command word on the
//! line, which makes it a different question from the one the executor asks.
//! The executor asks once, about a name someone meant; the highlighter asks
//! about every PREFIX of that name as it is typed — of `g`, `gi`, `git`, two are
//! misses, and a miss walks every `PATH` segment. Measured on this box: 90-160
//! microseconds for one miss, and ~940 for a six-stage pipeline of unknown
//! words, per keystroke.
//!
//! Hence a cache that remembers MISSES as well as hits — the command hash table
//! (#655) cannot serve here, because it only ever holds names that were
//! actually run.
//!
//! Two properties matter more than the speed:
//!
//! * it never writes to the hash table (`HashEffect::Discard`), so merely typing
//!   a name cannot change what `hash` prints;
//! * it never blocks the editor. A `PATH` on a hung network mount would make
//!   every keystroke wait; past a time budget this stops searching and answers
//!   `Unknown`, and an `Unknown` word is simply left unpainted.

use crate::builtins::{self, HashEffect, PathClassify};
use crate::shell_state::Shell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Past this, a single lookup is judged too slow to be doing while someone
/// types, and the cache stops searching until it is cleared. Generous: a local
/// `PATH` miss measures well under a millisecond.
const SEARCH_BUDGET: Duration = Duration::from_millis(20);

/// What the cache can say about a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    /// Resolves — an alias, function, builtin, or a file on `PATH`.
    Valid,
    /// Does not resolve. This is the only answer that paints anything.
    Invalid,
    /// Not established, and deliberately not established: the search budget was
    /// blown. Treated like `Valid` by the painter, because guessing "invalid"
    /// about a name we did not look up would be a lie in red.
    Unknown,
}

/// Positive AND negative command-validity cache.
///
/// Cleared at every prompt, which bounds staleness to a single line: install a
/// program in one line and the next line sees it.
#[derive(Debug, Default)]
pub struct ValidityCache {
    seen: HashMap<String, bool>,
    /// Set when a lookup blew the budget. Sticky until `clear` — one slow
    /// segment makes every subsequent search slow too, so retrying per keystroke
    /// would just re-pay the cost.
    degraded: bool,
}

impl ValidityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything. Called at each prompt.
    pub fn clear(&mut self) {
        self.seen.clear();
        self.degraded = false;
    }

    /// True once a lookup has exceeded the budget (visible for tests).
    pub fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub fn lookup(&mut self, name: &str, shell: &mut Shell) -> Validity {
        if name.is_empty() {
            return Validity::Unknown;
        }
        if let Some(&ok) = self.seen.get(name) {
            return if ok {
                Validity::Valid
            } else {
                Validity::Invalid
            };
        }
        if self.degraded {
            return Validity::Unknown;
        }
        let started = Instant::now();
        let ok = resolves(name, shell);
        if started.elapsed() > SEARCH_BUDGET {
            // Record the answer we did pay for, then stop searching.
            self.degraded = true;
        }
        self.seen.insert(name.to_string(), ok);
        if ok {
            Validity::Valid
        } else {
            Validity::Invalid
        }
    }
}

/// The shell's own resolution order, minus the keywords — a reserved word never
/// reaches here, because the parser has already marked it as one.
fn resolves(name: &str, shell: &mut Shell) -> bool {
    // A name with a slash is a PATH, not a name to search for. Neither shell
    // hashes one, and `resolve_for_exec` asserts as much.
    if name.contains('/') {
        return builtins::is_executable_file(std::path::Path::new(name));
    }
    if shell.aliases.contains_key(name)
        || shell.functions.contains_key(name)
        || builtins::is_builtin(name)
    {
        return true;
    }
    matches!(
        builtins::resolve_for_exec(name, shell, HashEffect::Discard),
        PathClassify::Executable(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_builtin_and_a_path_command_are_valid_and_a_nonsense_name_is_not() {
        let mut shell = Shell::new();
        let mut cache = ValidityCache::new();
        assert_eq!(cache.lookup("echo", &mut shell), Validity::Valid);
        assert_eq!(cache.lookup("sh", &mut shell), Validity::Valid);
        assert_eq!(
            cache.lookup("nosuchcmd_xyz", &mut shell),
            Validity::Invalid,
            "a name that resolves nowhere is the one thing that paints"
        );
    }

    #[test]
    fn a_miss_is_cached_as_a_miss() {
        // The whole reason this exists rather than reusing the hash table: a
        // MISS must be remembered, or every keystroke of a name being typed
        // re-walks PATH.
        let mut shell = Shell::new();
        let mut cache = ValidityCache::new();
        assert_eq!(cache.lookup("nosuchcmd_xyz", &mut shell), Validity::Invalid);
        assert_eq!(cache.seen.get("nosuchcmd_xyz"), Some(&false));
        // ...and clearing forgets it, so a newly installed program is picked up.
        cache.clear();
        assert!(cache.seen.is_empty());
    }

    #[test]
    fn a_function_and_an_alias_resolve() {
        let mut shell = Shell::new();
        shell
            .aliases
            .insert("myalias_xyz".to_string(), "echo hi".to_string());
        let mut cache = ValidityCache::new();
        assert_eq!(cache.lookup("myalias_xyz", &mut shell), Validity::Valid);
    }

    #[test]
    fn a_degraded_cache_answers_unknown_without_searching() {
        let mut shell = Shell::new();
        let mut cache = ValidityCache::new();
        cache.degraded = true;
        assert_eq!(cache.lookup("nosuchcmd_xyz", &mut shell), Validity::Unknown);
        assert!(
            cache.seen.is_empty(),
            "degraded must not search, so nothing is learned"
        );
        // Already-known names still answer from the cache while degraded.
        cache.seen.insert("known_xyz".to_string(), false);
        assert_eq!(cache.lookup("known_xyz", &mut shell), Validity::Invalid);
    }

    #[test]
    fn typing_a_name_does_not_touch_the_hash_table() {
        // `HashEffect::Discard`: highlighting must not change what `hash` prints.
        let mut shell = Shell::new();
        let before = shell.command_hash.len();
        let mut cache = ValidityCache::new();
        assert_eq!(cache.lookup("sh", &mut shell), Validity::Valid);
        assert_eq!(shell.command_hash.len(), before);
    }

    #[test]
    fn a_command_named_by_path_is_checked_as_a_file() {
        let mut shell = Shell::new();
        let mut cache = ValidityCache::new();
        assert_eq!(cache.lookup("/bin/sh", &mut shell), Validity::Valid);
        assert_eq!(
            cache.lookup("/nosuch/dir/nosuchcmd_xyz", &mut shell),
            Validity::Invalid
        );
    }
}
