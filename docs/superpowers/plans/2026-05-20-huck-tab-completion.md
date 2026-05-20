# huck v14: Tab Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Tab completion to huck's interactive prompt — command names, filenames/paths, and variable names — via a custom rustyline `Helper`.

**Architecture:** A new `src/completion.rs` module owns a cursor-context scanner (`analyze`), three completion sources (`complete_command`, `complete_variable`, `complete_file`), and `HuckHelper` which implements rustyline's `Helper`/`Completer`. The REPL switches from `DefaultEditor` to `Editor<HuckHelper, FileHistory>` and refreshes the helper's snapshot of shell state before each `readline`.

**Tech Stack:** Rust 2024 edition, `rustyline 18` (no new feature flags). No new dependencies.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-20-huck-tab-completion-design.md`.

**Note on signatures:** The spec sketched `complete_command(prefix, helper)`. This plan refines that — the three completion sources take plain data (`&str`, `&[String]`) rather than `&HuckHelper`, so they have no dependency on the helper struct and are trivially testable. `complete_file` also takes an explicit `home: &str` (instead of reading `$HOME` from the process env) so tests need not mutate global env.

---

## File Map

- **New:** `src/completion.rs` — `Candidate`, `CompletionContext`, `analyze`, `complete_command`/`complete_variable`/`complete_file`, `HuckHelper` + trait impls
- **Modify:** `src/builtins.rs` — `pub const BUILTIN_NAMES`; `is_builtin` rewritten
- **Modify:** `src/shell_state.rs` — `var_names` accessor
- **Modify:** `src/shell.rs` — custom `Editor<HuckHelper, FileHistory>`, helper refresh
- **Modify:** `src/main.rs` — register `mod completion`
- **Modify:** `README.md` — v14 row, features, test count

---

## Task 1: `BUILTIN_NAMES` const and `Shell::var_names`

Make the builtin-name set a public constant (single source of truth) and add an accessor for all shell variable names.

**Files:**
- Modify: `src/builtins.rs`
- Modify: `src/shell_state.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn builtin_names_const_matches_is_builtin() {
    for name in BUILTIN_NAMES {
        assert!(is_builtin(name), "{name} should be a builtin");
    }
    assert!(!is_builtin("definitely_not_a_builtin"));
}

