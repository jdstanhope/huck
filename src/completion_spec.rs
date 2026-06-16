//! Completion-spec data and the `run_spec()` candidate generator.
//!
//! A `CompletionSpec` is what the `complete` builtin builds and stores
//! per command name. `run_spec()` is the pure-ish function that
//! turns a spec plus a completion context into a list of candidate
//! strings. It is reused by tab-time dispatch (`completion.rs`) AND by
//! the `compgen` builtin.

use std::collections::HashMap;

use crate::shell_state::Shell;

/// Per-command + default + empty completion specs.
#[derive(Debug, Default, Clone)]
pub struct CompletionSpecs {
    pub by_command: HashMap<String, CompletionSpec>,
    pub default_spec: Option<CompletionSpec>,
    pub empty_spec: Option<CompletionSpec>,
}

/// A single completion spec. Multiple content generators (`-F`, `-W`,
/// `-G`, `-A`) may be set simultaneously; their results are concatenated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionSpec {
    pub function: Option<String>,
    pub wordlist: Option<String>,
    pub glob: Option<String>,
    pub actions: Vec<Action>,

    pub prefix: Option<String>,
    pub suffix: Option<String>,
    pub filter: Option<String>,

    pub options: CompOptions,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompOptions {
    pub default: bool,
    pub nospace: bool,
    pub filenames: bool,
    pub bashdefault: bool,
    pub dirnames: bool,
    /// `-o nosort`: do not sort the completion results — preserve the order the
    /// compspec (function / wordlist / action) produced them in.
    pub nosort: bool,
    /// `-o noquote`: do not quote shell metacharacters in filename completions.
    pub noquote: bool,
    /// `-o plusdirs`: in addition to the compspec's matches, also complete
    /// directory names for the current word and append them to the results.
    pub plusdirs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    File,
    Directory,
    Command,
    Function,
    Variable,
    Alias,
    Builtin,
    Keyword,
    Arrayvar,
    Binding,
    Disabled,
    Enabled,
    Export,
    Group,
    Helptopic,
    Hostname,
    Job,
    Running,
    Service,
    Setopt,
    Shopt,
    Signal,
    Stopped,
    User,
}

impl Action {
    /// Parses the short-form name accepted by `complete -A` / `compgen -A`.
    /// Returns `None` for unsupported actions (caller surfaces the diag).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Action::File),
            "directory" => Some(Action::Directory),
            "command" => Some(Action::Command),
            "function" => Some(Action::Function),
            "variable" => Some(Action::Variable),
            "alias" => Some(Action::Alias),
            "builtin" => Some(Action::Builtin),
            "keyword" => Some(Action::Keyword),
            "arrayvar" => Some(Action::Arrayvar),
            "binding" => Some(Action::Binding),
            "disabled" => Some(Action::Disabled),
            "enabled" => Some(Action::Enabled),
            "export" => Some(Action::Export),
            "group" => Some(Action::Group),
            "helptopic" => Some(Action::Helptopic),
            "hostname" => Some(Action::Hostname),
            "job" => Some(Action::Job),
            "running" => Some(Action::Running),
            "service" => Some(Action::Service),
            "setopt" => Some(Action::Setopt),
            "shopt" => Some(Action::Shopt),
            "signal" => Some(Action::Signal),
            "stopped" => Some(Action::Stopped),
            "user" => Some(Action::User),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Action::File => "file",
            Action::Directory => "directory",
            Action::Command => "command",
            Action::Function => "function",
            Action::Variable => "variable",
            Action::Alias => "alias",
            Action::Builtin => "builtin",
            Action::Keyword => "keyword",
            Action::Arrayvar => "arrayvar",
            Action::Binding => "binding",
            Action::Disabled => "disabled",
            Action::Enabled => "enabled",
            Action::Export => "export",
            Action::Group => "group",
            Action::Helptopic => "helptopic",
            Action::Hostname => "hostname",
            Action::Job => "job",
            Action::Running => "running",
            Action::Service => "service",
            Action::Setopt => "setopt",
            Action::Shopt => "shopt",
            Action::Signal => "signal",
            Action::Stopped => "stopped",
            Action::User => "user",
        }
    }
}

/// The set of bash shell keywords huck recognizes. Used by `-A keyword`.
const SHELL_KEYWORDS: &[&str] = &[
    "!", "[[", "]]", "case", "do", "done", "elif", "else", "esac",
    "fi", "for", "function", "if", "in", "select", "then", "until", "while", "{", "}",
];

