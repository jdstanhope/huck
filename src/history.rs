//! Command history storage and `!`-style history expansion.

use std::path::PathBuf;

const HISTORY_MAX: usize = 1000;

/// In-memory command history with absolute entry numbering. The entry at
/// index `i` has display number `base_number + i`. When the cap is hit,
/// the oldest entry is dropped and `base_number` increments, so live
/// entries keep stable numbers.
#[derive(Debug, Clone)]
pub struct History {
    entries: Vec<String>,
    base_number: usize,
    max: usize,
    file: Option<PathBuf>,
}

impl History {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            base_number: 1,
            max: HISTORY_MAX,
            file: resolve_histfile(),
        }
    }

    /// Appends a command, evicting the oldest entries past the cap.
    pub fn add(&mut self, line: String) {
        self.entries.push(line);
        while self.entries.len() > self.max {
            self.entries.remove(0);
            self.base_number += 1;
        }
    }

    /// Looks up an entry by its absolute display number.
    pub fn get(&self, number: usize) -> Option<&str> {
        if number < self.base_number {
            return None;
        }
        self.entries.get(number - self.base_number).map(|s| s.as_str())
    }

    /// The most recent entry.
    pub fn last(&self) -> Option<&str> {
        self.entries.last().map(|s| s.as_str())
    }

    /// The absolute number of the most recent entry.
    pub fn last_number(&self) -> Option<usize> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.base_number + self.entries.len() - 1)
        }
    }

    /// The most recent entry that starts with `prefix`.
    pub fn search_prefix(&self, prefix: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.starts_with(prefix))
            .map(|s| s.as_str())
    }

    /// Iterates `(absolute_number, command)` oldest-first.
    pub fn entries(&self) -> impl Iterator<Item = (usize, &str)> {
        let base = self.base_number;
        self.entries
            .iter()
            .enumerate()
            .map(move |(i, s)| (base + i, s.as_str()))
    }

    /// Empties the list and resets numbering to 1.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.base_number = 1;
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolves the histfile path: `$HISTFILE`, else `$HOME/.huck_history`,
/// else `None` (persistence disabled).
fn resolve_histfile() -> Option<PathBuf> {
    if let Ok(hf) = std::env::var("HISTFILE") {
        if !hf.is_empty() {
            return Some(PathBuf::from(hf));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(PathBuf::from(home).join(".huck_history"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> History {
        History { entries: Vec::new(), base_number: 1, max: 1000, file: None }
    }

    #[test]
    fn add_and_get_by_number() {
        let mut h = empty();
        h.add("first".to_string());
        h.add("second".to_string());
        assert_eq!(h.get(1), Some("first"));
        assert_eq!(h.get(2), Some("second"));
        assert_eq!(h.get(3), None);
        assert_eq!(h.get(0), None);
    }

    #[test]
    fn last_and_last_number() {
        let mut h = empty();
        assert_eq!(h.last(), None);
        assert_eq!(h.last_number(), None);
        h.add("a".to_string());
        h.add("b".to_string());
        assert_eq!(h.last(), Some("b"));
        assert_eq!(h.last_number(), Some(2));
    }

    #[test]
    fn cap_eviction_bumps_base_number() {
        let mut h = History { entries: Vec::new(), base_number: 1, max: 3, file: None };
        for cmd in ["c1", "c2", "c3", "c4", "c5"] {
            h.add(cmd.to_string());
        }
        assert_eq!(h.get(1), None);
        assert_eq!(h.get(2), None);
        assert_eq!(h.get(3), Some("c3"));
        assert_eq!(h.get(5), Some("c5"));
        assert_eq!(h.last_number(), Some(5));
    }

    #[test]
    fn search_prefix_returns_most_recent_match() {
        let mut h = empty();
        h.add("echo one".to_string());
        h.add("ls -l".to_string());
        h.add("echo two".to_string());
        assert_eq!(h.search_prefix("echo"), Some("echo two"));
        assert_eq!(h.search_prefix("ls"), Some("ls -l"));
        assert_eq!(h.search_prefix("nope"), None);
    }

    #[test]
    fn entries_yields_numbered_pairs() {
        let mut h = empty();
        h.add("a".to_string());
        h.add("b".to_string());
        let collected: Vec<(usize, &str)> = h.entries().collect();
        assert_eq!(collected, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn clear_resets_entries_and_numbering() {
        let mut h = empty();
        h.add("a".to_string());
        h.add("b".to_string());
        h.clear();
        assert_eq!(h.last(), None);
        h.add("fresh".to_string());
        assert_eq!(h.get(1), Some("fresh"));
    }
}
