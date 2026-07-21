//! Tab completion: cursor-context analysis and completion sources.

use std::collections::BTreeSet;

/// What kind of completion a `Candidate` represents. Useful for embedders
/// rendering icons or sorting by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CandidateKind {
    /// Command position: executable on PATH, shell function, builtin, or alias.
    Command,
    /// `$x`-style variable name.
    Variable,
    /// Regular file in an argument position.
    File,
    /// Directory in an argument position (display includes trailing `/`).
    Directory,
    /// Returned from a `complete -F func` callback — underlying kind unknown.
    Custom,
}

/// One completion candidate. `display` is shown in the Tab-Tab list;
/// `replacement` is the (possibly escaped) text inserted into the line.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Candidate {
    pub display: String,
    pub replacement: String,
    pub kind: CandidateKind,
}

/// What the cursor is positioned to complete.
#[derive(Debug, PartialEq, Eq)]
pub enum CompletionContext {
    Command { prefix: String },
    Variable { prefix: String },
    File { dir: String, prefix: String },
}

/// Classifies the completion context at byte offset `pos` in `line`.
/// Returns the basename replacement offset and the context.
pub fn analyze(line: &str, pos: usize) -> (usize, CompletionContext) {
    let (_, start, ctx) = analyze_full(line, pos);
    (start, ctx)
}

/// Like `analyze`, but also returns the start of the WHOLE word
/// (`word_start`) — the anchor the programmable-completion path uses to
/// replace the entire `cur` word with full-path candidates.
pub(crate) fn analyze_full(line: &str, pos: usize) -> (usize, usize, CompletionContext) {
    let head = &line[..pos];

    let mut word_start = 0usize;
    let mut current_has_content = false;
    let mut is_command_pos = true;
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    // The command-position state to restore when a backtick command
    // substitution closes (backticks use the same char to open and close).
    let mut saved_cmd_pos = true;

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
            '(' => {
                // Command-substitution / subshell / process-substitution
                // opener: the word after `(` is a fresh command, so bash
                // command-completes there. Covers `$(`, `(`, `<(`, `>(`, and
                // `$((` (each `(` resets). `${` is `{`, not `(`, so parameter
                // expansion is untouched. A matching `)` needs no handling —
                // the space after it resets `is_command_pos` via the word
                // check on the ` `/`\t` arm.
                //
                // EXCEPTION: `NAME=(` is an array literal, not a subshell, so
                // bash does NOT command-complete inside it — guard on the
                // preceding char being `=`. `NAME=$(…` (comsub in an
                // assignment RHS) has `$` before `(`, so it still resets.
                let array_literal = off > 0 && head.as_bytes()[off - 1] == b'=';
                if !array_literal {
                    current_has_content = false;
                    is_command_pos = true;
                    word_start = off + c.len_utf8();
                } else {
                    current_has_content = true;
                }
                i += 1;
            }
            '`' => {
                // Backtick command substitution toggles: the content inside is
                // a fresh command; on close, restore the outer position so a
                // following word (e.g. `echo `ls` foo`) stays an argument.
                if in_backtick {
                    in_backtick = false;
                    is_command_pos = saved_cmd_pos;
                } else {
                    in_backtick = true;
                    saved_cmd_pos = is_command_pos;
                    is_command_pos = true;
                }
                current_has_content = false;
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
        if name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        {
            let name_off = dollar + 1 + if brace { 1 } else { 0 };
            return (
                word_start,
                word_start + name_off,
                CompletionContext::Variable {
                    prefix: name.to_string(),
                },
            );
        }
    }

    if let Some(slash) = word.rfind('/') {
        let dir = unescape(&word[..=slash]);
        let prefix = unescape(&word[slash + 1..]);
        return (
            word_start,
            word_start + slash + 1,
            CompletionContext::File { dir, prefix },
        );
    }

    if !is_command_pos {
        return (
            word_start,
            word_start,
            CompletionContext::File {
                dir: String::new(),
                prefix: unescape(word),
            },
        );
    }

    (
        word_start,
        word_start,
        CompletionContext::Command {
            prefix: unescape(word),
        },
    )
}

/// True if `word` looks like a `NAME=value` assignment prefix.
fn is_assignment(word: &str) -> bool {
    let Some(eq) = word.find('=') else {
        return false;
    };
    let name = &word[..eq];
    !name.is_empty()
        && name
            .chars()
            .next()
            .map(|c| c == '_' || c.is_ascii_alphabetic())
            .unwrap_or(false)
        && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// True if `word` is a compound-command keyword after which the next word
/// is in command position (i.e., the start of a new simple command).
fn is_compound_keyword(word: &str) -> bool {
    matches!(
        word,
        "then" | "do" | "else" | "elif" | "fi" | "done" | "esac" | "{" | "}"
    )
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

/// Shell keywords completed at command position (bash completes these too).
const COMPLETION_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "coproc",
];

/// Appends bash's post-completion trailing space to the `replacement` of
/// every non-directory candidate; directories keep their `/` and get no
/// space. `display` is never touched. This is the tab-dispatch decoration
/// for the BUILT-IN completion kinds (command/variable/plain-file), whose
/// `CandidateKind` reliably distinguishes directories. The space surfaces
/// only on a unique match — rustyline inserts the full `replacement` for a
/// single candidate and only the common prefix (space excluded) otherwise.
fn append_trailing_space_non_dir(mut cands: Vec<Candidate>) -> Vec<Candidate> {
    for c in &mut cands {
        if c.kind != CandidateKind::Directory {
            c.replacement.push(' ');
        }
    }
    cands
}

/// Completes a command name: builtins, keywords, user-defined functions and
/// aliases (`function_names`/`alias_names`), plus executables found in the
/// `:`-separated `path` — the full bash command-position candidate set.
pub fn complete_command(
    prefix: &str,
    path: &str,
    function_names: &[String],
    alias_names: &[String],
) -> Vec<Candidate> {
    let mut names: BTreeSet<String> = BTreeSet::new();

    for &builtin in crate::builtins::BUILTIN_NAMES {
        if builtin.starts_with(prefix) {
            names.insert(builtin.to_string());
        }
    }

    for &kw in COMPLETION_KEYWORDS {
        if kw.starts_with(prefix) {
            names.insert(kw.to_string());
        }
    }

    // User-defined commands: functions + aliases (bash completes these at the
    // command position alongside builtins/keywords/PATH executables).
    for name in function_names.iter().chain(alias_names.iter()) {
        if name.starts_with(prefix) {
            names.insert(name.clone());
        }
    }

    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if name.starts_with(prefix) && is_executable_file(&entry) {
                names.insert(name.to_string());
            }
        }
    }

    names
        .into_iter()
        .map(|n| Candidate {
            display: n.clone(),
            replacement: escape_filename(&n),
            kind: CandidateKind::Command,
        })
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
        .map(|n| Candidate {
            display: n.clone(),
            replacement: n,
            kind: CandidateKind::Variable,
        })
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
        let Some(name) = file_name.to_str() else {
            continue;
        };
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
        let kind = if is_dir {
            display.push('/');
            replacement.push('/');
            CandidateKind::Directory
        } else {
            CandidateKind::File
        };
        candidates.push(Candidate {
            display,
            replacement,
            kind,
        });
    }
    candidates.sort_by(|a, b| a.display.cmp(&b.display));
    candidates
}

