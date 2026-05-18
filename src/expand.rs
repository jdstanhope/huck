use crate::command::Sequence;
use crate::executor;
use crate::lexer::{TildeSpec, Word, WordPart};
use crate::shell_state::Shell;

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

/// Expands a `Word` against the current `Shell` state into 0 or more
/// argument strings. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
pub fn expand(word: &Word, shell: &mut Shell) -> Vec<String> {
    // Snapshot $? at the start so every `LastStatus` part in this word sees
    // the same value — even if a `CommandSub` part earlier in the word
    // updates the live $?. This matches bash: substitutions update $? for
    // the next command, not for `$?` references in the same expansion.
    let snapshot_status = shell.last_status();
    let mut current = String::new();
    let mut has_emitted = false;
    let mut result: Vec<String> = Vec::new();

    for part in &word.0 {
        match part {
            WordPart::Literal { text, .. } => {
                current.push_str(text);
                has_emitted = true;
            }
            WordPart::Tilde(spec) => {
                let text = resolve_tilde(spec, shell)
                    .unwrap_or_else(|| render_tilde_literal(spec));
                current.push_str(&text);
                has_emitted = true;
            }
            WordPart::Var { name, quoted: true } => {
                if let Some(value) = shell.get(name) {
                    current.push_str(value);
                }
                has_emitted = true;
            }
            WordPart::LastStatus { quoted: true } => {
                current.push_str(&snapshot_status.to_string());
                has_emitted = true;
            }
            WordPart::Var { name, quoted: false } => {
                let value = shell.get(name).map(|s| s.to_string()).unwrap_or_default();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::LastStatus { quoted: false } => {
                let value = snapshot_status.to_string();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::CommandSub { sequence, quoted: true } => {
                let output = run_substitution(sequence, shell);
                current.push_str(&output);
                has_emitted = true;
            }
            WordPart::CommandSub { sequence, quoted: false } => {
                let output = run_substitution(sequence, shell);
                emit_split(&output, &mut current, &mut result, &mut has_emitted);
            }
        }
    }

    if has_emitted {
        result.push(current);
    }
    result
}

/// Expands a `Word` for assignment context: word-splitting is suppressed and
/// the result is one string. Each `Var`/`LastStatus`/`CommandSub` part
/// contributes its value verbatim regardless of the `quoted` flag — matching
/// bash, which disables splitting on the right-hand side of `NAME=...`.
pub fn expand_assignment(word: &Word, shell: &mut Shell) -> String {
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
                if let Some(value) = shell.get(name) {
                    result.push_str(value);
                }
            }
            WordPart::LastStatus { .. } => {
                result.push_str(&shell.last_status().to_string());
            }
            WordPart::CommandSub { sequence, .. } => {
                result.push_str(&run_substitution(sequence, shell));
            }
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

fn emit_split(
    value: &str,
    current: &mut String,
    result: &mut Vec<String>,
    has_emitted: &mut bool,
) {
    let fields: Vec<&str> = value.split_ascii_whitespace().collect();
    match fields.len() {
        0 => {}
        1 => {
            current.push_str(fields[0]);
            *has_emitted = true;
        }
        _ => {
            current.push_str(fields[0]);
            result.push(std::mem::take(current));
            for f in &fields[1..fields.len() - 1] {
                result.push((*f).to_string());
            }
            *current = fields[fields.len() - 1].to_string();
            *has_emitted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ExecCommand, Pipeline, SimpleCommand};

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
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
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn expand_literal_word() {
        let mut shell = Shell::new();
        assert_eq!(expand(&lit("hello"), &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(expand(&lit(""), &mut shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
        assert_eq!(expand(&word, &mut shell), vec!["foobar".to_string()]);
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let mut shell = Shell::new();
        assert!(expand(&var_unq("DEFINITELY_NOT_SET_XYZ"), &mut shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let mut shell = Shell::new();
        assert_eq!(
            expand(&var_q("DEFINITELY_NOT_SET_XYZ"), &mut shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(expand(&var_q("HUCK_T"), &mut shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("HUCK_T", "a b".to_string());
        assert_eq!(
            expand(&var_unq("HUCK_T"), &mut shell),
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
            expand(&word, &mut shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand(&word, &mut shell), vec!["42".to_string()]);
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
            expand(&word, &mut shell),
            vec!["/tmp/huck_test/foo".to_string()]
        );
    }

    #[test]
    fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
        let mut shell = Shell::new();
        assert_eq!(expand(&Word(vec![WordPart::Var {
            name: "DEFINITELY_NOT_SET_REDIR".to_string(),
            quoted: false,
        }]), &mut shell).len(), 0);
    }

    #[test]
    fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
        let mut shell = Shell::new();
        shell.set("HUCK_T_TWOFIELD", "a b".to_string());
        assert_eq!(expand(&Word(vec![WordPart::Var {
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
        assert_eq!(expand(&word, &mut shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_command_sub_unquoted_splits() {
        let mut shell = Shell::new();
        let word = Word(vec![WordPart::CommandSub {
            sequence: echo_sequence(&["a", "b"]),
            quoted: false,
        }]);
        assert_eq!(
            expand(&word, &mut shell),
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
        assert_eq!(expand(&word, &mut shell), vec!["a b".to_string()]);
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
            expand(&word, &mut shell),
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
        assert_eq!(expand(&word, &mut shell), vec!["hi".to_string()]);
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
        assert_eq!(expand(&word, &mut shell), vec!["3".to_string()]);
        // The substitution still updates $? for the NEXT word/command.
        assert_eq!(shell.last_status(), 7);
    }

    #[test]
    fn expand_tilde_home_unset_falls_back_to_literal() {
        let mut shell = Shell::new();
        shell.unset("HOME");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
        assert_eq!(expand(&word, &mut shell), vec!["~"]);
    }

    #[test]
    fn expand_tilde_pwd_resolves_when_pwd_set() {
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var/tmp".to_string());
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand(&word, &mut shell), vec!["/var/tmp"]);
    }

    #[test]
    fn expand_tilde_pwd_unset_falls_back_to_literal_plus() {
        let mut shell = Shell::new();
        shell.unset("PWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
        assert_eq!(expand(&word, &mut shell), vec!["~+"]);
    }

    #[test]
    fn expand_tilde_oldpwd_unset_falls_back_to_literal_minus() {
        let mut shell = Shell::new();
        shell.unset("OLDPWD");
        let word = Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]);
        assert_eq!(expand(&word, &mut shell), vec!["~-"]);
    }

    #[test]
    fn expand_tilde_unknown_user_falls_back_to_literal() {
        let mut shell = Shell::new();
        let word = Word(vec![
            WordPart::Tilde(TildeSpec::User("definitely_not_a_real_user_xyz_42".to_string())),
            WordPart::Literal { text: "/x".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand(&word, &mut shell),
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
}
