use crate::command::Sequence;
use crate::executor;
use crate::lexer::{TildeSpec, Word, WordPart};
use crate::shell_state::Shell;
use glob::{glob_with, MatchOptions};

fn resolve_tilde(spec: &TildeSpec, shell: &Shell) -> Option<String> {
    match spec {
        TildeSpec::Home   => shell.get("HOME").map(str::to_string),
        TildeSpec::Pwd    => shell.get("PWD").map(str::to_string),
        TildeSpec::OldPwd => shell.get("OLDPWD").map(str::to_string),
        TildeSpec::User(name) => lookup_home_for_user(name),
    }
}

fn render_tilde_literal(spec: &TildeSpec) -> String {
    match spec {
        TildeSpec::Home       => "~".to_string(),
        TildeSpec::Pwd        => "~+".to_string(),
        TildeSpec::OldPwd     => "~-".to_string(),
        TildeSpec::User(name) => format!("~{name}"),
    }
}

fn lookup_home_for_user(name: &str) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::mem::MaybeUninit;
    use std::ptr;

    let cname = CString::new(name).ok()?;
    let mut buf: Vec<u8> = vec![0; 1024];
    loop {
        let mut pwd: MaybeUninit<libc::passwd> = MaybeUninit::uninit();
        let mut result: *mut libc::passwd = ptr::null_mut();
        let rc = unsafe {
            libc::getpwnam_r(
                cname.as_ptr(),
                pwd.as_mut_ptr(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 && !result.is_null() {
            let pwd = unsafe { pwd.assume_init() };
            if pwd.pw_dir.is_null() {
                return None;
            }
            let home = unsafe { CStr::from_ptr(pwd.pw_dir) };
            return home.to_str().ok().map(str::to_string);
        }
        if rc == libc::ERANGE && buf.len() < 16384 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub chars: String,
    pub quoted: Vec<bool>,
}

impl Field {
    pub fn new() -> Self {
        Self { chars: String::new(), quoted: Vec::new() }
    }

    pub fn push_str(&mut self, s: &str, quoted: bool) {
        let count = s.chars().count();
        self.chars.push_str(s);
        self.quoted.extend(std::iter::repeat_n(quoted, count));
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}

/// Expands a `Word` against the current `Shell` state into 0 or more
/// `Field`s. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
///
/// Per-WordPart quoting propagation (v10 Task 5): each char appended to a
/// `Field` carries the `quoted` flag of its source `WordPart`. Tilde
/// expansions and IFS-split fragments are always marked unquoted. This
/// preserves the information that pathname expansion (glob) needs to skip
/// quoted metacharacters.
pub fn expand(word: &Word, shell: &mut Shell) -> Vec<Field> {
    // Snapshot $? at the start so every `LastStatus` part in this word sees
    // the same value — even if a `CommandSub` part earlier in the word
    // updates the live $?. This matches bash: substitutions update $? for
    // the next command, not for `$?` references in the same expansion.
    let snapshot_status = shell.last_status();
    let mut current = Field::new();
    let mut has_emitted = false;
    let mut result: Vec<Field> = Vec::new();

    for part in &word.0 {
        match part {
            WordPart::Literal { text, quoted } => {
                current.push_str(text, *quoted);
                has_emitted = true;
            }
            WordPart::Tilde(spec) => {
                // Tilde expansion result is always unquoted — pathname
                // expansion treats the expanded path as if the user typed it.
                let text = resolve_tilde(spec, shell)
                    .unwrap_or_else(|| render_tilde_literal(spec));
                current.push_str(&text, false);
                has_emitted = true;
            }
            WordPart::Var { name, quoted: true } => {
                if let Some(value) = shell.lookup_var(name) {
                    current.push_str(&value, true);
                }
                // Unset quoted var: relies on `has_emitted` so end-of-word
                // still produces a (possibly empty) Field.
                has_emitted = true;
            }
            WordPart::LastStatus { quoted: true } => {
                current.push_str(&snapshot_status.to_string(), true);
                has_emitted = true;
            }
            WordPart::Var { name, quoted: false } => {
                let value = shell.lookup_var(name).unwrap_or_default();
                emit_split_fields(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::AllArgs { quoted: false, joined: _ } => {
                // Unquoted $@ and $* are identical: each arg becomes its
                // own field(s), IFS-split. Args are independent — the
                // last IFS-fragment of arg N must NOT merge with the
                // first of arg N+1, so we flush current between args.
                let args = shell.positional_args.clone();
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 && !current.is_empty() {
                        result.push(std::mem::take(&mut current));
                    }
                    emit_split_fields(arg, &mut current, &mut result, &mut has_emitted);
                }
            }
            WordPart::AllArgs { quoted: true, joined: false } => {
                // "$@" — each arg its own quoted field, no splitting.
                // First arg merges into current; subsequent start new
                // fields; last becomes the new current.
                let args = shell.positional_args.clone();
                if !args.is_empty() {
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            // Start a new field for the next arg.
                            result.push(std::mem::take(&mut current));
                        }
                        current.push_str(arg, true);
                        has_emitted = true;
                    }
                }
                // Empty args: zero fields — do nothing.
            }
            WordPart::AllArgs { quoted: true, joined: true } => {
                // "$*" — single field, args joined by " " (first IFS char).
                let joined = shell.positional_args.join(" ");
                current.push_str(&joined, true);
                has_emitted = true;
            }
            WordPart::LastStatus { quoted: false } => {
                let value = snapshot_status.to_string();
                emit_split_fields(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::CommandSub { sequence, quoted: true } => {
                let output = run_substitution(sequence, shell);
                current.push_str(&output, true);
                has_emitted = true;
            }
            WordPart::CommandSub { sequence, quoted: false } => {
                let output = run_substitution(sequence, shell);
                emit_split_fields(&output, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::Arith { expr, quoted: _ } => {
                match crate::arith::eval(expr, shell) {
                    Ok(n) => {
                        current.push_str(&n.to_string(), true);
                        has_emitted = true;
                    }
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.set_last_status(1);
                        has_emitted = true;
                        // Append nothing; the field stays empty if no other parts.
                    }
                }
            }
            WordPart::ParamExpansion { name, modifier, quoted } => {
                match crate::param_expansion::expand_modifier(name, modifier, shell) {
                    crate::param_expansion::ExpansionResult::Value(v) => {
                        if *quoted {
                            current.push_str(&v, true);
                            has_emitted = true;
                        } else {
                            emit_split_fields(&v, &mut current, &mut result, &mut has_emitted);
                        }
                    }
                    crate::param_expansion::ExpansionResult::Empty => {
                        has_emitted = true;
                    }
                }
            }
        }
    }

    // End-of-word: push the in-progress field if it's non-empty, OR if
    // `has_emitted` is true (preserves the "this word produced something —
    // possibly an empty arg from `""` or a `"$UNSET"`" semantic).
    if !current.is_empty() || has_emitted {
        result.push(current);
    }
    result
}

/// Expands a `Word` for assignment context: word-splitting is suppressed and
/// the result is one string. Each `Var`/`LastStatus`/`CommandSub` part
/// contributes its value verbatim regardless of the `quoted` flag — matching
/// bash, which disables splitting on the right-hand side of `NAME=...`.
pub fn expand_assignment(word: &Word, shell: &mut Shell) -> String {
    // Snapshot $? so `LastStatus` parts read the value at the start of
    // expansion, not whatever a preceding `$(cmd)` mutated it to. Same
    // contract as `expand()` and `expand_pattern()`.
    let snapshot_status = shell.last_status();
    let mut result = String::new();
    for part in &word.0 {
        match part {
            WordPart::Literal { text, .. } => result.push_str(text),
            WordPart::Tilde(spec) => {
                let text = resolve_tilde(spec, shell)
                    .unwrap_or_else(|| render_tilde_literal(spec));
                result.push_str(&text);
            }
            WordPart::Var { name, .. } => {
                if let Some(value) = shell.lookup_var(name) {
                    result.push_str(&value);
                }
            }
            WordPart::LastStatus { .. } => {
                result.push_str(&snapshot_status.to_string());
            }
            WordPart::CommandSub { sequence, .. } => {
                result.push_str(&run_substitution(sequence, shell));
            }
            WordPart::Arith { expr, quoted: _ } => {
                match crate::arith::eval(expr, shell) {
                    Ok(n) => result.push_str(&n.to_string()),
                    Err(e) => {
                        eprintln!("huck: arithmetic: {}", e);
                        shell.set_last_status(1);
                        // Append nothing.
                    }
                }
            }
            WordPart::ParamExpansion { name, modifier, .. } => {
                match crate::param_expansion::expand_modifier(name, modifier, shell) {
                    crate::param_expansion::ExpansionResult::Value(v) => result.push_str(&v),
                    crate::param_expansion::ExpansionResult::Empty => {}
                }
            }
            WordPart::AllArgs { .. } => {
                // No field splitting in assignment context; join with space.
                let joined = shell.positional_args.join(" ");
                result.push_str(&joined);
            }
        }
    }
    result
}

/// True when `part` carried a `quoted` flag set to true. Tilde parts
/// have no quoted flag and count as unquoted.
fn word_part_is_quoted(part: &WordPart) -> bool {
    match part {
        WordPart::Literal { quoted, .. } => *quoted,
        WordPart::Var { quoted, .. } => *quoted,
        WordPart::LastStatus { quoted } => *quoted,
        WordPart::CommandSub { quoted, .. } => *quoted,
        WordPart::Arith { quoted, .. } => *quoted,
        WordPart::ParamExpansion { quoted, .. } => *quoted,
        WordPart::AllArgs { quoted, .. } => *quoted,
        WordPart::Tilde(_) => false,
    }
}

/// Expands `word` into a glob-pattern string for `case` matching.
/// Like `expand_assignment` (no field splitting), but text contributed
/// by a quoted part is escaped via `glob::Pattern::escape`, so a quoted
/// `*`/`?`/`[` matches literally while an unquoted one is a wildcard.
pub fn expand_pattern(word: &Word, shell: &mut Shell) -> String {
    // Snapshot `$?` so `LastStatus` parts read the value at the start of
    // the expansion, not whatever a preceding `$(cmd)` mutated it to.
    // Matches the contract in `expand()` (used for command arguments).
    let snapshot_status = shell.last_status();
    let mut result = String::new();
    for part in &word.0 {
        let text = if matches!(part, WordPart::LastStatus { .. }) {
            snapshot_status.to_string()
        } else {
            expand_assignment(&Word(vec![part.clone()]), shell)
        };
        if word_part_is_quoted(part) {
            result.push_str(&glob::Pattern::escape(&text));
        } else {
            result.push_str(&text);
        }
    }
    result
}

/// Runs a sub-sequence as a substituted command: clones the parent `Shell`
/// (so state mutations don't leak), captures stdout via the executor's
/// `execute_capturing`, strips trailing newlines, and propagates the
/// substituted command's exit status into the parent shell's `$?`.
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    strip_trailing_newlines(&output)
}

fn strip_trailing_newlines(s: &str) -> String {
    s.trim_end_matches('\n').to_string()
}

fn emit_split_fields(
    value: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    let fragments: Vec<&str> = value.split_ascii_whitespace().collect();
    if fragments.is_empty() {
        return;
    }
    // First fragment continues the in-progress field.
    current.push_str(fragments[0], false);
    *has_emitted = true;
    // Each subsequent fragment closes the field and starts a new one.
    for frag in &fragments[1..] {
        let finished = std::mem::take(current);
        result.push(finished);
        current.push_str(frag, false);
    }
}

/// Expands fields by pathname expansion (globbing). For fields with no
/// unquoted glob metacharacters, returns the field as-is. For fields with
/// unquoted metacharacters, builds a glob pattern (escaping quoted metachars
/// via bracket expressions) and invokes the `glob` crate. If no matches,
/// returns the field as-is (literal fallback — bash default behavior).
pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String> {
    let mut out = Vec::new();
    for field in fields {
        if !has_unquoted_metachar(&field) {
            out.push(field.chars);
            continue;
        }
        let pattern = build_glob_pattern(&field);
        // Bash semantics: a leading literal `.` in the pattern matches a
        // leading `.` in filenames; otherwise `*` and `?` never match one.
        // The `glob` crate's `require_literal_leading_dot=true` enforces the
        // "never" rule but also blocks an explicit dot-prefix pattern (`.*`,
        // `.foo`, or a bracket class like `[.]*`) from matching dotfiles, so
        // we toggle it off when the pattern's effective first char is a
        // literal `.`. We accept both bare `.` and the `[.]` single-element
        // bracket form (verified empirically against `glob` 0.3).
        let literal_leading_dot =
            pattern.starts_with('.') || pattern.starts_with("[.]");
        let opts = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: !literal_leading_dot,
        };
        match glob_with(&pattern, opts) {
            Ok(paths) => {
                let mut matched = Vec::new();
                for entry in paths {
                    let Ok(path) = entry else { continue };
                    match path.into_os_string().into_string() {
                        Ok(s) => matched.push(s),
                        Err(_) => eprintln!("huck: skipping non-UTF8 path"),
                    }
                }
                // Defensive: filter `.` and `..` if the glob crate ever emits
                // them for patterns like `.*`. (Current versions exclude them
                // under require_literal_leading_dot, but explicit filtering
                // makes the contract loud.)
                matched.retain(|p| {
                    let last = std::path::Path::new(p).file_name().and_then(|s| s.to_str());
                    !matches!(last, Some(".") | Some(".."))
                });
                if matched.is_empty() {
                    out.push(field.chars);
                } else {
                    out.extend(matched);
                }
            }
            Err(_) => {
                // Invalid pattern → literal fallback.
                out.push(field.chars);
            }
        }
    }
    out
}

/// Builds the glob pattern string for a `Field`: quoted metacharacters
/// (`*`, `?`, `[`, `]`) are escaped via one-char bracket expressions
/// (`[*]`, `[?]`, `[[]`, `[]]`), so the `glob` crate treats them as literal.
/// Unquoted chars pass through verbatim.
fn build_glob_pattern(field: &Field) -> String {
    let mut p = String::new();
    for (c, &q) in field.chars.chars().zip(field.quoted.iter()) {
        if q && matches!(c, '*' | '?' | '[' | ']') {
            p.push('[');
            p.push(c);
            p.push(']');
        } else {
            p.push(c);
        }
    }
    p
}

/// Checks whether a field contains any unquoted glob metacharacters: `*`, `?`, `[`.
fn has_unquoted_metachar(field: &Field) -> bool {
    field
        .chars
        .chars()
        .zip(field.quoted.iter())
        .any(|(c, &q)| !q && matches!(c, '*' | '?' | '['))
}

#[cfg(test)]
impl Field {
    pub fn from_unquoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![false; count] }
    }

    pub fn from_quoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![true; count] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, ExecCommand, Pipeline, SimpleCommand};

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    /// Test helper: project `Vec<Field>` back to `Vec<String>` so the existing
    /// assertions don't have to construct `Field` literals. (Task 4 only
    /// changes the signature; quoting propagation lands in Task 5.)
    fn expand_strings(word: &Word, shell: &mut Shell) -> Vec<String> {
        expand(word, shell).into_iter().map(|f| f.chars).collect()
    }

    fn var_unq(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: false }])
    }
    fn var_q(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: true }])
    }

    /// Builds a synthetic Sequence for `echo <args>` — used to drive
    /// CommandSub expansion in unit tests without invoking the lexer.
    fn echo_sequence(args: &[&str]) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn expand_literal_word() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&lit("hello"), &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&lit(""), &mut shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["foobar".to_string()]);
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let mut shell = Shell::new();
        assert!(expand_strings(&var_unq("DEFINITELY_NOT_SET_XYZ"), &mut shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(
            expand_strings(&var_q("DEFINITELY_NOT_SET_XYZ"), &mut shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(expand_strings(&var_q("HUCK_T"), &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(
            expand_strings(&var_unq("HUCK_T"), &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_unquoted_var_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "x y".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "a".to_string(), quoted: false },
            WordPart::Var { name: "HUCK_T".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["42".to_string()]);
    }

    #[test]
    fn expand_tilde_uses_home() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/tmp/huck_test".to_string());
        let word = Word(vec![
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal { text: "/foo".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["/tmp/huck_test/foo".to_string()]
        );
    }

    #[test]
    fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
        let mut shell = Shell::new();
        assert_eq!(expand_strings(&Word(vec![WordPart::Var {
            name: "DEFINITELY_NOT_SET_REDIR".to_string(),
            quoted: false,
        }]), &mut shell).len(), 0);
    }

    #[test]
    fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_TWOFIELD", "a b".to_string());
        assert_eq!(expand_strings(&Word(vec![WordPart::Var {
            name: "HUCK_T_TWOFIELD".to_string(),
            quoted: false,
        }]), &mut shell).len(), 2);
    }

    #[test]
    fn expand_assignment_preserves_interior_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_PAD", "a  b".to_string());
        let word = Word(vec![WordPart::Var {
            name: "HUCK_T_PAD".to_string(),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a  b".to_string());
    }

    #[test]
    fn expand_assignment_concatenates_parts() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_X", "x".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "pre-".to_string(), quoted: false },
            WordPart::Var { name: "HUCK_T_X".to_string(), quoted: false },
            WordPart::Literal { text: "-post".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "pre-x-post".to_string());
    }

    #[test]
    fn expand_assignment_unset_var_yields_empty_segment() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "[".to_string(), quoted: false },
            WordPart::Var { name: "DEFINITELY_NOT_SET_ASN".to_string(), quoted: false },
            WordPart::Literal { text: "]".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "[]".to_string());
    }

    // ---- CommandSub tests --------------------------------------------------

    #[test]
    fn expand_command_sub_invokes_inner_echo() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hello"]),
            quoted: false,
        }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_command_sub_unquoted_splits() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: true,
        }]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_command_sub_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "pre".to_string(), quoted: false },
            WordPart::CommandSub {
                sequence: echo_sequence(&["x", "y"]),
                quoted: false,
            },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["prex".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_command_sub_strips_trailing_newlines() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["hi"]),
            quoted: true,
        }]);
        // echo emits "hi\n"; run_substitution strips -> "hi" exactly.
        assert_eq!(expand_strings(&word, &mut shell), vec!["hi".to_string()]);
    }

    #[test]
    fn expand_command_sub_updates_parent_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(0);
        let word = Word(vec![WordPart::CommandSub {
            sequence: exit_sequence(7),
            quoted: true,
        }]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_assignment_last_status_after_command_sub_reads_snapshot() {
        // Parallel to expand_last_status_after_command_sub_in_same_word_reads_snapshot
        // but for assignment context. `NAME=$(exit 7)$?` with $?=3 before should
        // store "3", not "7" — `$?` reads the pre-assignment snapshot.
        let mut shell = Shell::new();
        shell.set_last_status(3);
        let word = Word(vec![
            WordPart::CommandSub {
                sequence: exit_sequence(7),
                quoted: false,
            },
            WordPart::LastStatus { quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "3".to_string());
        // The substitution still updates $? for the next command.
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_assignment_command_sub_concatenates_verbatim() {
        // expand_assignment suppresses splitting, so `FOO=$(echo a b)` stores
        // "a b" (one space) as the value — same as bash's IFS=behavior in
        // assignment context. (echo's argument joining already produces "a b"
        // with one space.)
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(expand_assignment(&word, &mut shell), "a b".to_string());
    }

    #[test]
    fn expand_last_status_after_command_sub_in_same_word_reads_snapshot() {
        // Bash semantics: within a single word, `$?` reads the value of $?
        // at the start of expansion, NOT the status set by an earlier
        // CommandSub in the same word. e.g. `"$(exit 7)$?"` with $?=3 before
        // expands to "73" (the substitution's "" output then "3"), not "77".
        let mut shell = Shell::new();
        shell.set_last_status(3);
        let word = Word(vec![
            WordPart::CommandSub {
                sequence: exit_sequence(7),
                quoted: true,
            },
            WordPart::LastStatus { quoted: true },
        ]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["3".to_string()]);
        // The substitution still updates $? for the NEXT word/command.
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_tilde_home_unset_falls_back_to_literal() {
        let mut shell = Shell::new();
        shell.unset("HOME");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~"]);
    }

    #[test]
    fn expand_tilde_pwd_resolves_when_pwd_set() {
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var/tmp".to_string());
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["/var/tmp"]);
    }

    #[test]
    fn expand_tilde_pwd_unset_falls_back_to_literal_plus() {
        let mut shell = Shell::new();
        shell.unset("PWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~+"]);
    }

    #[test]
    fn expand_tilde_oldpwd_unset_falls_back_to_literal_minus() {
        let mut shell = Shell::new();
        shell.unset("OLDPWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]);
        assert_eq!(expand_strings(&word, &mut shell), vec!["~-"]);
    }

    #[test]
    fn expand_tilde_unknown_user_falls_back_to_literal() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Tilde(TildeSpec::User("definitely_not_a_real_user_xyz_42".to_string())),
            WordPart::Literal { text: "/x".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand_strings(&word, &mut shell),
            vec!["~definitely_not_a_real_user_xyz_42/x"]
        );
    }

    #[test]
    fn expand_assignment_tilde_home_resolves() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/h".to_string());
        let word = Word(vec![
            WordPart::Literal { text: "PATH=".to_string(), quoted: false },
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal { text: "/bin".to_string(), quoted: false },
        ]);
        assert_eq!(expand_assignment(&word, &mut shell), "PATH=/h/bin");
    }

    #[test]
    fn field_from_unquoted_str_marks_all_chars_unquoted() {
        let f = Field::from_unquoted("abc");
        assert_eq!(f.chars, "abc");
        assert_eq!(f.quoted, vec![false, false, false]);
    }

    #[test]
    fn field_from_quoted_str_marks_all_chars_quoted() {
        let f = Field::from_quoted("xy");
        assert_eq!(f.chars, "xy");
        assert_eq!(f.quoted, vec![true, true]);
    }

    #[test]
    fn field_push_str_appends_chars_with_quoted_flag() {
        let mut f = Field::from_unquoted("a");
        f.push_str("bc", true);
        assert_eq!(f.chars, "abc");
        assert_eq!(f.quoted, vec![false, true, true]);
    }

    #[test]
    fn field_quoted_vec_uses_char_count_not_byte_count() {
        // Multi-byte char: should produce 1 quoted entry, not the UTF-8 byte count.
        let f = Field::from_unquoted("é");
        assert_eq!(f.chars.chars().count(), 1);
        assert_eq!(f.quoted.len(), 1);
    }

    // ---- Quoting propagation (v10 Task 5) ----------------------------------

    #[test]
    fn expand_literal_unquoted_marks_chars_unquoted() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: false }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![false, false, false]);
    }

    #[test]
    fn expand_literal_quoted_marks_chars_quoted() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: true }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![true, true, true]);
    }

    #[test]
    fn expand_mixed_quoted_unquoted_literal_parts() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "*".to_string(), quoted: true },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "foo*bar");
        assert_eq!(fields[0].quoted, vec![false, false, false, true, false, false, false]);
    }

    #[test]
    fn expand_quoted_var_marks_chars_quoted() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_Q", "val".to_string());
        let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: true }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![true, true, true]);
    }

    #[test]
    fn expand_unquoted_var_marks_chars_unquoted() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_Q", "val".to_string());
        let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: false }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].quoted, vec![false, false, false]);
    }

    #[test]
    fn expand_tilde_marks_chars_unquoted() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/h".to_string());
        let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields[0].chars, "/h");
        assert_eq!(fields[0].quoted, vec![false, false]);
    }

    // ---- glob_expand_fields tests (v10 Task 6) ----------------------------------

    #[test]
    fn glob_expand_no_metachar_returns_chars_as_string() {
        let f = Field::from_unquoted("plain.txt");
        let out = glob_expand_fields(vec![f]);
        assert_eq!(out, vec!["plain.txt".to_string()]);
    }

    #[test]
    fn glob_expand_quoted_metachar_treated_as_literal() {
        // All chars quoted including the `*` → no globbing.
        let f = Field::from_quoted("*.txt");
        let out = glob_expand_fields(vec![f]);
        assert_eq!(out, vec!["*.txt".to_string()]);
    }

    #[test]
    fn glob_expand_question_mark_metachar_detected() {
        // CWD is process-global; run inside an empty temp dir under the lock
        // so concurrent tests can't contaminate the glob result.
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("a");
        f.push_str("?", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        // No matches in empty temp dir → literal fallback.
        assert_eq!(out, vec!["a?".to_string()]);
    }

    #[test]
    fn glob_expand_preserves_field_order() {
        let f1 = Field::from_unquoted("first");
        let f2 = Field::from_unquoted("second");
        let out = glob_expand_fields(vec![f1, f2]);
        assert_eq!(out, vec!["first".to_string(), "second".to_string()]);
    }

    // ---- glob_expand_fields filesystem tests (v10 Task 7) ----------------------

    use std::sync::Mutex;

    // CWD is process-global; serialize tests that mutate it.
    static CWD_LOCK: Mutex<()> = Mutex::new(());

    fn touch(dir: &std::path::Path, name: &str) {
        std::fs::write(dir.join(name), b"").unwrap();
    }

    #[test]
    fn glob_star_matches_files_in_cwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("*");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn glob_star_excludes_dotfiles_by_default() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "visible");
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let f = Field::from_unquoted("*");
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["visible".to_string()]);
    }

    #[test]
    fn glob_dot_star_matches_dotfiles_but_excludes_dot_and_dotdot() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted(".");
        f.push_str("*", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert!(out.contains(&".hidden".to_string()));
        assert!(!out.contains(&".".to_string()));
        assert!(!out.contains(&"..".to_string()));
    }

    #[test]
    fn glob_bracket_dot_class_matches_dotfile() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), ".hidden");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[.]");
        f.push_str("hidden", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec![".hidden".to_string()]);
    }

    #[test]
    fn glob_bracket_class_matches_listed_chars() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        touch(tmp.path(), "c.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[ab]");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
    }

    #[test]
    fn glob_no_match_returns_literal_pattern() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("nonex");
        f.push_str("*", false);
        f.push_str(".xyz", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["nonex*.xyz".to_string()]);
    }

    #[test]
    fn glob_partial_quoting_keeps_literal_prefix() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "fooA");
        touch(tmp.path(), "fooB");
        touch(tmp.path(), "barA");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        // `"foo"*` — first three chars quoted, then unquoted `*`.
        let mut f = Field::from_quoted("foo");
        f.push_str("*", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["fooA".to_string(), "fooB".to_string()]);
    }

    #[test]
    fn glob_negation_bracket_excludes_listed() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        touch(tmp.path(), "a.txt");
        touch(tmp.path(), "b.txt");
        touch(tmp.path(), "c.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("[!a]");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["b.txt".to_string(), "c.txt".to_string()]);
    }

    #[test]
    fn glob_unterminated_bracket_falls_back_to_literal() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let f = Field::from_unquoted("[abc"); // no closing ]
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["[abc".to_string()]);
    }

    #[test]
    fn expand_then_glob_end_to_end_for_literal() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }]);
        let argv = glob_expand_fields(expand(&word, &mut shell));
        assert_eq!(argv, vec!["hello".to_string()]);
    }

    #[test]
    fn expand_arith_part_renders_decimal_result() {
        use crate::arith::ArithExpr;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Arith {
            expr: ArithExpr::Add(
                Box::new(ArithExpr::Num(2)),
                Box::new(ArithExpr::Num(3)),
            ),
            quoted: false,
        }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "5");
        assert_eq!(fields[0].quoted, vec![true]);
    }

    #[test]
    fn expand_arith_part_division_by_zero_yields_empty_field_and_sets_status() {
        use crate::arith::ArithExpr;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Arith {
            expr: ArithExpr::Div(
                Box::new(ArithExpr::Num(1)),
                Box::new(ArithExpr::Num(0)),
            ),
            quoted: false,
        }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "");
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn expand_assignment_arith_part_renders_decimal() {
        use crate::arith::ArithExpr;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::Arith {
            expr: ArithExpr::Mul(
                Box::new(ArithExpr::Num(6)),
                Box::new(ArithExpr::Num(7)),
            ),
            quoted: false,
        }]);
        let value = expand_assignment(&word, &mut shell);
        assert_eq!(value, "42");
    }

    #[test]
    fn expand_param_expansion_use_default_unquoted_unset() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E1".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "fallback".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["fallback".to_string()]);
    }

    #[test]
    fn expand_param_expansion_quoted_value_with_space_stays_one_field() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E2".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: true,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["a b c".to_string()]);
    }

    #[test]
    fn expand_param_expansion_unquoted_value_with_space_splits() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_E3", "a b c".to_string());
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E3".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![]),
                colon: true,
            },
            quoted: false,
        }]);
        let fields = expand(&word, &mut shell);
        let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
        assert_eq!(strings, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn expand_assignment_param_expansion_no_split() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E4".to_string(),
            modifier: ParamModifier::UseDefault {
                word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
        }]);
        let value = expand_assignment(&word, &mut shell);
        assert_eq!(value, "a b c");
    }

    #[test]
    fn expand_param_expansion_error_yields_empty_field_sets_status() {
        use crate::lexer::ParamModifier;
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::ParamExpansion {
            name: "HUCK_TEST_PE_E5".to_string(),
            modifier: ParamModifier::ErrorIfUnset {
                word: Word(vec![WordPart::Literal { text: "missing".to_string(), quoted: false }]),
                colon: true,
            },
            quoted: false,
        }]);
        let fields = expand(&word, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "");
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn expand_pattern_last_status_snapshots_before_command_sub() {
        use crate::command::Sequence;

        let mut shell = Shell::new();
        shell.set_last_status(7);

        // A pattern word of two parts: a CommandSub that runs `false` (which
        // mutates $? to 1), followed by $?. With the snapshot fix, $? reads
        // the pre-expansion value (7) — not the post-`false` value (1).
        let false_cmd = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("false"),
                    args: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            }),
            rest: vec![],
            background: false,
        };
        let word = Word(vec![
            WordPart::CommandSub { sequence: false_cmd, quoted: false },
            WordPart::LastStatus { quoted: false },
        ]);

        let pattern = expand_pattern(&word, &mut shell);
        assert!(
            pattern.ends_with("7"),
            "expected pattern to end with the pre-expansion $? value 7, got: {pattern:?}"
        );
    }

    #[test]
    fn glob_star_does_not_cross_path_separator() {
        let _g = CWD_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        touch(&tmp.path().join("sub"), "deep.txt");
        touch(tmp.path(), "top.txt");
        let saved = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let mut f = Field::from_unquoted("*");
        f.push_str(".txt", false);
        let out = glob_expand_fields(vec![f]);

        std::env::set_current_dir(saved).unwrap();

        assert_eq!(out, vec!["top.txt".to_string()]);
    }

    // ---- Positional parameter expander tests (v22 Task 4) -------------------

    #[test]
    fn expand_dollar_digit_reads_positional() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["alpha".to_string(), "beta".to_string()];
        let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "alpha");
    }

    #[test]
    fn expand_dollar_digit_unset_is_empty() {
        let mut shell = Shell::new();
        let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        // Unset positional → no field (consistent with unset var behaviour)
        assert!(fields.is_empty());
    }

    #[test]
    fn expand_dollar_hash_is_arg_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let w = Word(vec![WordPart::Var { name: "#".to_string(), quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "3");
    }

    #[test]
    fn expand_dollar_at_quoted_produces_field_per_arg() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a a".to_string(), "b".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
        let fields = expand(&w, &mut shell);
        // Each arg its own field; the space inside "a a" is preserved (no splitting).
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].chars, "a a");
        assert_eq!(fields[1].chars, "b");
    }

    #[test]
    fn expand_dollar_star_quoted_joins_with_space() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: true, quoted: true }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].chars, "a b c");
    }

    #[test]
    fn expand_dollar_at_empty_produces_no_fields() {
        let mut shell = Shell::new();
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
        let fields = expand(&w, &mut shell);
        // Either zero fields or all-empty fields are acceptable per the spec.
        assert!(fields.is_empty());
    }

    #[test]
    fn expand_dollar_at_unquoted_splits_each_arg_independently() {
        // $@ unquoted with two args, one containing whitespace.
        // POSIX: each arg becomes its own field(s) after IFS-splitting;
        // args do NOT merge across boundaries.
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello world".to_string(), "x".to_string()];
        let w = Word(vec![WordPart::AllArgs { joined: false, quoted: false }]);
        let fields = expand(&w, &mut shell);
        assert_eq!(fields.len(), 3, "fields: {fields:?}");
        assert_eq!(fields[0].chars, "hello");
        assert_eq!(fields[1].chars, "world");
        assert_eq!(fields[2].chars, "x");
    }
}