/// Completion context for a single `run_spec` call.
#[derive(Debug, Clone)]
pub struct CompletionCtx {
    /// The command name (word 0 of the simple command).
    pub cmd_name: String,
    /// The word the cursor is on (possibly empty).
    pub cur_word: String,
    /// The word at index COMP_CWORD - 1, or "" if cursor is on word 0.
    pub prev_word: String,
    /// Full COMP_WORDS list including separator-words from COMP_WORDBREAKS.
    pub comp_words: Vec<String>,
    /// Index of the cursor word in `comp_words`.
    pub comp_cword: usize,
    /// The full input line.
    pub comp_line: String,
    /// Byte offset of the cursor in the line.
    pub comp_point: usize,
}

/// Runs every generator in the spec, decorates, filters, and returns
/// the resulting candidate strings. `-F` invocation is delegated to
/// `call_completion_function` (filled in by Task 4); for now it returns
/// an empty vec.
///
/// Note: this does NOT apply `-o filenames` rendering or `-o default`/
/// `bashdefault` empty-fallback. Those are the caller's responsibility
/// (Task 5) because they depend on rustyline / dispatch-ladder state.
pub fn run_spec(
    spec: &CompletionSpec,
    ctx: &CompletionCtx,
    shell: &mut Shell,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // -F func: invoke the completion function. Sets COMP_*, runs body,
    // reads COMPREPLY back. See call_completion_function below.
    if let Some(func_name) = &spec.function {
        let mut from_func = call_completion_function(func_name, spec, ctx, shell);
        out.append(&mut from_func);
    }

    // -W wordlist: IFS-split the raw wordlist string AT USE TIME and
    // keep entries whose prefix matches cur_word.
    if let Some(wordlist) = &spec.wordlist {
        let ifs = shell.ifs();
        let words = split_wordlist(wordlist, &ifs);
        for w in words {
            if w.starts_with(&ctx.cur_word) {
                out.push(w);
            }
        }
    }

    // -G glob: shell-glob expansion against CWD; keep matches whose
    // basename starts with cur_word.
    if let Some(glob_pat) = &spec.glob {
        for matched in expand_glob(glob_pat) {
            if filename_matches_prefix(&matched, &ctx.cur_word) {
                out.push(matched);
            }
        }
    }

    // -A action: enumerate predefined sources, filtered by cur_word.
    for action in &spec.actions {
        let mut from_action = complete_action(*action, &ctx.cur_word, shell);
        out.append(&mut from_action);
    }

    // -X pattern filter. `pat` removes matches; `!pat` keeps only matches.
    if let Some(filter) = &spec.filter {
        let (pattern, invert) = match filter.strip_prefix('!') {
            Some(rest) => (rest, true),
            None => (filter.as_str(), false),
        };
        out.retain(|s| {
            let matches = glob_match(pattern, s);
            if invert { matches } else { !matches }
        });
    }

    // -P prefix / -S suffix decoration.
    if let Some(prefix) = &spec.prefix {
        for s in out.iter_mut() {
            *s = format!("{prefix}{s}");
        }
    }
    if let Some(suffix) = &spec.suffix {
        for s in out.iter_mut() {
            *s = format!("{s}{suffix}");
        }
    }

    // -o plusdirs: append directory-name completions for the current word.
    // bash adds these AFTER the compspec's own matches and does NOT apply the
    // -P/-S/-X decorations to them, so this runs last.
    if spec.options.plusdirs {
        let mut dirs = complete_action(Action::Directory, &ctx.cur_word, shell);
        out.append(&mut dirs);
    }

    out
}