#[test]
fn builtin_names_includes_history() {
    assert!(BUILTIN_NAMES.contains(&"history"));
}
```

Add to `src/shell_state.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn var_names_lists_all_variables() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_VN", "value".to_string());
    let names: Vec<&str> = shell.var_names().collect();
    assert!(names.contains(&"HUCK_TEST_VN"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test builtin_names_const_matches_is_builtin`
Expected: FAIL — `BUILTIN_NAMES` does not exist.

- [ ] **Step 3: Add `BUILTIN_NAMES` and rewrite `is_builtin`**

In `src/builtins.rs`, replace the existing `is_builtin` function with:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}
```

(The `run_builtin` dispatch `match` is unchanged.)

- [ ] **Step 4: Add `var_names` to `Shell`**

In `src/shell_state.rs`, add this method inside `impl Shell` (the `vars` field is a `HashMap<String, Variable>`):

```rust
/// Iterates the names of all variables (exported or not).
pub fn var_names(&self) -> impl Iterator<Item = &str> {
    self.vars.keys().map(|s| s.as_str())
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: all tests pass (528 baseline + 3 new = 531). The existing `is_builtin` tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs src/shell_state.rs
git commit -m "v14 task 1: BUILTIN_NAMES const and Shell::var_names accessor"
```

---

## Task 2: `completion.rs` skeleton — types and the `analyze` scanner

Create `src/completion.rs` with `Candidate`, `CompletionContext`, and the cursor-context scanner `analyze`, plus its helpers. Register the module.

**Files:**
- Create: `src/completion.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create `src/completion.rs`**

Create the file with this content:

```rust
//! Tab completion: cursor-context analysis and completion sources.

/// One completion candidate. `display` is shown in the Tab-Tab list;
/// `replacement` is the (possibly escaped) text inserted into the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub display: String,
    pub replacement: String,
}

/// What the cursor is positioned to complete.
#[derive(Debug, PartialEq, Eq)]
pub enum CompletionContext {
    Command { prefix: String },
    Variable { prefix: String },
    File { dir: String, prefix: String },
}

/// Classifies the completion context at byte offset `pos` in `line`.
/// Returns the byte offset where replacement begins and the context
/// (whose prefix has backslash-escapes resolved).
pub fn analyze(line: &str, pos: usize) -> (usize, CompletionContext) {
    let head = &line[..pos];

    // Forward scan: find the current word's start offset and whether
    // it sits in command position.
    let mut word_start = 0usize;
    let mut current_has_content = false;
    let mut is_command_pos = true;
    let mut in_single = false;
    let mut in_double = false;

    let indexed: Vec<(usize, char)> = head.char_indices().collect();
    let mut i = 0;
    while i < indexed.len() {
        let (off, c) = indexed[i];

        if !in_single && c == '\\' {
            // Backslash escapes the next character; both stay in the word.
            current_has_content = true;
            i += 2;
            continue;
        }
        if in_single {
            current_has_content = true;
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            current_has_content = true;
            if c == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' => {
                in_single = true;
                current_has_content = true;
                i += 1;
            }
            '"' => {
                in_double = true;
                current_has_content = true;
                i += 1;
            }
            ' ' | '\t' => {
                if current_has_content {
                    let word = &head[word_start..off];
                    if !is_assignment(word) {
                        is_command_pos = false;
                    }
                }
                current_has_content = false;
                word_start = off + c.len_utf8();
                i += 1;
            }
            ';' | '|' | '&' | '<' | '>' => {
                current_has_content = false;
                is_command_pos = true;
                word_start = off + c.len_utf8();
                i += 1;
            }
            _ => {
                current_has_content = true;
                i += 1;
            }
        }
    }

    let word = &head[word_start..];

    // Variable context: a trailing in-progress `$NAME` / `${NAME`.
    if let Some(dollar) = last_unescaped_dollar(word) {
        let after = &word[dollar + 1..];
        let (brace, name) = match after.strip_prefix('{') {
            Some(rest) => (true, rest),
            None => (false, after),
        };
        if name.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
            let name_off = dollar + 1 + if brace { 1 } else { 0 };
            return (
                word_start + name_off,
                CompletionContext::Variable { prefix: name.to_string() },
            );
        }
    }

    // File context: a word containing `/` is always a path.
    if let Some(slash) = word.rfind('/') {
        let dir = unescape(&word[..=slash]);
        let prefix = unescape(&word[slash + 1..]);
        return (word_start + slash + 1, CompletionContext::File { dir, prefix });
    }

    // Argument position (no slash) -> File in the current directory.
    if !is_command_pos {
        return (
            word_start,
            CompletionContext::File { dir: String::new(), prefix: unescape(word) },
        );
    }

    // Command position.
    (word_start, CompletionContext::Command { prefix: unescape(word) })
}

/// True if `word` looks like a `NAME=value` assignment prefix.
fn is_assignment(word: &str) -> bool {
    let Some(eq) = word.find('=') else { return false };
    let name = &word[..eq];
    !name.is_empty()
        && name.chars().next().map(|c| c == '_' || c.is_ascii_alphabetic()).unwrap_or(false)
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Byte offset of the last `$` in `word` that is not backslash-escaped.
fn last_unescaped_dollar(word: &str) -> Option<usize> {
    let mut result = None;
    let mut escaped = false;
    for (i, c) in word.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' {
            escaped = true;
        } else if c == '$' {
            result = Some(i);
        }
    }
    result
}