/// Replaces a leading `~/` with `home/` (the only tilde form `_filedir`
/// emits to `compgen`). Other inputs pass through unchanged.
pub(crate) fn expand_tilde_prefix(s: &str, home: &str) -> String {
    match s.strip_prefix("~/") {
        Some(rest) if !home.is_empty() => format!("{home}/{rest}"),
        _ => s.to_string(),
    }
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
        ' ', '\t', '\'', '"', '\\', '$', ';', '&', '|', '<', '>', '(', ')', '*', '?', '[', ']',
        '~', '#', '`',
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

pub mod dispatch {
    //! Tab-time dispatch ladder. Decides which completion source
    //! handles the cursor position: variable, command-pos commands,
    //! a registered -F spec, default-spec fallback, or file completion.

    use super::*;
    use crate::completion_spec::{CompletionCtx, CompletionSpec, run_spec};
    use crate::shell_state::Shell;

    /// Entry point. Returns (start_offset, candidates) for rustyline.
    pub fn resolve(line: &str, pos: usize, shell: &mut Shell) -> (usize, Vec<Candidate>) {
        // Defensive clear: any leftover spec from a prior `compgen -F` call
        // (run from script context, not via tab dispatch) would otherwise
        // corrupt this dispatch's spec options via the take() at the end of
        // run_spec_with_empty_fallback. Belt-and-suspenders alongside
        // builtin_compgen's own save/restore.
        shell.current_completion_spec = None;
        let (word_start, start, context) = analyze_full(line, pos);

        // Path 1: variable context — always wins, no spec lookup.
        if let CompletionContext::Variable { prefix } = &context {
            let var_names: Vec<String> = shell.completion_var_names();
            return (
                start,
                append_trailing_space_non_dir(complete_variable(prefix, &var_names)),
            );
        }

        // Path 2: command position.
        if let CompletionContext::Command { prefix } = &context {
            // -E: empty command line + an -E spec.
            if prefix.is_empty()
                && line[..pos].trim().is_empty()
                && let Some(spec) = shell.completion_specs.empty_spec.clone()
            {
                let cands = run_spec_with_empty_fallback(&spec, line, pos, "", shell);
                return (start, cands);
            }
            let path = shell.get("PATH").unwrap_or("").to_string();
            let funcs: Vec<String> = shell.functions.keys().cloned().collect();
            let aliases: Vec<String> = shell.aliases.keys().cloned().collect();
            return (
                start,
                append_trailing_space_non_dir(complete_command(prefix, &path, &funcs, &aliases)),
            );
        }

        // Path 3: file/argument position.
        let CompletionContext::File { dir, prefix } = &context else {
            // analyze() returns one of three; unreachable.
            return (start, Vec::new());
        };

        let cmd_name = extract_command_name(&line[..pos]).unwrap_or_default();

        let spec_opt: Option<CompletionSpec> = shell
            .completion_specs
            .by_command
            .get(&cmd_name)
            .cloned()
            .or_else(|| shell.completion_specs.default_spec.clone());

        match spec_opt {
            Some(spec) => {
                let cands = run_spec_with_empty_fallback(&spec, line, pos, &cmd_name, shell);
                // Programmable completion replaces the WHOLE cur word with
                // full-path candidates (bash's model) -> anchor at word_start,
                // not the basename offset. Fixes `cd projects/projects`.
                (word_start, cands)
            }
            None => {
                // No spec at all -> existing default file completion
                // (basenames, anchored after the last '/').
                let home = shell.get("HOME").unwrap_or("").to_string();
                (
                    start,
                    append_trailing_space_non_dir(complete_file(dir, prefix, &home)),
                )
            }
        }
    }

    /// Runs `run_spec` on the spec, applies `-o filenames` rendering
    /// and the empty-fallback (`-o default` / `-o bashdefault`).
    fn run_spec_with_empty_fallback(
        spec: &CompletionSpec,
        line: &str,
        pos: usize,
        cmd_name: &str,
        shell: &mut Shell,
    ) -> Vec<Candidate> {
        let wordbreaks = shell.get("COMP_WORDBREAKS").unwrap_or(" \t\n").to_string();
        let (comp_words, comp_cword) = tokenize_comp_words(&line[..pos], &wordbreaks);
        let cur_word = comp_words.get(comp_cword).cloned().unwrap_or_default();
        let prev_word = if comp_cword > 0 {
            comp_words.get(comp_cword - 1).cloned().unwrap_or_default()
        } else {
            String::new()
        };
        let ctx = CompletionCtx {
            cmd_name: cmd_name.to_string(),
            cur_word: cur_word.clone(),
            prev_word,
            comp_words,
            comp_cword,
            comp_line: line.to_string(),
            comp_point: pos,
        };

        let raw_results = run_spec(spec, &ctx, shell);

        // Take the (possibly mutated) options from current_completion_spec
        // back if Task 6's compopt has touched them. If -F never ran,
        // current_completion_spec stays None and we use the original options.
        let effective_options = shell
            .current_completion_spec
            .take()
            .map(|s| s.options)
            .unwrap_or(spec.options);

        // Empty-fallback.
        let used_fallback = raw_results.is_empty();
        let after_fallback: Vec<String> = if used_fallback {
            if effective_options.default {
                file_completion_strings(&ctx.cur_word, shell)
            } else if effective_options.bashdefault {
                bashdefault_strings(line, pos, shell)
            } else {
                Vec::new()
            }
        } else {
            raw_results
        };

        // Filename rendering.
        let home = shell.get("HOME").unwrap_or("").to_string();
        let candidates: Vec<Candidate> = if effective_options.filenames {
            after_fallback
                .into_iter()
                .map(|name| {
                    let is_dir = std::fs::metadata(expand_tilde_prefix(&name, &home))
                        .map(|m| m.is_dir())
                        .unwrap_or(false);
                    // Display is readline's `printable_part`: for filename
                    // completions the Tab-Tab list shows only the text after
                    // the last `/` (the basename), never the directory prefix
                    // that is already on the line. The `replacement` below keeps
                    // the full path — only the shown label is stripped.
                    let base = name
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or(&name);
                    let display = if is_dir {
                        format!("{base}/")
                    } else {
                        base.to_string()
                    };
                    // Preserve a leading `~/` UNescaped (tilde-expansion intent);
                    // escaping it would yield `cd \~/projects` (a literal `~` dir).
                    // `-o noquote` suppresses metacharacter escaping entirely.
                    let mut replacement = if effective_options.noquote {
                        name.clone()
                    } else {
                        match name.strip_prefix("~/") {
                            Some(rest) => format!("~/{}", escape_filename(rest)),
                            None => escape_filename(&name),
                        }
                    };
                    if is_dir {
                        replacement.push('/');
                    } else if !effective_options.nospace {
                        replacement.push(' ');
                    }
                    Candidate {
                        display,
                        replacement,
                        kind: CandidateKind::Custom,
                    }
                })
                .collect()
        } else {
            after_fallback
                .into_iter()
                .map(|s| {
                    let mut replacement = s.clone();
                    // Fallback results are readline filename completions: a
                    // trailing `/` marks a directory, which never gets a space
                    // (bash's `-o default`). Raw COMPREPLY / `-W` words are NOT
                    // filename completions, so a `-W 'foo/'` word DOES get a
                    // space — hence the used_fallback guard (verified vs bash
                    // 5.2.21: `complete -W 'foobar/'` completes to `foobar/ `).
                    let is_fallback_dir = used_fallback && s.ends_with('/');
                    if !effective_options.nospace && !is_fallback_dir {
                        replacement.push(' ');
                    }
                    Candidate {
                        display: s,
                        replacement,
                        kind: CandidateKind::Custom,
                    }
                })
                .collect()
        };

        // Dedupe by replacement (stable, preserves first-seen order), then sort
        // unless `-o nosort` asked to keep the compspec's own ordering.
        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| seen.insert(c.replacement.clone()))
            .collect();
        if !effective_options.nosort {
            deduped.sort_by(|a, b| a.display.cmp(&b.display));
        }
        deduped
    }

    fn file_completion_strings(prefix: &str, shell: &Shell) -> Vec<String> {
        let home = shell.get("HOME").unwrap_or("").to_string();
        // Split the cur word into (dir, base) and re-prepend dir so the
        // empty-fallback yields FULL cur-relative paths, consistent with the
        // word_start anchor (matches compgen / bash's `-o default`).
        let (dir, base) = match prefix.rfind('/') {
            Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
            None => ("", prefix),
        };
        complete_file(dir, base, &home)
            .into_iter()
            .map(|c| format!("{dir}{}", c.replacement))
            .collect()
    }

    fn bashdefault_strings(line: &str, pos: usize, shell: &Shell) -> Vec<String> {
        let (_, ctx) = analyze(line, pos);
        match ctx {
            CompletionContext::Variable { prefix } => {
                let names: Vec<String> = shell.completion_var_names();
                complete_variable(&prefix, &names)
                    .into_iter()
                    .map(|c| c.replacement)
                    .collect()
            }
            CompletionContext::Command { prefix } => {
                let path = shell.get("PATH").unwrap_or("").to_string();
                let funcs: Vec<String> = shell.functions.keys().cloned().collect();
                let aliases: Vec<String> = shell.aliases.keys().cloned().collect();
                complete_command(&prefix, &path, &funcs, &aliases)
                    .into_iter()
                    .map(|c| c.replacement)
                    .collect()
            }
            CompletionContext::File { dir, prefix } => {
                let home = shell.get("HOME").unwrap_or("").to_string();
                complete_file(&dir, &prefix, &home)
                    .into_iter()
                    .map(|c| format!("{dir}{}", c.replacement))
                    .collect()
            }
        }
    }

    /// Extracts the command word (word 0) of the simple command that
    /// the cursor is in. Returns None if the cursor is before any
    /// command word (e.g., empty line). Skips assignment prefixes.
    fn extract_command_name(head: &str) -> Option<String> {
        // Walk forward through `head` tracking quoting; the most recent
        // separator (`;`, `|`, `&`, `\n`, `(`, `{`) bumps `start` past it.
        let mut start = 0usize;
        let bytes = head.as_bytes();
        let mut in_single = false;
        let mut in_double = false;
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            if !in_single && c == b'\\' {
                i += 2;
                continue;
            }
            if in_single {
                if c == b'\'' {
                    in_single = false;
                }
                i += 1;
                continue;
            }
            if in_double {
                if c == b'"' {
                    in_double = false;
                }
                i += 1;
                continue;
            }
            match c {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b';' | b'|' | b'&' | b'\n' | b'(' | b'{' => start = i + 1,
                _ => {}
            }
            i += 1;
        }
        // Skip leading whitespace, then take the first whitespace-delimited word.
        let region = &head[start..];
        let mut chars = region.char_indices().peekable();
        while let Some(&(_, c)) = chars.peek() {
            if c == ' ' || c == '\t' {
                chars.next();
            } else {
                break;
            }
        }
        let word_start = chars.peek().map(|(i, _)| *i).unwrap_or(region.len());
        let rest = &region[word_start..];
        let word_end = rest.find([' ', '\t']).unwrap_or(rest.len());
        let candidate = &rest[..word_end];

        // If it looks like an assignment prefix, the command is the NEXT word.
        if is_assignment(candidate) {
            let after = rest[word_end..].trim_start();
            let next_end = after.find([' ', '\t']).unwrap_or(after.len());
            if next_end == 0 {
                return None;
            }
            return Some(unquote_command(&after[..next_end]).to_string());
        }

        if candidate.is_empty() {
            None
        } else {
            Some(unquote_command(candidate).to_string())
        }
    }

    /// Strips a matching pair of leading/trailing single or double quotes
    /// from the command word so a registered spec for `git` is found when
    /// the user types `"git" arg<TAB>`. Bash performs full quote removal
    /// on the command word before lookup; we approximate by stripping one
    /// outer pair, which covers the common case.
    fn unquote_command(s: &str) -> &str {
        let bytes = s.as_bytes();
        if bytes.len() >= 2 {
            let first = bytes[0];
            let last = bytes[bytes.len() - 1];
            if (first == b'\'' || first == b'"') && first == last {
                return &s[1..s.len() - 1];
            }
        }
        s
    }

    /// Tokenizes a line into COMP_WORDS per the wordbreaks set.
    /// Whitespace bytes in `wordbreaks` act as plain separators;
    /// non-whitespace bytes EACH produce their own single-char word.
    /// Returns (words, cword) where cword is the index of the word the
    /// cursor is in.
    pub(crate) fn tokenize_comp_words(line: &str, wordbreaks: &str) -> (Vec<String>, usize) {
        let ws: Vec<char> = wordbreaks
            .chars()
            .filter(|c| c.is_ascii_whitespace())
            .collect();
        let non_ws: Vec<char> = wordbreaks
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect();
        let mut words: Vec<String> = Vec::new();
        let mut cur = String::new();
        for c in line.chars() {
            if ws.contains(&c) {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else if non_ws.contains(&c) {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
                words.push(c.to_string());
            } else {
                cur.push(c);
            }
        }
        // If the line ends mid-word, that word IS the cursor word.
        // If the line ends with a separator (or is empty), the cursor word
        // is "" and it occupies a new slot.
        let ends_with_sep = line
            .chars()
            .last()
            .map(|c| ws.contains(&c) || non_ws.contains(&c))
            .unwrap_or(true);
        if !cur.is_empty() {
            words.push(cur);
        } else if ends_with_sep || words.is_empty() {
            words.push(String::new());
        }
        let cword = words.len().saturating_sub(1);
        (words, cword)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;
    use crate::test_support::CWD_LOCK;
    use std::rc::Rc;

    #[test]
    fn analyze_empty_line_is_command() {
        let (start, ctx) = analyze("", 0);
        assert_eq!(start, 0);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: String::new()
            }
        );
    }

    #[test]
    fn analyze_first_word_is_command() {
        let (start, ctx) = analyze("ec", 2);
        assert_eq!(start, 0);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_command_is_file() {
        let (start, ctx) = analyze("echo fo", 7);
        assert_eq!(start, 5);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "fo".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_semicolon_is_command() {
        let (start, ctx) = analyze("echo hi; ec", 11);
        assert_eq!(start, 9);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_pipe_is_command() {
        let (_, ctx) = analyze("ls | gr", 7);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "gr".to_string()
            }
        );
    }

    #[test]
    fn analyze_inside_dollar_paren_is_command() {
        // `echo $(whi` — the cursor is in command position inside a `$(...)`
        // command substitution (bash completes commands there). #244.
        let (start, ctx) = analyze("echo $(whi", 10);
        assert_eq!(start, 7, "anchor is right after `$(`");
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_inside_backtick_is_command() {
        let (start, ctx) = analyze("echo `whi", 9);
        assert_eq!(start, 6, "anchor is right after the backtick");
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_inside_subshell_is_command() {
        let (_, ctx) = analyze("(whi", 4);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_inside_procsub_is_command() {
        let (_, ctx) = analyze("echo <(whi", 10);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
        let (_, ctx2) = analyze("cat >(whi", 9);
        assert_eq!(
            ctx2,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_closed_cmdsub_is_argument() {
        // A CLOSED `$(...)` followed by a space returns to argument position:
        // `whi` is an argument to `echo`, not a command.
        let (_, ctx) = analyze("echo $(ls) whi", 14);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_closed_backtick_is_argument() {
        // A CLOSED backtick pair leaves the following word an argument, not a
        // command — the backtick toggle must restore the outer position.
        let (_, ctx) = analyze("echo `ls` whi", 13);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_array_literal_is_not_command() {
        // `x=(whi` is an array literal, not a subshell — bash does NOT
        // command-complete there (`(` after `=`). It stays the assignment
        // word, which command-completion finds no match for.
        let (_, ctx) = analyze("x=(whi", 6);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "x=(whi".to_string()
            },
            "array literal must not become inner command position"
        );
    }

    #[test]
    fn analyze_comsub_in_assignment_rhs_is_command() {
        // `x=$(whi` — command substitution in an assignment RHS DOES command-
        // complete (`$` before `(`, not `=`).
        let (start, ctx) = analyze("x=$(whi", 7);
        assert_eq!(start, 4, "anchor right after `$(`");
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_dollar_brace_stays_variable() {
        // `${whi` is parameter expansion, NOT a command opener — must stay
        // variable completion (the `(` handling must not touch `{`).
        let (_, ctx) = analyze("echo ${whi", 10);
        assert_eq!(
            ctx,
            CompletionContext::Variable {
                prefix: "whi".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_assignment_word_is_command() {
        let (start, ctx) = analyze("FOO=bar ec", 10);
        assert_eq!(start, 8);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_then_keyword_is_command() {
        let (start, ctx) = analyze("if true; then ec", 16);
        assert_eq!(start, 14);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_do_keyword_is_command() {
        let (_, ctx) = analyze("for x in 1; do ec", 17);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_else_keyword_is_command() {
        let (_, ctx) = analyze("if x; then y; else ec", 21);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_elif_keyword_is_command() {
        let (_, ctx) = analyze("if x; then y; elif ec", 21);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_open_brace_keyword_is_command() {
        let (_, ctx) = analyze("{ ec", 4);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_after_fi_keyword_is_command() {
        // After `fi`, a separator is conventionally required, but treating the
        // next word as a command position is the more useful completion default
        // — it lets the user tab-complete `if x; then y; fi <TAB>` to a command.
        let (_, ctx) = analyze("if x; then y; fi ec", 19);
        assert_eq!(
            ctx,
            CompletionContext::Command {
                prefix: "ec".to_string()
            }
        );
    }

    #[test]
    fn analyze_variable_dollar() {
        let (start, ctx) = analyze("echo $HO", 8);
        assert_eq!(start, 6);
        assert_eq!(
            ctx,
            CompletionContext::Variable {
                prefix: "HO".to_string()
            }
        );
    }

    #[test]
    fn analyze_variable_braced() {
        let (start, ctx) = analyze("echo ${HO", 9);
        assert_eq!(start, 7);
        assert_eq!(
            ctx,
            CompletionContext::Variable {
                prefix: "HO".to_string()
            }
        );
    }

    #[test]
    fn analyze_variable_mid_word() {
        let (start, ctx) = analyze("echo foo$BA", 11);
        assert_eq!(start, 9);
        assert_eq!(
            ctx,
            CompletionContext::Variable {
                prefix: "BA".to_string()
            }
        );
    }

    #[test]
    fn analyze_variable_empty_prefix() {
        let (start, ctx) = analyze("echo $", 6);
        assert_eq!(start, 6);
        assert_eq!(
            ctx,
            CompletionContext::Variable {
                prefix: String::new()
            }
        );
    }

    #[test]
    fn analyze_path_splits_at_slash() {
        let (start, ctx) = analyze("cat src/le", 10);
        assert_eq!(start, 8);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: "src/".to_string(),
                prefix: "le".to_string()
            }
        );
    }

    #[test]
    fn analyze_command_with_slash_is_file() {
        let (start, ctx) = analyze("./scr", 5);
        assert_eq!(start, 2);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: "./".to_string(),
                prefix: "scr".to_string()
            }
        );
    }

    #[test]
    fn analyze_escaped_space_stays_in_word() {
        let (start, ctx) = analyze("cat my\\ fi", 10);
        assert_eq!(start, 4);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "my fi".to_string()
            }
        );
    }

    #[test]
    fn analyze_ignores_text_after_cursor() {
        let (_, ctx) = analyze("echo fo bar", 7);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "fo".to_string()
            }
        );
    }

    #[test]
    fn analyze_escaped_dollar_is_not_variable() {
        let (_, ctx) = analyze("echo \\$HOM", 10);
        assert!(matches!(ctx, CompletionContext::File { .. }));
    }

    #[test]
    fn analyze_full_reports_word_start_for_slash_word() {
        // `cd projects/sub`: whole word starts at 3; basename anchor after the slash.
        let (word_start, start, ctx) = analyze_full("cd projects/sub", 15);
        assert_eq!(word_start, 3);
        assert_eq!(start, 12); // just past "projects/"
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: "projects/".to_string(),
                prefix: "sub".to_string()
            }
        );
    }

    #[test]
    fn analyze_full_word_start_equals_start_without_slash() {
        let (word_start, start, _) = analyze_full("cd pr", 5);
        assert_eq!(word_start, 3);
        assert_eq!(start, 3);
    }

    #[test]
    fn spec_completion_anchors_at_word_start() {
        // A `complete -F _fake cd` returning FULL-PATH candidates must replace the
        // whole `projects/` word (anchor at word_start=3), not the after-slash
        // suffix — otherwise rustyline double-pastes -> `cd projects/projects`.
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "_fake() { COMPREPLY=(projects/alpha projects/beta); }",
            &mut sh,
            false,
        );
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "cd".to_string(),
                crate::completion_spec::CompletionSpec {
                    function: Some("_fake".to_string()),
                    ..Default::default()
                },
            );
        let (start, cands) = dispatch::resolve("cd projects/", 12, &mut sh);
        assert_eq!(start, 3, "must anchor at the start of `projects/`");
        let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(reps.contains(&"projects/alpha "), "{reps:?}");
        assert!(reps.contains(&"projects/beta "), "{reps:?}");
    }

    #[test]
    fn spec_default_sorts_wordlist_results() {
        // Without `-o nosort`, a `-W` wordlist's results are sorted.
        let mut sh = Shell::new();
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "foo".to_string(),
                crate::completion_spec::CompletionSpec {
                    wordlist: Some("banana apple cherry".to_string()),
                    ..Default::default()
                },
            );
        let (_start, cands) = dispatch::resolve("foo ", 4, &mut sh);
        let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(reps, vec!["apple ", "banana ", "cherry "]);
    }

    #[test]
    fn spec_nosort_preserves_wordlist_order() {
        // `-o nosort` keeps the compspec's own ordering (here, wordlist order).
        let mut sh = Shell::new();
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "foo".to_string(),
                crate::completion_spec::CompletionSpec {
                    wordlist: Some("banana apple cherry".to_string()),
                    options: crate::completion_spec::CompOptions {
                        nosort: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        let (_start, cands) = dispatch::resolve("foo ", 4, &mut sh);
        let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(reps, vec!["banana ", "apple ", "cherry "]);
    }

    #[test]
    fn spec_default_fallback_yields_full_relative_paths() {
        // `complete -o default -F _empty cd`: when the function yields nothing, the
        // empty-fallback must return FULL cur-relative paths (not basenames), so the
        // word_start anchor replaces the whole `<dir>/<base>` correctly.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        let base = dir.path().to_str().unwrap(); // absolute => no chdir needed
        let mut sh = Shell::new();
        let _ = crate::shell::process_line("_empty() { COMPREPLY=(); }", &mut sh, false);
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "cd".to_string(),
                crate::completion_spec::CompletionSpec {
                    function: Some("_empty".to_string()),
                    options: crate::completion_spec::CompOptions {
                        default: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        let line = format!("cd {base}/al");
        let pos = line.len();
        let (start, cands) = dispatch::resolve(&line, pos, &mut sh);
        assert_eq!(start, 3, "anchor at the start of the path word");
        let reps: Vec<String> = cands.iter().map(|c| c.replacement.clone()).collect();
        assert!(
            reps.iter().any(|r| r == &format!("{base}/alpha/")),
            "expected full path {base}/alpha/, got {reps:?}"
        );
    }

    #[test]
    fn expand_tilde_prefix_handles_leading_tilde_slash() {
        assert_eq!(
            expand_tilde_prefix("~/projects", "/home/x"),
            "/home/x/projects"
        );
        assert_eq!(expand_tilde_prefix("~/", "/home/x"), "/home/x/");
        assert_eq!(expand_tilde_prefix("projects/", "/home/x"), "projects/"); // no tilde
        assert_eq!(expand_tilde_prefix("~/p", ""), "~/p"); // empty home -> unchanged
    }

    #[test]
    fn spec_filenames_appends_slash_to_tilde_dir() {
        // A `-o filenames` candidate `~/projects` (a real dir under HOME) must get a
        // trailing `/` so `cd ~/<TAB>` can descend. COMPREPLY is single-quoted so the
        // shell does NOT tilde-expand it — the candidate stays literal `~/projects`.
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("projects")).unwrap();
        let mut sh = Shell::new();
        sh.set("HOME", home.path().to_str().unwrap().to_string());
        let _ = crate::shell::process_line("_t() { COMPREPLY=('~/projects'); }", &mut sh, false);
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "cd".to_string(),
                crate::completion_spec::CompletionSpec {
                    function: Some("_t".to_string()),
                    options: crate::completion_spec::CompOptions {
                        filenames: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        let (_start, cands) = dispatch::resolve("cd ~/", 5, &mut sh);
        let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(
            reps.contains(&"~/projects/"),
            "tilde dir should get trailing slash: {reps:?}"
        );
    }

    #[test]
    fn spec_filenames_display_is_basename_not_full_path() {
        // A `-o filenames` completion (bash-completion's default `complete -D`)
        // returns FULL-path candidates and replaces the whole word — but bash's
        // readline displays only the text after the last `/` (printable_part),
        // so the Tab-Tab list shows `huck-cli/`, not `crates/huck-cli/`. The
        // replacement stays the full path; only `display` is the basename.
        let root = tempfile::tempdir().unwrap();
        for d in ["huck-cli", "huck-engine", "huck-syntax"] {
            std::fs::create_dir_all(root.path().join("crates").join(d)).unwrap();
        }
        let mut sh = Shell::new();
        // COMPREPLY holds full paths, exactly as `_filedir` produces them.
        let _ = crate::shell::process_line(
            "_t() { COMPREPLY=('crates/huck-cli' 'crates/huck-engine' 'crates/huck-syntax'); }",
            &mut sh,
            false,
        );
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "ls".to_string(),
                crate::completion_spec::CompletionSpec {
                    function: Some("_t".to_string()),
                    options: crate::completion_spec::CompOptions {
                        filenames: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        // Run from a cwd where the candidate paths resolve (is_dir → trailing /).
        let _guard = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.path()).unwrap();
        let (_start, cands) = dispatch::resolve("ls crates/huck-", 15, &mut sh);
        std::env::set_current_dir(prev).unwrap();

        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        let reps: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        // Display: basename with a trailing slash (directories), NO dir prefix.
        assert!(
            displays.contains(&"huck-cli/"),
            "display should be the basename `huck-cli/`, got: {displays:?}"
        );
        assert!(
            !displays
                .iter()
                .any(|d| d.contains('/') && d.contains("crates")),
            "display must not carry the `crates/` prefix: {displays:?}"
        );
        // Replacement: still the full path (the whole word is replaced).
        assert!(
            reps.contains(&"crates/huck-cli/"),
            "replacement should keep the full path: {reps:?}"
        );
    }

    #[test]
    fn spec_default_appends_space_nospace_suppresses() {
        // A `-W` wordlist spec for `tcmd`. Default -> trailing space;
        // -o nospace -> no space. Directories are not involved here.
        let mut sh = Shell::new();
        let _ = crate::shell::process_line("_noop() { :; }", &mut sh, false);
        let spec_default = crate::completion_spec::CompletionSpec {
            wordlist: Some("foobar".to_string()),
            ..Default::default()
        };
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert("tcmd".to_string(), spec_default);
        let (_s, cands) = dispatch::resolve("tcmd foo", 8, &mut sh);
        let c = cands.iter().find(|c| c.display == "foobar").unwrap();
        assert_eq!(
            c.replacement, "foobar ",
            "default spec completion gets a space"
        );

        let spec_nospace = crate::completion_spec::CompletionSpec {
            wordlist: Some("foobar".to_string()),
            options: crate::completion_spec::CompOptions {
                nospace: true,
                ..Default::default()
            },
            ..Default::default()
        };
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert("tcmd".to_string(), spec_nospace);
        let (_s2, cands2) = dispatch::resolve("tcmd foo", 8, &mut sh);
        let c2 = cands2.iter().find(|c| c.display == "foobar").unwrap();
        assert_eq!(
            c2.replacement, "foobar",
            "-o nospace suppresses the trailing space"
        );
    }

    #[test]
    fn spec_filenames_dir_keeps_slash_even_under_nospace() {
        // A -o filenames -o nospace spec whose candidate is a real directory:
        // the `/` survives nospace; a real file under nospace gets no space.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("adir")).unwrap();
        std::fs::write(root.path().join("afile"), b"x").unwrap();
        let mut sh = Shell::new();
        let _ = crate::shell::process_line("_t() { COMPREPLY=('adir' 'afile'); }", &mut sh, false);
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "dcmd".to_string(),
                crate::completion_spec::CompletionSpec {
                    function: Some("_t".to_string()),
                    options: crate::completion_spec::CompOptions {
                        filenames: true,
                        nospace: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );
        let _guard = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.path()).unwrap();
        let (_s, cands) = dispatch::resolve("dcmd a", 6, &mut sh);
        std::env::set_current_dir(prev).unwrap();

        let d = cands.iter().find(|c| c.display == "adir/").unwrap();
        assert_eq!(d.replacement, "adir/", "directory keeps `/` under nospace");
        let f = cands.iter().find(|c| c.display == "afile").unwrap();
        assert_eq!(f.replacement, "afile", "file gets no space under nospace");
    }

    #[test]
    fn wordlist_word_ending_in_slash_still_gets_space() {
        // `-W 'foobar/'` is NOT a filename completion, so bash adds the space
        // even though it ends in `/`. Guards against a naive "no space after /".
        let mut sh = Shell::new();
        std::rc::Rc::make_mut(&mut sh.completion_specs)
            .by_command
            .insert(
                "wcmd".to_string(),
                crate::completion_spec::CompletionSpec {
                    wordlist: Some("foobar/".to_string()),
                    ..Default::default()
                },
            );
        let (_s, cands) = dispatch::resolve("wcmd foo", 8, &mut sh);
        let c = cands.iter().find(|c| c.display == "foobar/").unwrap();
        assert_eq!(
            c.replacement, "foobar/ ",
            "a -W word ending in / still gets a space"
        );
    }

    #[test]
    fn complete_command_matches_builtin_prefix() {
        let cands = complete_command("ec", "", &[], &[]);
        assert!(cands.iter().any(|c| c.replacement == "echo"));
    }

    #[test]
    fn complete_command_empty_prefix_includes_builtins() {
        let cands = complete_command("", "", &[], &[]);
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
        let cands = complete_command("huckcmd_", path, &[], &[]);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(
            names.contains(&"huckcmd_exe"),
            "exe should match: {names:?}"
        );
        assert!(
            !names.contains(&"huckcmd_plain"),
            "non-exe should not match"
        );
        assert!(
            !names.contains(&"huckcmd_subdir"),
            "subdir should not match"
        );
    }

    #[test]
    fn complete_command_results_are_sorted_and_unique() {
        let cands = complete_command("", "", &[], &[]);
        let mut sorted = cands.clone();
        sorted.sort_by(|a, b| a.replacement.cmp(&b.replacement));
        assert_eq!(cands, sorted);
    }

    #[test]
    fn complete_variable_matches_prefix() {
        let names = vec!["HOME".to_string(), "HOST".to_string(), "PATH".to_string()];
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
        assert!(
            cands.iter().any(|c| c.replacement == "homefile"),
            "{cands:?}"
        );
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
        let cands = complete_command("huckcmd_", path, &[], &[]);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"huckcmd_real"), "{names:?}");
        assert!(
            names.contains(&"huckcmd_link"),
            "symlinked exe should complete: {names:?}"
        );
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
        assert_eq!(
            cands[0].replacement, "linkdir/",
            "symlinked dir should get a trailing slash"
        );
    }

    #[test]
    fn analyze_redirect_target_is_file_not_command() {
        // `echo > lo` — the word after `>` is a redirect target (a file),
        // not a command.
        let (_, ctx) = analyze("echo > lo", 9);
        assert_eq!(
            ctx,
            CompletionContext::File {
                dir: String::new(),
                prefix: "lo".to_string()
            }
        );
    }

    #[test]
    fn dispatch_variable_context_bypasses_spec() {
        use std::cell::RefCell;
        let shell = Rc::new(RefCell::new(Shell::new()));
        shell.borrow_mut().set("MY_VAR", "x".to_string());
        // Register a spec for some command — it should NOT fire on $var.
        {
            let mut s = shell.borrow_mut();
            Rc::make_mut(&mut s.completion_specs).by_command.insert(
                "echo".to_string(),
                crate::completion_spec::CompletionSpec {
                    wordlist: Some("should_not_appear".to_string()),
                    ..Default::default()
                },
            );
        }
        let mut s = shell.borrow_mut();
        let (start, cands) = dispatch::resolve("echo $MY_V", 10, &mut s);
        assert_eq!(start, 6);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"MY_VAR "), "{names:?}");
        assert!(
            !names.contains(&"should_not_appear"),
            "spec fired on var: {names:?}"
        );
    }

    #[test]
    fn dispatch_command_position_uses_command_completion() {
        let mut shell = Shell::new();
        let (_, cands) = dispatch::resolve("ec", 2, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"echo "), "{names:?}");
    }

    #[test]
    fn complete_command_includes_functions_aliases_and_keywords() {
        let funcs = vec!["hello".to_string(), "helper".to_string()];
        let aliases = vec!["heya".to_string()];
        let cands = complete_command("he", "", &funcs, &aliases);
        let names: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(names.contains(&"hello"), "function missing: {names:?}");
        assert!(names.contains(&"helper"), "function missing: {names:?}");
        assert!(names.contains(&"heya"), "alias missing: {names:?}");
        // a keyword starting with the prefix is also completed
        let kw = complete_command("whi", "", &[], &[]);
        assert!(kw.iter().any(|c| c.display == "while"), "keyword missing");
        // a function NOT matching the prefix is excluded
        let only = complete_command("hell", "", &funcs, &aliases);
        let only_names: Vec<&str> = only.iter().map(|c| c.display.as_str()).collect();
        assert_eq!(only_names, vec!["hello"]);
    }

    #[test]
    fn comp_wordbreaks_initialized_to_bash_default() {
        // Root cause of the "git <tab> breaks all completion" bug: huck left
        // COMP_WORDBREAKS unset, so a completion script's
        // `COMP_WORDBREAKS="$COMP_WORDBREAKS:"` (git does this) turned it into
        // just ":" — dropping whitespace, so COMP_WORDS tokenization no longer
        // split on spaces and every subsequent spec completion returned nothing.
        let shell = Shell::new();
        assert_eq!(shell.get("COMP_WORDBREAKS"), Some(" \t\n\"'@><=;|&(:"));
    }

    #[test]
    fn spec_completion_survives_colon_appended_wordbreaks() {
        // Simulates git's `COMP_WORDBREAKS="$COMP_WORDBREAKS:"`: with the
        // whitespace-inclusive default, appending ':' must keep `cd ~/`
        // tokenizing on the space (cur word "~/"), so the spec still completes.
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        shell.export_set(
            "HOME",
            std::env::var("HOME").unwrap_or_else(|_| "/root".into()),
        );
        // Append ':' the way a completion script does.
        let wb = format!("{}:", shell.get("COMP_WORDBREAKS").unwrap());
        shell.export_set("COMP_WORDBREAKS", wb);
        // A `-o default` spec for cd (so it routes through run_spec_with_empty_fallback).
        Rc::make_mut(&mut shell.completion_specs).by_command.insert(
            "cd".to_string(),
            crate::completion_spec::CompletionSpec {
                options: crate::completion_spec::CompOptions {
                    default: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let (_, cands) = dispatch::resolve("cd ~/", 5, &mut shell);
        assert!(
            !cands.is_empty(),
            "cd ~/ completion broke with ':'-appended COMP_WORDBREAKS"
        );
    }

    #[test]
    fn dispatch_command_position_completes_a_user_alias() {
        // Regression: command-position TAB must include user-defined command
        // names (functions + aliases), not just builtins + PATH. (Functions go
        // through the identical wiring; an alias is trivial to set up here.)
        let mut shell = Shell::new();
        shell
            .aliases
            .insert("myuniquealias".to_string(), "ls".to_string());
        let (_, cands) = dispatch::resolve("myuniquea", 9, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(
            names.contains(&"myuniquealias "),
            "alias not completed: {names:?}"
        );
    }

    #[test]
    fn dispatch_arg_position_uses_spec() {
        let mut shell = Shell::new();
        Rc::make_mut(&mut shell.completion_specs).by_command.insert(
            "myc".to_string(),
            crate::completion_spec::CompletionSpec {
                wordlist: Some("alpha alpine beta".to_string()),
                ..Default::default()
            },
        );
        let (_, cands) = dispatch::resolve("myc al", 6, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(names, vec!["alpha ", "alpine "]);
    }

    #[test]
    fn dispatch_falls_back_to_file_when_no_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("targetfile"), b"").unwrap();
        let mut shell = Shell::new();
        let line = format!("ls {}/targ", dir.path().to_str().unwrap());
        let pos = line.len();
        let (_, cands) = dispatch::resolve(&line, pos, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(names.contains(&"targetfile "), "{names:?}");
    }

    #[test]
    fn dispatch_o_default_falls_back_on_empty() {
        let _g = CWD_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alphafile"), b"").unwrap();
        let prior_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let mut shell = Shell::new();
        let spec = crate::completion_spec::CompletionSpec {
            wordlist: Some("nothing_matches".to_string()),
            options: crate::completion_spec::CompOptions {
                default: true,
                ..Default::default()
            },
            ..Default::default()
        };
        Rc::make_mut(&mut shell.completion_specs)
            .by_command
            .insert("mycmd".to_string(), spec);

        let (_, cands) = dispatch::resolve("mycmd alpha", 11, &mut shell);
        std::env::set_current_dir(prior_cwd).unwrap();

        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert!(
            names.contains(&"alphafile "),
            "fallback didn't fire: {names:?}"
        );
    }

    #[test]
    fn dispatch_d_default_spec_applies_when_no_match() {
        let mut shell = Shell::new();
        Rc::make_mut(&mut shell.completion_specs).default_spec =
            Some(crate::completion_spec::CompletionSpec {
                wordlist: Some("dfault".to_string()),
                ..Default::default()
            });
        let (_, cands) = dispatch::resolve("randomcmd df", 12, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(names, vec!["dfault "]);
    }

    #[test]
    fn tokenize_default_wordbreaks_is_whitespace() {
        let (words, cword) = dispatch::tokenize_comp_words("git checkout main", " \t\n");
        assert_eq!(words, vec!["git", "checkout", "main"]);
        assert_eq!(cword, 2);
    }

    #[test]
    fn tokenize_custom_wordbreaks_splits_on_colon() {
        let (words, _cword) = dispatch::tokenize_comp_words("user:pass", " \t\n:");
        assert_eq!(words, vec!["user", ":", "pass"]);
    }

    #[test]
    fn tokenize_trailing_separator_means_empty_cursor_word() {
        let (words, cword) = dispatch::tokenize_comp_words("cmd ", " \t\n");
        assert_eq!(words, vec!["cmd", ""]);
        assert_eq!(cword, 1);
    }

    #[test]
    fn dispatch_resolve_starts_with_clean_current_spec() {
        let _g = CWD_LOCK.lock().unwrap();
        // Simulate a leaked spec from a prior compgen -F (without going
        // through the actual compgen path). Without the defensive clear
        // at the top of dispatch::resolve, the .take() inside
        // run_spec_with_empty_fallback would observe this leaked spec
        // (all-default options) and use it to override the registered
        // spec's real options.
        let mut shell = Shell::new();
        shell.current_completion_spec = Some(crate::completion_spec::CompletionSpec::default());
        // Register a spec whose -o filenames would be silently lost if
        // the leaked all-default spec replaced it. We exercise filenames
        // rendering by completing a real directory.
        let tempdir = tempfile::tempdir().unwrap();
        std::fs::create_dir(tempdir.path().join("subd")).unwrap();
        let prior = std::env::current_dir().unwrap();
        std::env::set_current_dir(tempdir.path()).unwrap();

        Rc::make_mut(&mut shell.completion_specs).by_command.insert(
            "mycmd".to_string(),
            crate::completion_spec::CompletionSpec {
                wordlist: Some("subd".to_string()),
                options: crate::completion_spec::CompOptions {
                    filenames: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let (_, cands) = dispatch::resolve("mycmd su", 8, &mut shell);
        std::env::set_current_dir(prior).unwrap();

        // After dispatch returned, the leaked slot must have been cleared
        // up front and remain cleared (no -F ran here, so nothing put
        // anything back).
        assert!(
            shell.current_completion_spec.is_none(),
            "dispatch should leave the slot clean when no -F ran",
        );
        // Filenames rendering should add a trailing / to the directory —
        // proving the registered spec's options.filenames was used, not
        // the leaked default-options spec.
        let displays: Vec<&str> = cands.iter().map(|c| c.display.as_str()).collect();
        assert!(
            displays.contains(&"subd/"),
            "filenames-mode trailing / not applied; leaked spec must have \
             overridden the registered spec's options: {displays:?}",
        );
    }

    #[test]
    fn extract_command_name_strips_outer_quotes() {
        // Quoted command name like "git" or 'git' should reach its
        // registered spec — the registry is keyed by the unquoted name.
        let mut shell = Shell::new();
        Rc::make_mut(&mut shell.completion_specs).by_command.insert(
            "mycmd".to_string(),
            crate::completion_spec::CompletionSpec {
                wordlist: Some("alpha".to_string()),
                ..Default::default()
            },
        );
        let (_, cands) = dispatch::resolve("\"mycmd\" al", 10, &mut shell);
        let names: Vec<&str> = cands.iter().map(|c| c.replacement.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha "],
            "quoted command name should map to its registered spec",
        );
    }

    #[test]
    fn bash_source_lineno_tab_complete_when_set() {
        use crate::shell_state::{Frame, FrameKind};
        let mut sh = Shell::new();
        sh.call_stack.push(Frame {
            funcname: "f".into(),
            source: "lib.sh".into(),
            call_line: 3,
            kind: FrameKind::Function,
        });
        sh.sync_call_arrays();
        let names: Vec<String> = sh.var_names().map(|s| s.to_string()).collect();
        let bash_cands: Vec<String> = complete_variable("BASH_", &names)
            .into_iter()
            .map(|c| c.replacement.to_string())
            .collect();
        assert!(
            bash_cands.iter().any(|c| c.contains("BASH_SOURCE")),
            "BASH_SOURCE should complete, got {bash_cands:?}"
        );
        assert!(
            bash_cands.iter().any(|c| c.contains("BASH_LINENO")),
            "BASH_LINENO should complete, got {bash_cands:?}"
        );
        let fn_cands: Vec<String> = complete_variable("FUNC", &names)
            .into_iter()
            .map(|c| c.replacement.to_string())
            .collect();
        assert!(
            fn_cands.iter().any(|c| c.contains("FUNCNAME")),
            "FUNCNAME should complete, got {fn_cands:?}"
        );
    }

    #[test]
    fn dispatch_command_appends_trailing_space() {
        let mut sh = Shell::new();
        // `ech` uniquely prefixes the `echo` builtin among commands here.
        let (_start, cands) = dispatch::resolve("ech", 3, &mut sh);
        let echo = cands
            .iter()
            .find(|c| c.display == "echo")
            .expect("echo candidate present");
        assert_eq!(
            echo.replacement, "echo ",
            "command replacement gets a trailing space"
        );
        assert_eq!(echo.display, "echo", "display stays clean (no space)");
    }

    #[test]
    fn dispatch_variable_appends_trailing_space() {
        let mut sh = Shell::new();
        sh.set("MYUNIQUEVAR", "x".to_string());
        let (_start, cands) = dispatch::resolve("echo $MYUNIQ", 11, &mut sh);
        let v = cands
            .iter()
            .find(|c| c.display == "MYUNIQUEVAR")
            .expect("variable candidate present");
        assert_eq!(v.replacement, "MYUNIQUEVAR ");
        assert_eq!(v.display, "MYUNIQUEVAR");
    }

    #[test]
    fn dispatch_plain_file_space_but_dir_slash() {
        // No completion spec registered -> the None (plain-file) branch.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("solofile.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("solodir")).unwrap();
        let mut sh = Shell::new();
        let _guard = crate::test_support::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let (_s1, file_cands) = dispatch::resolve("cat solof", 9, &mut sh);
        let (_s2, dir_cands) = dispatch::resolve("cat solod", 9, &mut sh);
        std::env::set_current_dir(prev).unwrap();

        let f = file_cands
            .iter()
            .find(|c| c.display == "solofile.txt")
            .unwrap();
        assert_eq!(f.replacement, "solofile.txt ", "regular file gets a space");
        let d = dir_cands.iter().find(|c| c.display == "solodir/").unwrap();
        assert_eq!(d.replacement, "solodir/", "directory keeps `/`, no space");
    }
}