/// Invokes a -F completion function. Sets COMP_*, positional params,
/// calls the function via the executor, reads COMPREPLY, then restores
/// COMPREPLY and $?. The live spec is stashed in
/// `shell.current_completion_spec` so a `compopt` inside the function
/// (Task 6) can mutate it; dispatch (Task 5) `.take()`s it back out.
/// `COMP_*` shell vars are LEFT SET after return (matches bash —
/// they're meant to remain readable until next completion).
///
/// Known divergence from bash: `exit` inside a completion function
/// does NOT terminate the shell. rustyline's `complete()` returns
/// `rustyline::Result<...>` and has no graceful way to propagate
/// `ExecOutcome::Exit`. The function instead returns immediately with
/// the COMPREPLY-so-far.
fn call_completion_function(
    func_name: &str,
    spec: &CompletionSpec,
    ctx: &CompletionCtx,
    shell: &mut Shell,
) -> Vec<String> {
    // 1. Snapshot variables we'll mutate so we can restore on return.
    //    (Positional args are saved by call_function internally.)
    let saved_last_status = shell.last_status();
    let saved_reply = shell.snapshot_var("COMPREPLY");

    // 2. Set COMP_* shell vars.
    shell.set("COMP_LINE", ctx.comp_line.clone());
    shell.set("COMP_POINT", ctx.comp_point.to_string());
    shell.set("COMP_CWORD", ctx.comp_cword.to_string());

    // COMP_WORDS as indexed array.
    let mut words_map: std::collections::BTreeMap<usize, String> =
        std::collections::BTreeMap::new();
    for (i, w) in ctx.comp_words.iter().enumerate() {
        words_map.insert(i, w.clone());
    }
    let _ = shell.replace_array("COMP_WORDS", words_map);

    // Clear COMPREPLY so the function can detect "not set yet" if it
    // wants — and so an empty result is unambiguous.
    shell.unset("COMPREPLY");

    // 3. Stash the spec for compopt-in-function mutation (Task 6 reads
    //    this). Dispatch (Task 5) `.take()`s it back out — we
    //    intentionally LEAVE it set after the function returns.
    shell.current_completion_spec = Some(spec.clone());

    // 4. Build positional args [cmd_name, cur_word, prev_word].
    let pos_args = vec![
        ctx.cmd_name.clone(),
        ctx.cur_word.clone(),
        ctx.prev_word.clone(),
    ];

    // 5. Invoke. Exit/Continue/Return all treated as "function finished"
    //    — see header comment about the rustyline-propagation divergence.
    // Run the completer WITHOUT job control: a completer's external commands /
    // pipelines must not setpgid / hand off the controlling terminal while huck
    // is mid-line-edit (that deadlocks — M-116). bash runs completion functions
    // without job control.
    let saved_in_completion = shell.in_completion;
    shell.in_completion = true;
    let _outcome = crate::executor::call_function_body(func_name, pos_args, shell);
    shell.in_completion = saved_in_completion;

    // 6. Read COMPREPLY (values in index order).
    let reply_values: Vec<String> = match shell.get_array("COMPREPLY") {
        Some(map) => map.values().cloned().collect(),
        None => Vec::new(),
    };

    // 7. Restore COMPREPLY (compgen / next completion expects a clean slot).
    shell.restore_var("COMPREPLY", saved_reply);

    // 8. Restore $?. Completion functions do NOT pollute the user's $?.
    shell.set_last_status(saved_last_status);

    // 9. Drain any pending fatal PE error from inside the function so
    //    the next prompt is clean.
    let _ = shell.take_pending_fatal_pe_error();

    reply_values
}

