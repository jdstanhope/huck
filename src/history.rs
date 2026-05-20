//! Command history storage and `!`-style history expansion.

use std::path::PathBuf;

const HISTORY_MAX: usize = 1000;

/// A history-expansion failure. The shell prints these and refuses to
/// run the offending line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistError {
    /// A referenced event (`!foo`, `!99`, `!-99`, `!!`) does not exist.
    EventNotFound(String),
    /// A `^old^new^` substitution failed (no previous command, or `old`
    /// not found in it).
    Substitution(String),
}

impl std::fmt::Display for HistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EventNotFound(t) => write!(f, "{t}: event not found"),
            Self::Substitution(o) => write!(f, "{o}: substitution failed"),
        }
    }
}

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

/// Expands history references in `line`. Returns `Ok(None)` if nothing
/// changed, `Ok(Some(expanded))` if at least one reference expanded, or
/// `Err` if a referenced event could not be resolved.
pub fn expand(line: &str, history: &History) -> Result<Option<String>, HistError> {
    if !line.contains('!') {
        return Ok(None);
    }
    scan(line, history)
}

/// Walks the line, tracking quote state, and replaces `!`-references.
fn scan(line: &str, history: &History) -> Result<Option<String>, HistError> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut expanded = false;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                out.push('\\');
                if i + 1 < chars.len() {
                    out.push(chars[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                out.push('\'');
                i += 1;
            }
            '"' if !in_single => {
                in_double = !in_double;
                out.push('"');
                i += 1;
            }
            '!' if !in_single => {
                let next = chars.get(i + 1).copied();
                match next {
                    None => {
                        out.push('!');
                        i += 1;
                    }
                    Some(n) if n.is_whitespace() || n == '=' || n == '(' => {
                        out.push('!');
                        i += 1;
                    }
                    Some(_) => match read_event(&chars, i, history)? {
                        Some((text, consumed)) => {
                            out.push_str(&text);
                            i += consumed;
                            expanded = true;
                        }
                        None => {
                            out.push('!');
                            i += 1;
                        }
                    },
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }

    if expanded { Ok(Some(out)) } else { Ok(None) }
}

/// Reads one `!`-event starting at `chars[start]` (which is `!`). Returns
/// `Some((replacement, chars_consumed))` on a recognized reference,
/// `None` if the form is not a recognized trigger (caller emits a literal
/// `!`), or `Err` if a recognized reference failed to resolve.
fn read_event(
    chars: &[char],
    start: usize,
    history: &History,
) -> Result<Option<(String, usize)>, HistError> {
    let after = start + 1;
    match chars.get(after).copied() {
        Some('!') => {
            let text = history
                .last()
                .ok_or_else(|| HistError::EventNotFound("!!".to_string()))?;
            Ok(Some((text.to_string(), 2)))
        }
        Some('-') => {
            let mut j = after + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j == after + 1 {
                // `!-` not followed by digits — not a trigger.
                return Ok(None);
            }
            let n: usize = chars[after + 1..j].iter().collect::<String>().parse().unwrap();
            let token: String = chars[start..j].iter().collect();
            let last_num = history
                .last_number()
                .ok_or_else(|| HistError::EventNotFound(token.clone()))?;
            let target = (last_num + 1).checked_sub(n);
            match target.and_then(|t| history.get(t)) {
                Some(s) => Ok(Some((s.to_string(), j - start))),
                None => Err(HistError::EventNotFound(token)),
            }
        }
        Some(d) if d.is_ascii_digit() => {
            let mut j = after;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let n: usize = chars[after..j].iter().collect::<String>().parse().unwrap();
            let token: String = chars[start..j].iter().collect();
            match history.get(n) {
                Some(s) => Ok(Some((s.to_string(), j - start))),
                None => Err(HistError::EventNotFound(token)),
            }
        }
        Some('$') => {
            let prev = history
                .last()
                .ok_or_else(|| HistError::EventNotFound("!$".to_string()))?;
            let words: Vec<&str> = prev.split_whitespace().collect();
            let word = words.last().copied().unwrap_or("");
            Ok(Some((word.to_string(), 2)))
        }
        Some('^') => {
            let prev = history
                .last()
                .ok_or_else(|| HistError::EventNotFound("!^".to_string()))?;
            let words: Vec<&str> = prev.split_whitespace().collect();
            let word = words.get(1).copied().unwrap_or("");
            Ok(Some((word.to_string(), 2)))
        }
        Some('*') => {
            let prev = history
                .last()
                .ok_or_else(|| HistError::EventNotFound("!*".to_string()))?;
            let words: Vec<&str> = prev.split_whitespace().collect();
            let joined = if words.len() > 1 {
                words[1..].join(" ")
            } else {
                String::new()
            };
            Ok(Some((joined, 2)))
        }
        Some(_) => {
            // !string — prefix search.
            let mut j = after;
            while j < chars.len() {
                let c = chars[j];
                if c.is_whitespace() || c == '\'' || c == '"' || c == '!' {
                    break;
                }
                j += 1;
            }
            let needle: String = chars[after..j].iter().collect();
            let token: String = chars[start..j].iter().collect();
            match history.search_prefix(&needle) {
                Some(s) => Ok(Some((s.to_string(), j - start))),
                None => Err(HistError::EventNotFound(token)),
            }
        }
        None => Ok(None),
    }
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

    fn hist_with(cmds: &[&str]) -> History {
        let mut h = History { entries: Vec::new(), base_number: 1, max: 1000, file: None };
        for c in cmds {
            h.add(c.to_string());
        }
        h
    }

    #[test]
    fn expand_no_bang_is_noop() {
        let h = hist_with(&["echo hi"]);
        assert_eq!(expand("ls -l", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_bang_is_previous_command() {
        let h = hist_with(&["echo one", "ls -l"]);
        assert_eq!(expand("!!", &h).unwrap(), Some("ls -l".to_string()));
    }

    #[test]
    fn expand_bang_bang_embedded_in_line() {
        let h = hist_with(&["ls -l"]);
        assert_eq!(expand("sudo !!", &h).unwrap(), Some("sudo ls -l".to_string()));
    }

    #[test]
    fn expand_bang_n_absolute() {
        let h = hist_with(&["first", "second", "third"]);
        assert_eq!(expand("!2", &h).unwrap(), Some("second".to_string()));
    }

    #[test]
    fn expand_bang_n_out_of_range_errors() {
        let h = hist_with(&["only"]);
        assert!(matches!(expand("!9", &h).unwrap_err(), HistError::EventNotFound(_)));
    }

    #[test]
    fn expand_bang_minus_n() {
        let h = hist_with(&["first", "second", "third"]);
        assert_eq!(expand("!-1", &h).unwrap(), Some("third".to_string()));
        assert_eq!(expand("!-2", &h).unwrap(), Some("second".to_string()));
    }

    #[test]
    fn expand_bang_minus_n_out_of_range_errors() {
        let h = hist_with(&["one"]);
        assert!(matches!(expand("!-5", &h).unwrap_err(), HistError::EventNotFound(_)));
    }

    #[test]
    fn expand_bang_bang_no_history_errors() {
        let h = hist_with(&[]);
        assert!(matches!(expand("!!", &h).unwrap_err(), HistError::EventNotFound(_)));
    }

    #[test]
    fn expand_bang_before_whitespace_is_literal() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("echo ! hi", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_before_equals_is_literal() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("x != y", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_at_end_of_line_is_literal() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("echo hi!", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_inside_single_quotes_is_literal() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("echo '!!'", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_inside_double_quotes_still_expands() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("echo \"!!\"", &h).unwrap(), Some("echo \"prev\"".to_string()));
    }

    #[test]
    fn expand_single_quote_inside_double_quote_not_a_quote_region() {
        let h = hist_with(&["prev"]);
        assert_eq!(
            expand("echo \"it's !!\"", &h).unwrap(),
            Some("echo \"it's prev\"".to_string())
        );
    }

    #[test]
    fn expand_escaped_bang_is_literal() {
        let h = hist_with(&["prev"]);
        assert_eq!(expand("echo \\!!", &h).unwrap(), None);
    }

    #[test]
    fn expand_bang_dollar_is_last_word() {
        let h = hist_with(&["ls -l /tmp"]);
        assert_eq!(expand("echo !$", &h).unwrap(), Some("echo /tmp".to_string()));
    }

    #[test]
    fn expand_bang_caret_is_first_argument() {
        let h = hist_with(&["ls -l /tmp"]);
        assert_eq!(expand("echo !^", &h).unwrap(), Some("echo -l".to_string()));
    }

    #[test]
    fn expand_bang_star_is_all_arguments() {
        let h = hist_with(&["ls -l /tmp /var"]);
        assert_eq!(expand("echo !*", &h).unwrap(), Some("echo -l /tmp /var".to_string()));
    }

    #[test]
    fn expand_bang_dollar_single_word_command() {
        let h = hist_with(&["pwd"]);
        assert_eq!(expand("echo !$", &h).unwrap(), Some("echo pwd".to_string()));
    }

    #[test]
    fn expand_bang_caret_no_arguments_is_empty() {
        let h = hist_with(&["pwd"]);
        assert_eq!(expand("echo !^", &h).unwrap(), Some("echo ".to_string()));
    }

    #[test]
    fn expand_bang_star_no_arguments_is_empty() {
        let h = hist_with(&["pwd"]);
        assert_eq!(expand("echo !*", &h).unwrap(), Some("echo ".to_string()));
    }

    #[test]
    fn expand_bang_dollar_no_history_errors() {
        let h = hist_with(&[]);
        assert!(matches!(expand("echo !$", &h).unwrap_err(), HistError::EventNotFound(_)));
    }

    #[test]
    fn expand_bang_string_prefix_search() {
        let h = hist_with(&["echo one", "ls -l", "echo two"]);
        assert_eq!(expand("!echo", &h).unwrap(), Some("echo two".to_string()));
    }

    #[test]
    fn expand_bang_string_no_match_errors() {
        let h = hist_with(&["ls -l"]);
        assert!(matches!(expand("!nope", &h).unwrap_err(), HistError::EventNotFound(_)));
    }

    #[test]
    fn expand_bang_string_stops_at_whitespace() {
        let h = hist_with(&["make build"]);
        assert_eq!(expand("!make again", &h).unwrap(), Some("make build again".to_string()));
    }
}
