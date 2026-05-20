# huck v13: Command History and History Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add persistent command history (histfile + `history` builtin) and `!`-style history expansion (`!!`, `!n`, `!-n`, `!string`, `!$`, `!^`, `!*`, `^old^new^`) to huck.

**Architecture:** A new `src/history.rs` module owns a `History` struct (in-memory entry list with absolute numbering, capped at 1000), the `HistError` type, and a free `expand` function. `History` is a field on `Shell`. The interactive loop in `src/shell.rs` loads the histfile at startup, runs `expand` on each raw input line, echoes the expanded form, records it, runs it, and saves the histfile on exit.

**Tech Stack:** Rust 2024 edition. No new dependencies (`tempfile` already a dev-dependency).

**Reference:** Design spec at `docs/superpowers/specs/2026-05-20-huck-history-design.md`.

---

## File Map

- **Create:** `src/history.rs` — `History`, `HistError`, `expand`
- **Create:** `tests/history_integration.rs` — end-to-end via shell binary
- **Modify:** `src/shell_state.rs` — `history: History` field on `Shell`
- **Modify:** `src/shell.rs` — load at startup, expand/echo/record in the loop, save on exit
- **Modify:** `src/builtins.rs` — `history` builtin + `is_builtin` + dispatch
- **Modify:** `src/main.rs` — register `mod history`
- **Modify:** `README.md` — v13 row, features, builtins list, test count

---

## Task 1: `History` struct and core methods

Create `src/history.rs` with the `History` struct and its non-persistence methods. Add a `history` field to `Shell`.

**Files:**
- Create: `src/history.rs`
- Modify: `src/main.rs`
- Modify: `src/shell_state.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/history.rs` with this content:

```rust
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
        // Only the last 3 survive: c3, c4, c5 with numbers 3, 4, 5.
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
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs`. Add `mod history;` alphabetically with the other `mod` declarations.

- [ ] **Step 3: Add the `history` field to `Shell`**

Edit `src/shell_state.rs`. Add a field to the `Shell` struct:

```rust
pub history: crate::history::History,
```

In `Shell::new()`, initialize it:

```rust
history: crate::history::History::new(),
```

`History` derives `Debug, Clone` so the existing `Shell` derives still hold.

- [ ] **Step 4: Run tests**

Run: `cargo test history::`
Expected: 6 tests pass.

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass (472 baseline + 6 new = 478).

- [ ] **Step 6: Commit**

```bash
git add src/history.rs src/main.rs src/shell_state.rs
git commit -m "v13 task 1: History struct with absolute numbering"
```

---

## Task 2: Histfile persistence

Add `load` and `save` methods to `History`.

**Files:**
- Modify: `src/history.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/history.rs`:

```rust
#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hist");

    let mut writer = History {
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
    // Most recent 3 lines kept, numbered from 1.
    assert_eq!(collected, vec![(1, "c3"), (2, "c4"), (3, "c5")]);
}

#[test]
fn load_and_save_no_op_when_file_is_none() {
    let mut h = History { entries: Vec::new(), base_number: 1, max: 1000, file: None };
    h.load();  // must not panic
    h.save();  // must not panic
    assert_eq!(h.last(), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test history::tests::save_then_load`
Expected: FAIL — `load`/`save` don't exist.

- [ ] **Step 3: Implement `load` and `save`**

Add these methods inside `impl History` in `src/history.rs`:

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test history::`
Expected: 10 tests pass (6 from Task 1 + 4 new).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/history.rs
git commit -m "v13 task 2: histfile load and save"
```

---

## Task 3: `history` builtin

Add a `history` builtin: `history` lists entries, `history -c` clears.

**Files:**
- Modify: `src/builtins.rs`

- [ ] **Step 1: Write failing tests**

The existing builtins are tested via `run_builtin`. Add to `src/builtins.rs` `#[cfg(test)] mod tests` (if there is no test module, create one):