/// Splits a -W wordlist on the IFS bytes. Whitespace IFS bytes (space,
/// tab, newline) collapse runs and strip leading/trailing; non-whitespace
/// IFS bytes each delimit a single field.
fn split_wordlist(wordlist: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![wordlist.to_string()];
    }
    let ws: Vec<char> = ifs.chars().filter(|c| c.is_ascii_whitespace()).collect();
    let non_ws: Vec<char> = ifs.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = wordlist.chars().peekable();
    // Strip leading whitespace-IFS.
    while let Some(&c) = chars.peek() {
        if ws.contains(&c) { chars.next(); } else { break; }
    }
    while let Some(c) = chars.next() {
        if ws.contains(&c) {
            // Collapse run of whitespace-IFS.
            while let Some(&n) = chars.peek() {
                if ws.contains(&n) { chars.next(); } else { break; }
            }
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if non_ws.contains(&c) {
            // Each non-ws IFS byte ends a field.
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn expand_glob(pattern: &str) -> Vec<String> {
    match glob::glob(pattern) {
        Ok(paths) => paths
            .filter_map(|p| p.ok())
            .filter_map(|p| p.to_str().map(|s| s.to_string()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn filename_matches_prefix(path: &str, prefix: &str) -> bool {
    let basename = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    basename.starts_with(prefix)
}

fn glob_match(pattern: &str, candidate: &str) -> bool {
    // The glob crate lacks POSIX [:name:] classes; route class-bearing
    // patterns through the own-matcher (case-sensitive, matching the
    // default glob::Pattern::matches below).
    if crate::glob_match::has_posix_class(pattern) {
        return crate::glob_match::extglob_match(pattern, candidate, false);
    }
    let pattern = crate::glob_match::translate_bracket_negation(pattern);
    match glob::Pattern::new(&pattern) {
        Ok(p) => p.matches(candidate),
        Err(_) => false,
    }
}

fn complete_action(action: Action, prefix: &str, shell: &Shell) -> Vec<String> {
    let home = shell.get("HOME").unwrap_or("").to_string();
    match action {
        Action::File => list_dir_with_path_prefix(prefix, false, &home),
        Action::Directory => list_dir_with_path_prefix(prefix, true, &home),
        Action::Command => {
            // Reuse src/completion.rs::complete_command which already
            // walks PATH + builtins. Use `display` (raw name); the
            // `replacement` field is escape_filename()'d for rustyline
            // and is not what compgen / -A command consumers expect.
            let path = shell.get("PATH").unwrap_or("").to_string();
            let funcs: Vec<String> = shell.functions.keys().cloned().collect();
            let aliases: Vec<String> = shell.aliases.keys().cloned().collect();
            crate::completion::complete_command(prefix, &path, &funcs, &aliases)
                .into_iter()
                .map(|c| c.display)
                .collect()
        }
        Action::Function => {
            let mut names: Vec<String> = shell
                .functions
                .keys()
                .filter(|n| n.starts_with(prefix))
                .cloned()
                .collect();
            names.sort();
            names
        }
        Action::Variable => {
            let mut names: Vec<String> = shell
                .completion_var_names()
                .into_iter()
                .filter(|n| n.starts_with(prefix))
                .collect();
            names.sort();
            names.dedup();
            names
        }
        Action::Alias => {
            let mut names: Vec<String> = shell
                .aliases
                .keys()
                .filter(|n| n.starts_with(prefix))
                .cloned()
                .collect();
            names.sort();
            names
        }
        Action::Builtin => {
            let mut names: Vec<String> = crate::builtins::BUILTIN_NAMES
                .iter()
                .filter(|n| n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names
        }
        Action::Keyword => SHELL_KEYWORDS
            .iter()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(),
        Action::Setopt => crate::builtins::seto_option_names()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(), // table order — bash compgen does not sort
        Action::Shopt => crate::shell_state::SHOPT_TABLE
            .iter()
            .map(|o| o.name)
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(), // table order
        Action::Helptopic => crate::builtins::help_topic_names()
            .filter(|n| n.starts_with(prefix))
            .map(|s| s.to_string())
            .collect(),
        Action::Signal => crate::builtins::signal_names()
            .into_iter()
            .filter(|n| n.starts_with(prefix))
            .collect(),
        Action::Export => {
            let mut names: Vec<String> = shell
                .var_names()
                .filter(|n| shell.is_exported(n) && n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names.dedup();
            names
        }
        Action::Arrayvar => {
            let mut names: Vec<String> = shell
                .array_var_names()
                .into_iter()
                .filter(|n| n.starts_with(prefix))
                .collect();
            names.sort();
            names
        }
        Action::Enabled => {
            let mut names: Vec<String> = crate::builtins::BUILTIN_NAMES
                .iter()
                .filter(|n| n.starts_with(prefix))
                .map(|s| s.to_string())
                .collect();
            names.sort();
            names
        }
        Action::Job => shell
            .jobs
            .jobs()
            .iter()
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Running => shell
            .jobs
            .jobs()
            .iter()
            .filter(|j| matches!(j.state, crate::jobs::JobState::Running))
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Stopped => shell
            .jobs
            .jobs()
            .iter()
            .filter(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
            .map(|j| j.command.clone())
            .filter(|c| c.starts_with(prefix))
            .collect(),
        Action::Disabled
        | Action::Binding
        | Action::Hostname
        | Action::User
        | Action::Group
        | Action::Service => Vec::new(),
    }
}

fn list_dir_filtered(dir: &str, prefix: &str, dirs_only: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut out: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.starts_with(prefix) {
            continue;
        }
        if name.starts_with('.') && !show_hidden {
            continue;
        }
        if dirs_only {
            let is_dir = std::fs::metadata(entry.path())
                .map(|m| m.is_dir())
                .unwrap_or(false);
            if !is_dir {
                continue;
            }
        }
        out.push(name.to_string());
    }
    out.sort();
    out
}

/// Splits `prefix` at the last `/` into a directory portion and a
/// basename portion, scans the directory for entries whose basename
/// starts with the basename portion, then re-prepends the directory
/// portion to each result so the final string round-trips with the
/// caller's input. This is what bash's `compgen -A file` /
/// `compgen -A directory` does: a prefix of `src/comp` reads `src/`
/// and prefix-matches basenames against `comp`.
fn list_dir_with_path_prefix(prefix: &str, dirs_only: bool, home: &str) -> Vec<String> {
    let (dir, base) = match prefix.rfind('/') {
        Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
        None => ("", prefix),
    };
    let scan_raw = if dir.is_empty() { "." } else { dir };
    // _filedir passes a literal `~/…`; expand it for the read_dir, but
    // re-prepend the ORIGINAL `dir` so candidates come back as `~/projects`.
    let scan_dir = crate::completion::expand_tilde_prefix(scan_raw, home);
    let bare_results = list_dir_filtered(&scan_dir, base, dirs_only);
    bare_results
        .into_iter()
        .map(|name| format!("{dir}{name}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;
    use crate::test_support::CWD_LOCK;

    #[test]
    fn action_parse_round_trips_all_24() {
        let names = [
            "file", "directory", "command", "function", "variable", "alias", "builtin", "keyword",
            "arrayvar", "binding", "disabled", "enabled", "export", "group", "helptopic",
            "hostname", "job", "running", "service", "setopt", "shopt", "signal", "stopped", "user",
        ];
        for n in names {
            let a = Action::parse(n).unwrap_or_else(|| panic!("parse failed: {n}"));
            assert_eq!(a.as_str(), n, "round-trip mismatch for {n}");
        }
        assert_eq!(Action::parse("bogus_action"), None);
    }

    #[test]
    fn enumerate_setopt_shopt_table_order_and_membership() {
        let sh = Shell::new();
        let setopt = complete_action(Action::Setopt, "", &sh);
        assert!(setopt.contains(&"errexit".to_string()));
        assert_eq!(setopt[0], "allexport"); // table order, NOT sorted
        let shopt = complete_action(Action::Shopt, "", &sh);
        assert!(shopt.contains(&"nullglob".to_string()));
        assert_eq!(
            &shopt[0..2],
            &["autocd".to_string(), "assoc_expand_once".to_string()]
        ); // table order
        assert_eq!(
            complete_action(Action::Shopt, "null", &sh),
            vec!["nullglob".to_string()]
        );
    }

    #[test]
    fn enumerate_signal_helptopic_enabled() {
        let sh = Shell::new();
        assert!(complete_action(Action::Signal, "SIGIN", &sh) == vec!["SIGINT".to_string()]);
        assert!(!complete_action(Action::Helptopic, "", &sh).is_empty());
        assert!(complete_action(Action::Enabled, "ech", &sh).contains(&"echo".to_string()));
    }

    #[test]
    fn enumerate_empty_actions_return_empty() {
        let sh = Shell::new();
        for a in [
            Action::Disabled,
            Action::Binding,
            Action::Hostname,
            Action::User,
            Action::Group,
            Action::Service,
        ] {
            assert!(
                complete_action(a, "", &sh).is_empty(),
                "expected empty for {a:?}"
            );
        }
    }

    fn ctx(cur: &str) -> CompletionCtx {
        CompletionCtx {
            cmd_name: "cmd".to_string(),
            cur_word: cur.to_string(),
            prev_word: String::new(),
            comp_words: vec!["cmd".to_string(), cur.to_string()],
            comp_cword: 1,
            comp_line: format!("cmd {cur}"),
            comp_point: 4 + cur.len(),
        }
    }

    #[test]
    fn wordlist_filters_by_prefix() {
        let spec = CompletionSpec {
            wordlist: Some("alpha alpine beta".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx("al"), &mut sh);
        assert_eq!(got, vec!["alpha", "alpine"]);
    }

    #[test]
    fn wordlist_with_no_match_is_empty() {
        let spec = CompletionSpec {
            wordlist: Some("alpha beta".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx("z"), &mut sh);
        assert!(got.is_empty());
    }

    #[test]
    fn wordlist_respects_ifs() {
        let spec = CompletionSpec {
            wordlist: Some("alpha:apple:banana".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        sh.set("IFS", ":".to_string());
        let got = run_spec(&spec, &ctx("a"), &mut sh);
        assert_eq!(got, vec!["alpha", "apple"]);
    }

    #[test]
    fn action_function_enumerates_functions() {
        let mut sh = Shell::new();
        // Use a minimal valid empty function body — Simple(Assign(vec![])).
        let body: Box<crate::command::Command> = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        sh.define_function("alpha".to_string(), body.clone());
        sh.define_function("alpine".to_string(), body.clone());
        sh.define_function("beta".to_string(), body);

        let spec = CompletionSpec {
            actions: vec![Action::Function],
            ..Default::default()
        };
        let got = run_spec(&spec, &ctx("al"), &mut sh);
        assert_eq!(got, vec!["alpha", "alpine"]);
    }

    #[test]
    fn action_builtin_enumerates_builtins() {
        let spec = CompletionSpec {
            actions: vec![Action::Builtin],
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx("ec"), &mut sh);
        assert!(got.contains(&"echo".to_string()), "{got:?}");
    }

    #[test]
    fn action_keyword_enumerates_keywords() {
        let spec = CompletionSpec {
            actions: vec![Action::Keyword],
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx("fo"), &mut sh);
        assert_eq!(got, vec!["for"]);
    }

    #[test]
    fn filter_removes_matches() {
        let spec = CompletionSpec {
            wordlist: Some("alpha apple banana cherry".to_string()),
            filter: Some("a*".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx(""), &mut sh);
        // "a*" removes alpha and apple; banana and cherry remain.
        assert_eq!(got, vec!["banana", "cherry"]);
    }

    #[test]
    fn filter_bang_keeps_only_matches() {
        let spec = CompletionSpec {
            wordlist: Some("alpha apple banana cherry".to_string()),
            filter: Some("!a*".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["alpha", "apple"]);
    }

    #[test]
    fn prefix_suffix_decorate_results() {
        let spec = CompletionSpec {
            wordlist: Some("a b".to_string()),
            prefix: Some("x:".to_string()),
            suffix: Some(":y".to_string()),
            ..Default::default()
        };
        let mut sh = Shell::new();
        let got = run_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["x:a:y", "x:b:y"]);
    }

    #[test]
    fn function_invocation_reads_compreply() {
        let mut sh = Shell::new();
        // Define a function whose body sets COMPREPLY=(alpha beta).
        // Building the AST by hand is painful; route through process_line.
        let outcome = crate::shell::process_line(
            "_myf() { COMPREPLY=(alpha beta); }",
            &mut sh,
            false,
        );
        assert!(matches!(outcome, crate::builtins::ExecOutcome::Continue(_)));
        assert!(sh.functions.contains_key("_myf"));

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let got = run_spec(&spec, &ctx(""), &mut sh);
        assert_eq!(got, vec!["alpha", "beta"]);
    }

    #[test]
    fn function_invocation_sets_comp_words() {
        let mut sh = Shell::new();
        // Function copies COMP_WORDS[1] into COMPREPLY[0].
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(\"${COMP_WORDS[1]}\"); }",
            &mut sh,
            false,
        );

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let mut c = ctx("");
        c.comp_words = vec!["cmd".to_string(), "expected_word".to_string()];
        c.comp_cword = 1;
        let got = run_spec(&spec, &c, &mut sh);
        assert_eq!(got, vec!["expected_word"]);
    }

    #[test]
    fn function_invocation_positional_params() {
        let mut sh = Shell::new();
        // Function copies $1, $2, $3 into COMPREPLY (three elements).
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(\"$1\" \"$2\" \"$3\"); }",
            &mut sh,
            false,
        );

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let c = CompletionCtx {
            cmd_name: "git".to_string(),
            cur_word: "che".to_string(),
            prev_word: "checkout".to_string(),
            comp_words: vec!["git".to_string(), "checkout".to_string(), "che".to_string()],
            comp_cword: 2,
            comp_line: "git checkout che".to_string(),
            comp_point: 16,
        };
        let got = run_spec(&spec, &c, &mut sh);
        assert_eq!(got, vec!["git", "che", "checkout"]);
    }

    #[test]
    fn function_invocation_preserves_last_status() {
        let mut sh = Shell::new();
        // Function exits with 17.
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(x); return 17; }",
            &mut sh,
            false,
        );
        // Set $? AFTER process_line (which itself zeroes $? on success).
        sh.set_last_status(42);

        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let _ = run_spec(&spec, &ctx(""), &mut sh);
        // Completion functions must NOT pollute $?.
        assert_eq!(sh.last_status(), 42);
    }

    #[test]
    fn function_invocation_leaves_current_spec_set() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line("_myf() { COMPREPLY=(a); }", &mut sh, false);
        let spec = CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let _ = run_spec(&spec, &ctx(""), &mut sh);
        assert!(
            sh.current_completion_spec.is_some(),
            "Task 5/6 depend on current_completion_spec being left set",
        );
    }

    #[test]
    fn function_missing_returns_empty() {
        let mut sh = Shell::new();
        let spec = CompletionSpec {
            function: Some("_does_not_exist".to_string()),
            ..Default::default()
        };
        let got = run_spec(&spec, &ctx(""), &mut sh);
        assert!(got.is_empty());
    }

    #[test]
    fn action_parse_recognizes_known() {
        assert_eq!(Action::parse("file"), Some(Action::File));
        assert_eq!(Action::parse("directory"), Some(Action::Directory));
        assert_eq!(Action::parse("keyword"), Some(Action::Keyword));
    }

    #[test]
    fn action_parse_rejects_unknown() {
        assert_eq!(Action::parse("bogus_action"), None);
        assert_eq!(Action::parse("nosuchaction"), None);
    }

    #[test]
    fn split_wordlist_default_ifs_collapses_runs() {
        let got = split_wordlist("  a   b  c ", " \t\n");
        assert_eq!(got, vec!["a", "b", "c"]);
    }

    #[test]
    fn split_wordlist_non_ws_each_delimits() {
        // With IFS=":" (single non-ws byte), each ":" ends a field.
        // "a::b" → ["a", "", "b"].
        let got = split_wordlist("a::b", ":");
        assert_eq!(got, vec!["a", "", "b"]);
    }

    #[test]
    fn action_command_returns_raw_names_not_escaped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // Create an executable with a metachar-free name; the point is
        // to verify the `display` (raw) form is returned, not the
        // backslash-escaped form. Confirm by checking the name has no
        // backslash.
        let exe = dir.path().join("myrawcmd");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut sh = Shell::new();
        sh.set("PATH", dir.path().to_str().unwrap().to_string());
        let spec = CompletionSpec {
            actions: vec![Action::Command],
            ..Default::default()
        };
        let got = run_spec(&spec, &ctx("myraw"), &mut sh);
        assert!(got.iter().any(|s| s == "myrawcmd"), "{got:?}");
        // No backslash anywhere in the results.
        assert!(got.iter().all(|s| !s.contains('\\')), "{got:?}");
    }

    #[test]
    fn action_file_handles_dir_prefix() {
        let _g = CWD_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        std::fs::write(subdir.join("alpha.txt"), b"").unwrap();
        std::fs::write(subdir.join("beta.txt"), b"").unwrap();

        // CD into the tempdir so "." resolution and relative prefixes work.
        let prior = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let mut sh = Shell::new();
        let spec = CompletionSpec {
            actions: vec![Action::File],
            ..Default::default()
        };
        let got = run_spec(&spec, &ctx("subdir/al"), &mut sh);
        std::env::set_current_dir(prior).unwrap();

        // Result must round-trip with the "subdir/" prefix included.
        assert!(got.iter().any(|s| s == "subdir/alpha.txt"), "{got:?}");
        // And NOT match unrelated files (no bare "alpha.txt" without prefix).
        assert!(got.iter().all(|s| s.starts_with("subdir/")), "{got:?}");
    }

    #[test]
    fn directory_action_tilde_expands_home() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("projects")).unwrap();
        std::fs::create_dir(home.path().join("pub")).unwrap();
        let mut sh = Shell::new();
        sh.set("HOME", home.path().to_str().unwrap().to_string());

        let res = complete_action(Action::Directory, "~/", &sh);
        assert!(res.contains(&"~/projects".to_string()), "{res:?}");
        assert!(res.contains(&"~/pub".to_string()), "{res:?}");

        let res2 = complete_action(Action::Directory, "~/pro", &sh);
        assert_eq!(res2, vec!["~/projects".to_string()], "{res2:?}");
    }
}