/// Resolves backslash escapes: `\x` -> `x`.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => out.push(next),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_empty_line_is_command() {
        let (start, ctx) = analyze("", 0);
        assert_eq!(start, 0);
        assert_eq!(ctx, CompletionContext::Command { prefix: String::new() });
    }

    #[test]
    fn analyze_first_word_is_command() {
        let (start, ctx) = analyze("ec", 2);
        assert_eq!(start, 0);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_command_is_file() {
        let (start, ctx) = analyze("echo fo", 7);
        assert_eq!(start, 5);
        assert_eq!(ctx, CompletionContext::File { dir: String::new(), prefix: "fo".to_string() });
    }

    #[test]
    fn analyze_after_semicolon_is_command() {
        let (start, ctx) = analyze("echo hi; ec", 11);
        assert_eq!(start, 9);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_pipe_is_command() {
        let (_, ctx) = analyze("ls | gr", 7);
        assert_eq!(ctx, CompletionContext::Command { prefix: "gr".to_string() });
    }

    #[test]
    fn analyze_after_assignment_word_is_command() {
        let (start, ctx) = analyze("FOO=bar ec", 10);
        assert_eq!(start, 8);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_variable_dollar() {
        let (start, ctx) = analyze("echo $HO", 8);
        assert_eq!(start, 6);
        assert_eq!(ctx, CompletionContext::Variable { prefix: "HO".to_string() });
    }

    #[test]
    fn analyze_variable_braced() {
        let (start, ctx) = analyze("echo ${HO", 9);
        assert_eq!(start, 7);
        assert_eq!(ctx, CompletionContext::Variable { prefix: "HO".to_string() });
    }

    #[test]
    fn analyze_variable_mid_word() {
        let (start, ctx) = analyze("echo foo$BA", 11);
        assert_eq!(start, 9);
        assert_eq!(ctx, CompletionContext::Variable { prefix: "BA".to_string() });
    }

    #[test]
    fn analyze_variable_empty_prefix() {
        let (start, ctx) = analyze("echo $", 6);
        assert_eq!(start, 6);
        assert_eq!(ctx, CompletionContext::Variable { prefix: String::new() });
    }

    #[test]
    fn analyze_path_splits_at_slash() {
        let (start, ctx) = analyze("cat src/le", 10);
        assert_eq!(start, 8);
        assert_eq!(ctx, CompletionContext::File { dir: "src/".to_string(), prefix: "le".to_string() });
    }

    #[test]
    fn analyze_command_with_slash_is_file() {
        // A command word containing `/` is path-completed.
        let (start, ctx) = analyze("./scr", 5);
        assert_eq!(start, 2);
        assert_eq!(ctx, CompletionContext::File { dir: "./".to_string(), prefix: "scr".to_string() });
    }

    #[test]
    fn analyze_escaped_space_stays_in_word() {
        // `cat my\ fi` — the escaped space is part of the filename word.
        let (start, ctx) = analyze("cat my\\ fi", 10);
        assert_eq!(start, 4);
        assert_eq!(ctx, CompletionContext::File { dir: String::new(), prefix: "my fi".to_string() });
    }

    #[test]
    fn analyze_ignores_text_after_cursor() {
        let (_, ctx) = analyze("echo fo bar", 7);
        assert_eq!(ctx, CompletionContext::File { dir: String::new(), prefix: "fo".to_string() });
    }

    #[test]
    fn analyze_escaped_dollar_is_not_variable() {
        // `\$HOME` — the `$` is escaped, so this is not a variable context.
        let (_, ctx) = analyze("echo \\$HOM", 10);
        assert!(matches!(ctx, CompletionContext::File { .. }));
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs`. Add `mod completion;` alphabetically with the other `mod` declarations.

- [ ] **Step 3: Run tests**

Run: `cargo test completion::`
Expected: 15 tests pass.

- [ ] **Step 4: Run full suite**

Run: `cargo test`
Expected: all tests pass. Dead-code warnings on `Candidate` and the unused-yet completion functions are expected.

- [ ] **Step 5: Commit**

```bash
git add src/completion.rs src/main.rs
git commit -m "v14 task 2: completion types and the analyze cursor scanner"
```

---

## Task 3: `complete_command`

Add command-name completion: builtins plus executables found on a `$PATH`-style string.

**Files:**
- Modify: `src/completion.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/completion.rs`:

```rust
#[test]
fn complete_command_matches_builtin_prefix() {
    let cands = complete_command("ec", "");
    assert!(cands.iter().any(|c| c.replacement == "echo"));
}

#[test]
fn complete_command_empty_prefix_includes_builtins() {
    let cands = complete_command("", "");
    assert!(cands.iter().any(|c| c.replacement == "cd"));
    assert!(cands.iter().any(|c| c.replacement == "history"));
}

#[test]
fn complete_command_scans_path_for_executables() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    // An executable file.
    let exe = dir.path().join("huckcmd_exe");
    std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    // A non-executable file.
    let plain = dir.path().join("huckcmd_plain");
    std::fs::write(&plain, b"data").unwrap();
    std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
    // A subdirectory.
    std::fs::create_dir(dir.path().join("huckcmd_subdir")).unwrap();

    let path = dir.path().to_str().unwrap();
    let cands = complete_command("huckcmd_", path);
    let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert!(names.contains(&"huckcmd_exe"), "exe should match: {names:?}");
    assert!(!names.contains(&"huckcmd_plain"), "non-exe should not match");
    assert!(!names.contains(&"huckcmd_subdir"), "subdir should not match");
}

#[test]
fn complete_command_results_are_sorted_and_unique() {
    let cands = complete_command("", "");
    let mut sorted = cands.clone();
    sorted.sort_by(|a, b| a.replacement.cmp(&b.replacement));
    assert_eq!(cands, sorted);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test complete_command_matches_builtin_prefix`
Expected: FAIL — `complete_command` does not exist.

- [ ] **Step 3: Implement `complete_command`**

Add to `src/completion.rs` (above the test module):

```rust
use std::collections::BTreeSet;

/// Completes a command name: builtins plus executables found in the
/// `:`-separated `path`.
pub fn complete_command(prefix: &str, path: &str) -> Vec<Candidate> {
    let mut names: BTreeSet<String> = BTreeSet::new();

    for &builtin in crate::builtins::BUILTIN_NAMES {
        if builtin.starts_with(prefix) {
            names.insert(builtin.to_string());
        }
    }

    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else { continue };
            if name.starts_with(prefix) && is_executable_file(&entry) {
                names.insert(name.to_string());
            }
        }
    }

    names
        .into_iter()
        .map(|n| Candidate { display: n.clone(), replacement: n })
        .collect()
}

/// True if the directory entry is a regular file with an executable bit.
fn is_executable_file(entry: &std::fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match entry.metadata() {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test completion::`
Expected: all completion tests pass (15 from Task 2 + 4 new = 19).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/completion.rs
git commit -m "v14 task 3: complete_command (builtins + PATH executables)"
```

---

## Task 4: `complete_variable`

Add variable-name completion from a list of names.

**Files:**
- Modify: `src/completion.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/completion.rs`:

```rust
#[test]
fn complete_variable_matches_prefix() {
    let names = vec![
        "HOME".to_string(),
        "HOST".to_string(),
        "PATH".to_string(),
    ];
    let cands = complete_variable("HO", &names);
    let got: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert_eq!(got, vec!["HOME", "HOST"]);
}

#[test]
fn complete_variable_empty_prefix_returns_all_sorted() {
    let names = vec!["ZED".to_string(), "ABC".to_string()];
    let cands = complete_variable("", &names);
    let got: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert_eq!(got, vec!["ABC", "ZED"]);
}

#[test]
fn complete_variable_no_match_is_empty() {
    let names = vec!["HOME".to_string()];
    assert!(complete_variable("XYZ", &names).is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test complete_variable_matches_prefix`
Expected: FAIL — `complete_variable` does not exist.

- [ ] **Step 3: Implement `complete_variable`**

Add to `src/completion.rs` (above the test module):

```rust
/// Completes a variable name from `var_names`.
pub fn complete_variable(prefix: &str, var_names: &[String]) -> Vec<Candidate> {
    let mut matches: Vec<String> = var_names
        .iter()
        .filter(|n| n.starts_with(prefix))
        .cloned()
        .collect();
    matches.sort();
    matches.dedup();
    matches
        .into_iter()
        .map(|n| Candidate { display: n.clone(), replacement: n })
        .collect()
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test completion::`
Expected: all completion tests pass (19 from Tasks 2-3 + 3 new = 22).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/completion.rs
git commit -m "v14 task 4: complete_variable"
```

---

## Task 5: `complete_file`

Add filename/path completion with directory `/` suffixes, hidden-file gating, `~/` expansion, and metacharacter escaping.

**Files:**
- Modify: `src/completion.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/completion.rs`:

```rust
fn touch(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"").unwrap();
}

#[test]
fn complete_file_matches_prefix() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "alpha.txt");
    touch(dir.path(), "alpine.txt");
    touch(dir.path(), "beta.txt");
    let cands = complete_file(dir.path().to_str().unwrap(), "alp", "");
    let got: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
    assert_eq!(got, vec!["alpha.txt", "alpine.txt"]);
}

#[test]
fn complete_file_directory_gets_trailing_slash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("mysub")).unwrap();
    let cands = complete_file(dir.path().to_str().unwrap(), "mys", "");
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].display, "mysub/");
    assert_eq!(cands[0].replacement, "mysub/");
}