```rust
#[test]
fn history_lists_numbered_entries() {
    let mut shell = Shell::new();
    shell.history.add("first cmd".to_string());
    shell.history.add("second cmd".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("history", &[], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("first cmd"), "output: {text}");
    assert!(text.contains("second cmd"), "output: {text}");
    assert!(text.contains("1"), "output should have numbers: {text}");
}

#[test]
fn history_dash_c_clears() {
    let mut shell = Shell::new();
    shell.history.add("doomed".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("history", &["-c".to_string()], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.history.last(), None);
}

#[test]
fn history_invalid_option_errors() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("history", &["--bogus".to_string()], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test history_lists_numbered_entries`
Expected: FAIL — `"history"` is not a recognized builtin (`run_builtin` hits `unreachable!`).

- [ ] **Step 3: Add `history` to `is_builtin`**

Edit `src/builtins.rs`. In the `is_builtin` `matches!` list, add `"history"`:

```rust
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "exit" | "pwd" | "echo" | "export" | "unset" | "jobs"
            | "wait" | "fg" | "bg" | "kill" | "disown" | "history"
    )
}
```

- [ ] **Step 4: Add the dispatch arm**

In `run_builtin`'s `match name`, add before the `_ =>` arm:

```rust
"history" => builtin_history(args, out, shell),
```

- [ ] **Step 5: Implement `builtin_history`**

Add this function in `src/builtins.rs` (alongside the other `builtin_*` functions):

```rust
fn builtin_history(
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match args.first().map(|s| s.as_str()) {
        None => {
            for (number, command) in shell.history.entries() {
                if writeln!(out, "{number:>5}\t{command}").is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
            ExecOutcome::Continue(0)
        }
        Some("-c") => {
            shell.history.clear();
            ExecOutcome::Continue(0)
        }
        Some(other) => {
            eprintln!("huck: history: {other}: invalid option");
            ExecOutcome::Continue(1)
        }
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test history`
Expected: builtin tests pass + the Task 1/2 `history::` tests still pass.

- [ ] **Step 7: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/builtins.rs
git commit -m "v13 task 3: history builtin (list and -c clear)"
```

---

## Task 4: History expansion — scanner skeleton + `!!` / `!n` / `!-n`

Add the `HistError` type and the `expand` function with the quoting-aware scanner. This task handles `!!`, `!n`, and `!-n`; other `!`-forms are treated as literal (no trigger) until Task 5.

**Files:**
- Modify: `src/history.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/history.rs`:

```rust
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
    // !-1 == last, !-2 == second-to-last.
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
    // The `'` chars are literal inside "...", so `!!` here IS inside the
    // double-quoted span (expands), not a single-quoted span.
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_no_bang_is_noop`
Expected: FAIL — `expand` and `HistError` don't exist.

- [ ] **Step 3: Add `HistError`**

Add to `src/history.rs` (near the top, after the `HISTORY_MAX` const):

```rust
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
```

- [ ] **Step 4: Implement `expand` and the scanner**

Add to `src/history.rs` (after the `impl History` block, before the test module):

```rust
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
        // `!$`, `!^`, `!*`, `!string` are added in Task 5.
        _ => Ok(None),
    }
}
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test history::`
Expected: all `history::` tests pass (10 from Tasks 1-2 + 15 new = 25).

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/history.rs
git commit -m "v13 task 4: history expansion scanner with !!, !n, !-n"
```

---

## Task 5: History expansion — `!$` / `!^` / `!*` / `!string`

Extend `read_event` to handle the word-shorthand designators and prefix search.

**Files:**
- Modify: `src/history.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/history.rs`:

```rust
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
    // Only one word — last word is the command itself.
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
    // `!make` resolves; the trailing ` again` is copied through.
    assert_eq!(expand("!make again", &h).unwrap(), Some("make build again".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_bang_dollar_is_last_word`
Expected: FAIL — `!$` currently falls into the `_ => Ok(None)` arm, so no expansion happens.

- [ ] **Step 3: Extend `read_event`**

In `src/history.rs`, replace the final `_ => Ok(None)` arm of `read_event` with these arms:

```rust
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
```

Note: the `None` arm (no character after `!`) cannot actually be reached here because `scan` already handles end-of-line `!` before calling `read_event` — but `read_event`'s `match` must stay exhaustive. Keep the `None => Ok(None)` arm.

- [ ] **Step 4: Run the new tests**

