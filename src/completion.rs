//! Tab completion: cursor-context analysis and completion sources.

use std::collections::BTreeSet;

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
                    // Assignment prefix and compound-command keywords both keep
                    // the next word in command position; everything else moves
                    // into argument position.
                    is_command_pos = is_assignment(word) || is_compound_keyword(word);
                }
                current_has_content = false;
                word_start = off + c.len_utf8();
                i += 1;
            }
            ';' | '|' | '&' => {
                current_has_content = false;
                is_command_pos = true;
                word_start = off + c.len_utf8();
                i += 1;
            }
            '<' | '>' => {
                current_has_content = false;
                // A redirect operator does not introduce a command; what follows
                // is a file argument. Leave is_command_pos unchanged.
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

    if let Some(slash) = word.rfind('/') {
        let dir = unescape(&word[..=slash]);
        let prefix = unescape(&word[slash + 1..]);
        return (word_start + slash + 1, CompletionContext::File { dir, prefix });
    }

    if !is_command_pos {
        return (
            word_start,
            CompletionContext::File { dir: String::new(), prefix: unescape(word) },
        );
    }

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

/// True if `word` is a compound-command keyword after which the next word
/// is in command position (i.e., the start of a new simple command).
fn is_compound_keyword(word: &str) -> bool {
    matches!(word, "then" | "do" | "else" | "elif" | "fi" | "done" | "esac" | "{" | "}")
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
        .map(|n| Candidate { display: n.clone(), replacement: escape_filename(&n) })
        .collect()
}

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
        // std::fs::metadata follows symlinks, so a symlink to a directory is
        // correctly given a trailing slash.
        let is_dir = std::fs::metadata(entry.path())
            .map(|m| m.is_dir())
            .unwrap_or(false);
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

/// True if the directory entry resolves to a regular file with an
/// executable bit. Uses `std::fs::metadata` (follows symlinks) so that
/// symlinked executables — common in /usr/bin — are found.
fn is_executable_file(entry: &std::fs::DirEntry) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(entry.path()) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

use crate::shell_state::Shell;
use std::cell::RefCell;
use std::rc::Rc;

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

impl rustyline::completion::Completer for HuckHelper {
    type Candidate = rustyline::completion::Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        let shell = self.shell.borrow();
        let path = shell.get("PATH").unwrap_or("").to_string();
        let home = shell.get("HOME").unwrap_or("").to_string();
        let var_names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
        drop(shell);

        let (start, context) = analyze(line, pos);
        let candidates = match context {
            CompletionContext::Command { prefix } => complete_command(&prefix, &path),
            CompletionContext::Variable { prefix } => complete_variable(&prefix, &var_names),
            CompletionContext::File { dir, prefix } => complete_file(&dir, &prefix, &home),
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
    fn analyze_after_then_keyword_is_command() {
        let (start, ctx) = analyze("if true; then ec", 16);
        assert_eq!(start, 14);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_do_keyword_is_command() {
        let (_, ctx) = analyze("for x in 1; do ec", 17);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_else_keyword_is_command() {
        let (_, ctx) = analyze("if x; then y; else ec", 21);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_elif_keyword_is_command() {
        let (_, ctx) = analyze("if x; then y; elif ec", 21);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_open_brace_keyword_is_command() {
        let (_, ctx) = analyze("{ ec", 4);
        assert_eq!(ctx, CompletionContext::Command { prefix: "ec".to_string() });
    }

    #[test]
    fn analyze_after_fi_keyword_is_command() {
        // After `fi`, a separator is conventionally required, but treating the
        // next word as a command position is the more useful completion default
        // — it lets the user tab-complete `if x; then y; fi <TAB>` to a command.
        let (_, ctx) = analyze("if x; then y; fi ec", 19);
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
        let (start, ctx) = analyze("./scr", 5);
        assert_eq!(start, 2);
        assert_eq!(ctx, CompletionContext::File { dir: "./".to_string(), prefix: "scr".to_string() });
    }

    #[test]
    fn analyze_escaped_space_stays_in_word() {
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
        let (_, ctx) = analyze("echo \\$HOM", 10);
        assert!(matches!(ctx, CompletionContext::File { .. }));
    }

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
        let exe = dir.path().join("huckcmd_exe");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.path().join("huckcmd_plain");
        std::fs::write(&plain, b"data").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
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
        let cands = complete_file("~/", "homef", home_str);
        assert!(cands.iter().any(|c| c.replacement == "homefile"), "{cands:?}");
    }

    #[test]
    fn complete_file_empty_dir_scans_relative() {
        let _ = complete_file("", "", "");
    }

    #[test]
    fn complete_file_unreadable_dir_is_empty() {
        assert!(complete_file("/nonexistent/huck/path", "x", "").is_empty());
    }

    #[test]
    fn complete_command_finds_symlinked_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // A real executable.
        let real = dir.path().join("huckcmd_real");
        std::fs::write(&real, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        // A symlink to it.
        let link = dir.path().join("huckcmd_link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let path = dir.path().to_str().unwrap();
        let cands = complete_command("huckcmd_", path);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"huckcmd_real"), "{names:?}");
        assert!(names.contains(&"huckcmd_link"), "symlinked exe should complete: {names:?}");
    }

    #[test]
    fn complete_file_symlinked_directory_gets_trailing_slash() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("realdir");
        std::fs::create_dir(&real_dir).unwrap();
        let link = dir.path().join("linkdir");
        std::os::unix::fs::symlink(&real_dir, &link).unwrap();

        let cands = complete_file(dir.path().to_str().unwrap(), "linkd", "");
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].replacement, "linkdir/", "symlinked dir should get a trailing slash");
    }

    #[test]
    fn analyze_redirect_target_is_file_not_command() {
        // `echo > lo` — the word after `>` is a redirect target (a file),
        // not a command.
        let (_, ctx) = analyze("echo > lo", 9);
        assert_eq!(ctx, CompletionContext::File { dir: String::new(), prefix: "lo".to_string() });
    }

    #[test]
    fn helper_holds_rc_refcell_shell() {
        use std::rc::Rc;
        use std::cell::RefCell;
        let shell = Rc::new(RefCell::new(Shell::new()));
        let helper = HuckHelper::new(Rc::clone(&shell));
        // Mutate shell through the cell; helper must see the change live.
        shell.borrow_mut().set("MY_VAR", "hello".to_string());
        let history = rustyline::history::FileHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (start, pairs) = rustyline::completion::Completer::complete(
            &helper, "echo $MY_V", 10, &ctx,
        ).unwrap();
        assert_eq!(start, 6);
        let replacements: Vec<&str> = pairs.iter().map(|p| p.replacement.as_str()).collect();
        assert!(pairs.iter().any(|p| p.replacement == "MY_VAR"),
                "live var not visible to helper: {replacements:?}");
    }
}