#[test]
fn complete_file_hidden_excluded_unless_prefix_dot() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), ".secret");
    touch(dir.path(), "visible");
    let no_dot = complete_file(dir.path().to_str().unwrap(), "", "");
    assert!(no_dot.iter().all(|c| c.replacement != ".secret"));
    let with_dot = complete_file(dir.path().to_str().unwrap(), ".", "");
    assert!(with_dot.iter().any(|c| c.replacement == ".secret"));
}

#[test]
fn complete_file_escapes_spaces() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "my file.txt");
    let cands = complete_file(dir.path().to_str().unwrap(), "my", "");
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].display, "my file.txt");
    assert_eq!(cands[0].replacement, "my\\ file.txt");
}

#[test]
fn complete_file_tilde_expands_with_home() {
    let home = tempfile::tempdir().unwrap();
    touch(home.path(), "homefile");
    let home_str = home.path().to_str().unwrap();
    // dir "~/" should resolve against the supplied home.
    let cands = complete_file("~/", "homef", home_str);
    assert!(cands.iter().any(|c| c.replacement == "homefile"), "{cands:?}");
}

#[test]
fn complete_file_empty_dir_scans_relative() {
    // An empty `dir` scans the current directory; just confirm no panic
    // and a Vec is returned.
    let _ = complete_file("", "", "");
}