Run: `cargo test history::`
Expected: all `history::` tests pass (25 from Tasks 1-4 + 10 new = 35).

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/history.rs
git commit -m "v13 task 5: history expansion !\$, !^, !*, !string"
```

---

## Task 6: Quick substitution `^old^new^`

Add `^old^new^` handling to `expand`, recognized only when `^` is the first non-blank character of the line.

**Files:**
- Modify: `src/history.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/history.rs`:

```rust
#[test]
fn expand_quick_substitution_basic() {
    let h = hist_with(&["echo hello"]);
    assert_eq!(expand("^hello^world^", &h).unwrap(), Some("echo world".to_string()));
}

#[test]
fn expand_quick_substitution_trailing_caret_optional() {
    let h = hist_with(&["echo hello"]);
    assert_eq!(expand("^hello^world", &h).unwrap(), Some("echo world".to_string()));
}

#[test]
fn expand_quick_substitution_first_occurrence_only() {
    let h = hist_with(&["a a a"]);
    assert_eq!(expand("^a^X^", &h).unwrap(), Some("X a a".to_string()));
}

#[test]
fn expand_quick_substitution_leading_blanks_allowed() {
    let h = hist_with(&["echo hello"]);
    assert_eq!(expand("  ^hello^world^", &h).unwrap(), Some("echo world".to_string()));
}

#[test]
fn expand_quick_substitution_old_not_found_errors() {
    let h = hist_with(&["echo hello"]);
    assert!(matches!(
        expand("^missing^world^", &h).unwrap_err(),
        HistError::Substitution(_)
    ));
}

#[test]
fn expand_quick_substitution_no_history_errors() {
    let h = hist_with(&[]);
    assert!(matches!(
        expand("^a^b^", &h).unwrap_err(),
        HistError::Substitution(_)
    ));
}

#[test]
fn expand_caret_not_at_line_start_is_not_substitution() {
    // A `^` that isn't the first non-blank char is just a literal char.
    let h = hist_with(&["echo hello"]);
    assert_eq!(expand("echo a^b^c", &h).unwrap(), None);
}

