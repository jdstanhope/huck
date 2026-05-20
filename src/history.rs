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

    /// Reads the histfile into `entries`, keeping the most recent `max`
    /// lines. A missing file loads as empty history. Other I/O errors
    /// print a warning and leave history empty.
    pub fn load(&mut self) {
        let Some(path) = &self.file else { return };
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let mut lines: Vec<String> =
                    contents.lines().map(|l| l.to_string()).collect();
                if lines.len() > self.max {
                    lines.drain(0..lines.len() - self.max);
                }
                self.entries = lines;
                self.base_number = 1;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("huck: warning: could not read history file: {e}");
            }
        }
    }

    /// Writes `entries` to the histfile, one command per line, overwriting.
    /// A write error prints a warning; it never aborts the shell.
    pub fn save(&self) {
        let Some(path) = &self.file else { return };
        let mut out = String::new();
        for entry in &self.entries {
            out.push_str(entry);
            out.push('\n');
        }
        if let Err(e) = std::fs::write(path, out) {
            eprintln!("huck: warning: could not write history file: {e}");
        }
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

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hist");

        let writer = History {
            entries: vec!["one".to_string(), "two".to_string(), "three".to_string()],
            base_number: 1,
            max: 1000,
            file: Some(path.clone()),
        };
        writer.save();

        let mut reader = History {
            entries: Vec::new(),
            base_number: 1,
            max: 1000,
            file: Some(path.clone()),
        };
        reader.load();
        let collected: Vec<(usize, &str)> = reader.entries().collect();
        assert_eq!(collected, vec![(1, "one"), (2, "two"), (3, "three")]);
    }

    #[test]
    fn load_missing_file_is_empty_no_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does_not_exist");
        let mut h = History {
            entries: Vec::new(),
            base_number: 1,
            max: 1000,
            file: Some(path),
        };
        h.load();
        assert_eq!(h.last(), None);
    }

    #[test]
    fn load_truncates_to_max_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hist");
        std::fs::write(&path, "c1\nc2\nc3\nc4\nc5\n").unwrap();

        let mut h = History {
            entries: Vec::new(),
            base_number: 1,
            max: 3,
            file: Some(path),
        };
        h.load();
        let collected: Vec<(usize, &str)> = h.entries().collect();
        assert_eq!(collected, vec![(1, "c3"), (2, "c4"), (3, "c5")]);
    }

    #[test]
    fn load_and_save_no_op_when_file_is_none() {
        let mut h = History { entries: Vec::new(), base_number: 1, max: 1000, file: None };
        h.load();
        h.save();
        assert_eq!(h.last(), None);
    }
}