#[test]
fn complete_file_unreadable_dir_is_empty() {
    assert!(complete_file("/nonexistent/huck/path", "x", "").is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test complete_file_matches_prefix`
Expected: FAIL — `complete_file` does not exist.

- [ ] **Step 3: Implement `complete_file`**

Add to `src/completion.rs` (above the test module):

```rust
use std::path::PathBuf;

/// Completes a filename. `dir` is the directory portion of the word
/// (empty = current directory; a leading `~/` is expanded against
/// `home`). `prefix` is the filename fragment. Directory results get a
/// trailing `/`; `replacement` is metacharacter-escaped.
pub fn complete_file(dir: &str, prefix: &str, home: &str) -> Vec<Candidate> {
    let Some(scan_dir) = resolve_dir(dir, home) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&scan_dir) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut candidates: Vec<Candidate> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.starts_with(prefix) {
            continue;
        }
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut display = name.to_string();
        let mut replacement = escape_filename(name);
        if is_dir {
            display.push('/');
            replacement.push('/');
        }
        candidates.push(Candidate { display, replacement });
    }
    candidates.sort_by(|a, b| a.display.cmp(&b.display));
    candidates
}

/// Resolves the directory to scan. `~/` is expanded against `home`.
fn resolve_dir(dir: &str, home: &str) -> Option<PathBuf> {
    if dir.is_empty() {
        return Some(PathBuf::from("."));
    }
    if let Some(rest) = dir.strip_prefix("~/") {
        if home.is_empty() {
            return None;
        }
        return Some(PathBuf::from(home).join(rest));
    }
    Some(PathBuf::from(dir))
}