#[test]
fn expand_quick_substitution_trailing_text_appended() {
    let h = hist_with(&["echo hello"]);
    assert_eq!(
        expand("^hello^world^ extra", &h).unwrap(),
        Some("echo world extra".to_string())
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_quick_substitution_basic`
Expected: FAIL — `^hello^world^` contains no `!`, so `expand` returns `Ok(None)`.

- [ ] **Step 3: Add quick-substitution handling to `expand`**

In `src/history.rs`, replace the body of `expand` with:

```rust
pub fn expand(line: &str, history: &History) -> Result<Option<String>, HistError> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('^') {
        return quick_substitution(line, history);
    }
    if !line.contains('!') {
        return Ok(None);
    }
    scan(line, history)
}

/// Handles `^old^new^` (or `^old^new`) quick substitution on the previous
/// command. `line`, after leading blanks, begins with `^`.
fn quick_substitution(
    line: &str,
    history: &History,
) -> Result<Option<String>, HistError> {
    let leading = line.len() - line.trim_start().len();
    // Skip the leading blanks and the first `^`.
    let body = &line[leading + 1..];
    let mut parts = body.splitn(3, '^');
    let old = parts.next().unwrap_or("");
    let new = match parts.next() {
        Some(n) => n,
        None => return Err(HistError::Substitution(old.to_string())),
    };
    let rest = parts.next().unwrap_or("");

    if old.is_empty() {
        return Err(HistError::Substitution(old.to_string()));
    }
    let prev = history
        .last()
        .ok_or_else(|| HistError::Substitution(old.to_string()))?;
    if !prev.contains(old) {
        return Err(HistError::Substitution(old.to_string()));
    }
    let replaced = prev.replacen(old, new, 1);
    Ok(Some(format!("{replaced}{rest}")))
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test history::`
Expected: all `history::` tests pass (35 from Tasks 1-5 + 8 new = 43).

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/history.rs
git commit -m "v13 task 6: quick substitution ^old^new^"
```

---

## Task 7: REPL integration

Wire history expansion, recording, and persistence into the interactive loop in `src/shell.rs`.

**Files:**
- Modify: `src/shell.rs`

- [ ] **Step 1: Inspect the current loop**

Read `src/shell.rs`. The current `run` loop reads a line, calls `editor.add_history_entry`, and runs `process_line`. The exit paths are `ExecOutcome::Exit(code)` (returns `code`) and `Err(ReadlineError::Eof)` (returns `shell.last_status()`).

- [ ] **Step 2: Load history at startup**

In `run`, after `let mut shell = Shell::new();` and the signal-handler installs, add:

```rust
    shell.history.load();
    for (_, command) in shell.history.entries() {
        let _ = editor.add_history_entry(command);
    }
```

This makes loaded commands available to rustyline arrow-up recall.

- [ ] **Step 3: Replace the readline-handling block**

The current `Ok(line)` arm looks roughly like:

```rust
Ok(line) => {
    if !line.trim().is_empty() {
        let _ = editor.add_history_entry(line.as_str());
    }
    match process_line(&line, &mut shell) {
        ExecOutcome::Exit(code) => return code,
        ExecOutcome::Continue(status) => shell.set_last_status(status),
    }
}
```

Replace it with:

```rust
Ok(line) => {
    let to_run = match crate::history::expand(&line, &shell.history) {
        Ok(None) => line.clone(),
        Ok(Some(expanded)) => {
            println!("{expanded}");
            expanded
        }
        Err(e) => {
            eprintln!("huck: {e}");
            shell.set_last_status(1);
            continue;
        }
    };
    if !to_run.trim().is_empty() {
        shell.history.add(to_run.clone());
        let _ = editor.add_history_entry(to_run.as_str());
    }
    match process_line(&to_run, &mut shell) {
        ExecOutcome::Exit(code) => {
            shell.history.save();
            return code;
        }
        ExecOutcome::Continue(status) => shell.set_last_status(status),
    }
}
```

- [ ] **Step 4: Save history on the EOF exit path**

The `Err(ReadlineError::Eof)` arm currently returns `shell.last_status()`. Change it to save first:

```rust
Err(ReadlineError::Eof) => {
    shell.history.save();
    return shell.last_status();
}
```

- [ ] **Step 5: Build and run the full suite**

Run: `cargo build && cargo test`
Expected: all tests pass (the existing `shell.rs` behavior tests, if any, still pass; history unit tests unaffected).

- [ ] **Step 6: Manual smoke test**

```bash
cargo build --release
HISTFILE=/tmp/huck_smoke_hist
rm -f "$HISTFILE"
HISTFILE=/tmp/huck_smoke_hist ~/projects/shuck/target/release/huck <<'EOF'
echo one
echo two
!!
!1
echo !$
^two^three^
history
!nope
history -c
history
exit
EOF
echo "--- histfile contents ---"
cat /tmp/huck_smoke_hist
rm -f /tmp/huck_smoke_hist
```

Expected behavior:
- `!!` echoes and reruns `echo two` → prints `echo two` then `two`.
- `!1` reruns `echo one` → prints `echo one` then `one`.
- `echo !$` → expands to `echo one` (last word of the previous command `echo one`) → prints `echo one` then `one`.
- `^two^three^` → previous command was `echo !$`'s expansion... note: the previous command is whatever was last *added*. Verify the substitution runs against the most recent history entry and prints sensibly.
- `history` lists the numbered entries.
- `!nope` → stderr `huck: !nope: event not found`, nothing run.
- `history -c` then `history` → empty.

Confirm no panic and the histfile is written. Report the actual output.

- [ ] **Step 7: Commit**

```bash
git add src/shell.rs
git commit -m "v13 task 7: wire history expansion and persistence into the REPL"
```

---

## Task 8: End-to-end integration tests

**Files:**
- Create: `tests/history_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/history_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with the given stdin script and an isolated HISTFILE.
fn run_with_histfile(script: &str, histfile: &std::path::Path) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .env("HISTFILE", histfile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn bang_bang_reruns_previous_command() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo hello\n!!\nexit\n", &hf);
    // `echo hello` prints once directly, then `!!` reruns it.
    let count = out.lines().filter(|l| *l == "hello").count();
    assert!(count >= 2, "expected 'hello' at least twice, stdout: {out}");
}

#[test]
fn bang_dollar_substitutes_last_argument() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo alpha beta\necho !$\nexit\n", &hf);
    assert!(out.lines().any(|l| l == "beta"), "stdout: {out}");
}

#[test]
fn quick_substitution_replaces_text() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo hello\n^hello^goodbye^\nexit\n", &hf);
    assert!(out.lines().any(|l| l == "goodbye"), "stdout: {out}");
}

#[test]
fn failed_expansion_writes_error_and_does_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (_, err) = run_with_histfile("!nonexistent\nexit\n", &hf);
    assert!(err.contains("event not found"), "stderr: {err}");
}

#[test]
fn history_builtin_lists_commands() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo aaa\necho bbb\nhistory\nexit\n", &hf);
    // The `history` output lists the prior commands.
    assert!(out.contains("echo aaa"), "stdout: {out}");
    assert!(out.contains("echo bbb"), "stdout: {out}");
}

#[test]
fn history_dash_c_clears() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile(
        "echo keep\nhistory -c\nhistory\nexit\n",
        &hf,
    );
    // After `history -c`, the final `history` shows nothing — so the
    // command text "echo keep" should not appear in a `history` listing
    // line. (It still appears as the echoed command's own output.)
    let listing_lines: Vec<&str> = out.lines().filter(|l| l.contains('\t')).collect();
    assert!(listing_lines.is_empty(), "history should be empty, stdout: {out}");
}

#[test]
fn history_persists_across_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    // Session 1: run two commands.
    run_with_histfile("echo first\necho second\nexit\n", &hf);
    // Session 2: history should show session 1's commands.
    let (out, _) = run_with_histfile("history\nexit\n", &hf);
    assert!(out.contains("echo first"), "stdout: {out}");
    assert!(out.contains("echo second"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test history_integration`
Expected: 7 tests pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/history_integration.rs
git commit -m "v13 task 8: end-to-end history integration tests"
```

---

## Task 9: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v13 row to the status table**

Append after the v12 row:

```
| v13       | Command history + history expansion (`!!`, `!$`, `^a^b^`) |
```

Match the table's column alignment.

- [ ] **Step 2: Add a History subsection in Features**

After the existing **Parameter-expansion modifiers (v12):** block, add:

```markdown
**Command history (v13):**
Commands are recorded in memory and persisted to `$HISTFILE` (default
`~/.huck_history`), loaded at startup and saved on exit, capped at
1000 entries. The `history` builtin lists numbered entries; `history
-c` clears them. History expansion runs on each input line before
parsing: `!!` (previous command), `!n` (entry n), `!-n` (n entries
back), `!string` (most recent starting with `string`), `!$` (last
argument), `!^` (first argument), `!*` (all arguments), and
`^old^new^` quick substitution. A `!` is literal inside single
quotes, before whitespace/`=`, or when escaped (`\!`); it still
expands inside double quotes (matching bash). An expanded line is
echoed before it runs. Word designators (`!!:2`) and modifiers
(`:h`/`:t`/`:s`) are not yet implemented.
```

- [ ] **Step 2b: Add `history` to the Builtins list**

Find the builtins listing in `README.md` (the line enumerating `cd`, `pwd`, `echo`, ... `disown`) and add `history` to it.

- [ ] **Step 3: Update the Not-yet-implemented section**

Remove `history expansion` from the not-yet-implemented list. If the list mentions history only as part of a longer bullet, edit it so the remaining text still reads correctly.

- [ ] **Step 4: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the passed counts. Update the `cargo test               # full test suite (NNN tests)` line. Expected total is roughly 472 baseline + ~58 new = ~530 — use the actual number.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v13 task 9: README — add v13 row and history section"
```

---

## Final review checkpoint

After Task 9:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session: run a few commands, then exercise `!!`, `!1`, `!-1`, `!echo`, `!$`, `!^`, `!*`, `^a^b^`, a failing `!x`, `history`, `history -c`. Quit and relaunch to confirm the histfile persisted.
- [ ] Final review the whole branch as a single diff before merging to main