/// Backslash-escapes shell metacharacters in a filename.
fn escape_filename(name: &str) -> String {
    const SPECIAL: &[char] = &[
        ' ', '\t', '\'', '"', '\\', '$', ';', '&', '|', '<', '>',
        '(', ')', '*', '?', '[', ']', '~', '#', '`',
    ];
    let mut out = String::new();
    for c in name.chars() {
        if SPECIAL.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test completion::`
Expected: all completion tests pass (22 from Tasks 2-4 + 7 new = 29).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/completion.rs
git commit -m "v14 task 5: complete_file (paths, escaping, ~/ expansion)"
```

---

## Task 6: `HuckHelper` and the `Completer` glue

Add the `HuckHelper` struct, its rustyline trait impls, and the `Completer::complete` method that ties `analyze` to the three sources.

**Files:**
- Modify: `src/completion.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/completion.rs`:

```rust
#[test]
fn helper_complete_command_context() {
    let helper = HuckHelper {
        var_names: Vec::new(),
        path: String::new(),
        home: String::new(),
    };
    let history = rustyline::history::FileHistory::new();
    let ctx = rustyline::Context::new(&history);
    let (start, pairs) = rustyline::completion::Completer::complete(
        &helper, "ec", 2, &ctx,
    ).unwrap();
    assert_eq!(start, 0);
    assert!(pairs.iter().any(|p| p.replacement == "echo"));
}

#[test]
fn helper_complete_variable_context() {
    let helper = HuckHelper {
        var_names: vec!["HOME".to_string(), "PATH".to_string()],
        path: String::new(),
        home: String::new(),
    };
    let history = rustyline::history::FileHistory::new();
    let ctx = rustyline::Context::new(&history);
    let (start, pairs) = rustyline::completion::Completer::complete(
        &helper, "echo $HO", 8, &ctx,
    ).unwrap();
    assert_eq!(start, 6);
    assert!(pairs.iter().any(|p| p.replacement == "HOME"));
}

#[test]
fn helper_complete_file_context() {
    let dir = tempfile::tempdir().unwrap();
    touch(dir.path(), "targetfile");
    let helper = HuckHelper {
        var_names: Vec::new(),
        path: String::new(),
        home: String::new(),
    };
    let history = rustyline::history::FileHistory::new();
    let ctx = rustyline::Context::new(&history);
    let line = format!("echo {}/targ", dir.path().to_str().unwrap());
    let pos = line.len();
    let (_, pairs) = rustyline::completion::Completer::complete(
        &helper, &line, pos, &ctx,
    ).unwrap();
    assert!(pairs.iter().any(|p| p.replacement == "targetfile"), "{pairs:?}");
}
```

Note: if `rustyline::Context::new` or `FileHistory::new` differ in rustyline 18's API, adjust the construction (e.g. `FileHistory::default()`); the intent is an empty history and a `Context` borrowing it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test helper_complete_command_context`
Expected: FAIL — `HuckHelper` does not exist.

- [ ] **Step 3: Implement `HuckHelper` and the trait impls**

Add to `src/completion.rs` (above the test module):

```rust
use crate::shell_state::Shell;

/// rustyline completion helper. Holds a snapshot of shell state
/// (variable names, `$PATH`, `$HOME`) refreshed before each readline.
pub struct HuckHelper {
    var_names: Vec<String>,
    path: String,
    home: String,
}

impl HuckHelper {
    pub fn new() -> Self {
        Self { var_names: Vec::new(), path: String::new(), home: String::new() }
    }

    /// Refreshes the cached snapshot from live shell state.
    pub fn refresh(&mut self, shell: &Shell) {
        self.var_names = shell.var_names().map(|s| s.to_string()).collect();
        self.path = shell.get("PATH").unwrap_or("").to_string();
        self.home = shell.get("HOME").unwrap_or("").to_string();
    }
}

impl Default for HuckHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl rustyline::completion::Completer for HuckHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let (start, context) = analyze(line, pos);
        let candidates = match context {
            CompletionContext::Command { prefix } => {
                complete_command(&prefix, &self.path)
            }
            CompletionContext::Variable { prefix } => {
                complete_variable(&prefix, &self.var_names)
            }
            CompletionContext::File { dir, prefix } => {
                complete_file(&dir, &prefix, &self.home)
            }
        };
        let pairs = candidates
            .into_iter()
            .map(|c| rustyline::completion::Pair {
                display: c.display,
                replacement: c.replacement,
            })
            .collect();
        Ok((start, pairs))
    }
}

impl rustyline::hint::Hinter for HuckHelper {
    type Hint = String;
}

impl rustyline::highlight::Highlighter for HuckHelper {}

impl rustyline::validate::Validator for HuckHelper {}

impl rustyline::Helper for HuckHelper {}
```

If any of the four trait impls fails to compile because a required method has no default in rustyline 18, implement that method minimally (e.g. `Hinter::hint` returning `None`). The intent: `Completer` is real; the other three are inert.

- [ ] **Step 4: Run tests**

Run: `cargo test completion::`
Expected: all completion tests pass (29 from Tasks 2-5 + 3 new = 32).

- [ ] **Step 5: Run the full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/completion.rs
git commit -m "v14 task 6: HuckHelper and the Completer glue"
```

---

## Task 7: REPL integration

Switch `src/shell.rs` from `DefaultEditor` to a custom `Editor<HuckHelper, FileHistory>` with list-style completion, and refresh the helper before each `readline`.

**Files:**
- Modify: `src/shell.rs`

- [ ] **Step 1: Inspect the current code**

Read `src/shell.rs`. Currently:
- `use rustyline::DefaultEditor;` and `use rustyline::error::ReadlineError;`
- `let mut editor = match DefaultEditor::new() { ... }`
- After `Shell::new()`: `shell.history.load()` and a loop seeding `editor.add_history_entry`.
- The loop calls `editor.readline(PROMPT)`.

- [ ] **Step 2: Update the imports**

Replace `use rustyline::DefaultEditor;` with:

```rust
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};
```

Keep `use rustyline::error::ReadlineError;`.

Add, with the other `use crate::...` lines:

```rust
use crate::completion::HuckHelper;
```

- [ ] **Step 3: Build the editor with completion configured**

Replace the `let mut editor = match DefaultEditor::new() { ... }` block with:

```rust
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: Editor<HuckHelper, FileHistory> = match Editor::with_config(config) {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("huck: failed to initialize line editor: {e}");
            return 1;
        }
    };
    editor.set_helper(Some(HuckHelper::new()));
```

- [ ] **Step 4: Refresh the helper before each `readline`**

In the loop, immediately before `match editor.readline(PROMPT)`, add:

```rust
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
```

(Place it after the existing `crate::jobs::reap_and_notify(&mut shell);` call, if that is the first statement in the loop body — keep both, order: reap, then refresh, then readline.)

- [ ] **Step 5: Build and run the full suite**

Run: `cargo build && cargo test`
Expected: builds cleanly; all tests pass. The v13 history calls (`add_history_entry`, the seeding loop, `editor` history use) compile unchanged on the generic `Editor`.

- [ ] **Step 6: Manual smoke test**

Tab completion needs a TTY, so it cannot be scripted. Build and drive it by hand:

```bash
cargo build --release
~/projects/shuck/target/release/huck
```

In the interactive prompt, verify:
- `ec<Tab>` → completes to `echo` (and other `ec*` commands).
- `<Tab><Tab>` on an empty line → lists builtins + PATH commands.
- `echo src/<Tab>` (run from the repo root) → lists files in `src/`.
- `cd sr<Tab>` → completes `src/` with a trailing slash.
- `echo $HO<Tab>` → completes `$HOME`.
- `echo $<Tab><Tab>` → lists variable names.
- Create a file with a space (`touch "a b.txt"`) in a scratch dir, then `cat a<Tab>` → inserts `a\ b.txt`.

Report the observed behavior. If something is broken, fix it before committing.

- [ ] **Step 7: Commit**

```bash
git add src/shell.rs
git commit -m "v14 task 7: wire HuckHelper into the REPL editor"
```

---

## Task 8: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v14 row to the status table**

Append after the v13 row:

```
| v14       | Tab completion (commands, filenames, variables)         |
```

Match the table's column alignment.

- [ ] **Step 2: Add a Tab-completion subsection in Features**

After the existing **Command history (v13):** block, add:

```markdown
**Tab completion (v14):**
Tab completes command names (builtins and `$PATH` executables) in
command position, filenames and paths in argument position
(directories shown with a trailing `/`), and variable names after
`$`/`${`. The first Tab fills in the longest common prefix; a second
Tab lists all candidates. Filenames with shell-special characters are
backslash-escaped when inserted; a leading `~/` is expanded before
the directory is scanned; hidden files appear only when the typed
prefix begins with `.`. Per-command argument completion and `~user`
completion are not implemented.
```

- [ ] **Step 3: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the passed counts. Update the `cargo test               # full test suite (NNN tests)` line. Expected total is roughly 528 baseline + ~32 new ≈ 560 — use the actual number.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "v14 task 8: README — add v14 row and tab-completion section"
```

---

## Final review checkpoint

After Task 8:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session covering command, file, and variable completion; the Tab-Tab list; a directory `/` suffix; a filename with a space
- [ ] Final review the whole branch as a single diff before merging to main
