use super::*;
use crate::command::{Command, ExecCommand, Pipeline, SimpleCommand};

fn lit(s: &str) -> Word {
    Word(vec![WordPart::Literal {
        text: s.to_string(),
        quoted: false,
    }])
}

#[test]
fn reconstruct_source_scalars() {
    fn rt(src: &str) -> String {
        // Parse `<src>` as the sole argument word of `echo` via the live
        // atom front-end, then reconstruct its source.
        let line = format!("echo {src}");
        let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            &line,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        ))
        .expect("parse")
        .expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected pipeline, got {other:?}"),
        };
        let w = match &pipeline.commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
            other => panic!("expected simple command, got {other:?}"),
        };
        reconstruct_word_source(&w)
    }
    assert_eq!(rt("abc"), "abc");
    assert_eq!(rt("$xs"), "$xs");
    assert_eq!(rt("a$x.b"), "a$x.b");
    assert_eq!(rt("${x}"), "$x"); // bare ${x} lexes to Var, brace lost
    assert_eq!(rt("${x:-d}"), "${x:-d}");
    assert_eq!(rt("${x##*/}"), "${x##*/}");
    assert_eq!(rt("${arr[@]}"), "${arr[@]}");
    assert_eq!(rt("${#x}"), "${#x}");
    assert_eq!(rt("$((1+2))"), "$((1+2))");
    assert_eq!(rt("$(ls -l)"), "$(ls -l)");
    assert_eq!(rt("$(a && b)"), "$(a && b)");
    assert_eq!(rt("$(a; b)"), "$(a; b)");
    assert_eq!(rt("\"$x\""), "\"$x\"");
    assert_eq!(rt("a"), "a");
    assert_eq!(rt("\"a b\""), "\"a b\"");
    assert_eq!(rt("pre\"$x\"post"), "pre\"$x\"post");
    assert_eq!(rt("\"$x$y\""), "\"$x$y\"");
}

#[test]
fn collapse_globstar_reduces_double_star_to_single() {
    assert_eq!(collapse_globstar("**"), "*");
    assert_eq!(collapse_globstar("***"), "*");
    assert_eq!(collapse_globstar("**/*.txt"), "*/*.txt");
    assert_eq!(collapse_globstar("a/**/b"), "a/*/b");
    assert_eq!(collapse_globstar("a*b"), "a*b"); // single star unchanged
    assert_eq!(collapse_globstar("[**]"), "[**]"); // inside bracket class: untouched
    assert_eq!(collapse_globstar("\\*\\*"), "\\*\\*"); // escaped stars: untouched
}

/// Test helper: project `Vec<Field>` back to `Vec<String>` so the existing
/// assertions don't have to construct `Field` literals. (Task 4 only
/// changes the signature; quoting propagation lands in Task 5.)
fn expand_strings(word: &Word, shell: &mut Shell) -> Vec<String> {
    expand(word, shell).into_iter().map(|f| f.chars).collect()
}

fn var_unq(name: &str) -> Word {
    Word(vec![WordPart::Var {
        name: name.to_string(),
        quoted: false,
    }])
}
fn var_q(name: &str) -> Word {
    Word(vec![WordPart::Var {
        name: name.to_string(),
        quoted: true,
    }])
}

/// Builds a synthetic Sequence for `echo <args>` — used to drive
/// CommandSub expansion in unit tests without invoking the lexer.
fn echo_sequence(args: &[&str]) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: lit("echo"),
                args: args.iter().map(|a| lit(a)).collect(),
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

fn exit_sequence(code: i32) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: lit("exit"),
                args: vec![lit(&code.to_string())],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

#[test]
fn expand_literal_word() {
    let mut shell = Shell::new();
    assert_eq!(
        expand_strings(&lit("hello"), &mut shell),
        vec!["hello".to_string()]
    );
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
        WordPart::Literal {
            text: "foo".to_string(),
            quoted: false,
        },
        WordPart::Literal {
            text: "bar".to_string(),
            quoted: false,
        },
    ]);
    assert_eq!(
        expand_strings(&word, &mut shell),
        vec!["foobar".to_string()]
    );
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
    assert_eq!(
        expand_strings(&var_q("HUCK_T"), &mut shell),
        vec!["a b".to_string()]
    );
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
        WordPart::Literal {
            text: "a".to_string(),
            quoted: false,
        },
        WordPart::Var {
            name: "HUCK_T".to_string(),
            quoted: false,
        },
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
        WordPart::Tilde {
            spec: TildeSpec::Home,
            assign_ctx: false,
        },
        WordPart::Literal {
            text: "/foo".to_string(),
            quoted: false,
        },
    ]);
    assert_eq!(
        expand_strings(&word, &mut shell),
        vec!["/tmp/huck_test/foo".to_string()]
    );
}

#[test]
fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
    let mut shell = Shell::new();
    assert_eq!(
        expand_strings(
            &Word(vec![WordPart::Var {
                name: "DEFINITELY_NOT_SET_REDIR".to_string(),
                quoted: false,
            }]),
            &mut shell
        )
        .len(),
        0
    );
}

#[test]
fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
    let mut shell = Shell::new();
    shell.set("HUCK_T_TWOFIELD", "a b".to_string());
    assert_eq!(
        expand_strings(
            &Word(vec![WordPart::Var {
                name: "HUCK_T_TWOFIELD".to_string(),
                quoted: false,
            }]),
            &mut shell
        )
        .len(),
        2
    );
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
        WordPart::Literal {
            text: "pre-".to_string(),
            quoted: false,
        },
        WordPart::Var {
            name: "HUCK_T_X".to_string(),
            quoted: false,
        },
        WordPart::Literal {
            text: "-post".to_string(),
            quoted: false,
        },
    ]);
    assert_eq!(
        expand_assignment(&word, &mut shell),
        "pre-x-post".to_string()
    );
}

#[test]
fn expand_assignment_unset_var_yields_empty_segment() {
    let mut shell = Shell::new();
    let word = Word(vec![
        WordPart::Literal {
            text: "[".to_string(),
            quoted: false,
        },
        WordPart::Var {
            name: "DEFINITELY_NOT_SET_ASN".to_string(),
            quoted: false,
        },
        WordPart::Literal {
            text: "]".to_string(),
            quoted: false,
        },
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
        WordPart::Literal {
            text: "pre".to_string(),
            quoted: false,
        },
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
fn run_substitution_records_last_cmd_sub_status() {
    let mut shell = Shell::new();
    let _ = run_substitution(&exit_sequence(7), &mut shell);
    assert_eq!(shell.last_cmd_sub_status(), Some(7));
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
    let word = Word(vec![WordPart::Tilde {
        spec: TildeSpec::Home,
        assign_ctx: false,
    }]);
    assert_eq!(expand_strings(&word, &mut shell), vec!["~"]);
}

#[test]
fn expand_assign_ctx_tilde_literal_under_posix_in_arg() {
    // POSIX: an assignment-context tilde (`~` after `=`/`:` in a name=value
    // word) in a plain command ARGUMENT stays literal. `expand()` is the
    // argument path.
    let mut shell = Shell::new();
    shell.export_set("HOME", "/usr/xyz".to_string());
    shell.shell_options.posix = true;
    let word = Word(vec![
        WordPart::Literal {
            text: "foo=bar:".to_string(),
            quoted: false,
        },
        WordPart::Tilde {
            spec: TildeSpec::Home,
            assign_ctx: true,
        },
    ]);
    assert_eq!(expand_strings(&word, &mut shell), vec!["foo=bar:~"]);
    // Non-posix: the same word DOES expand.
    shell.shell_options.posix = false;
    assert_eq!(expand_strings(&word, &mut shell), vec!["foo=bar:/usr/xyz"]);
    // POSIX word-start tilde (assign_ctx=false) still expands even in an arg.
    shell.shell_options.posix = true;
    let ws = Word(vec![WordPart::Tilde {
        spec: TildeSpec::Home,
        assign_ctx: false,
    }]);
    assert_eq!(expand_strings(&ws, &mut shell), vec!["/usr/xyz"]);
    // POSIX assignment PATH (expand_assignment) always resolves an
    // assign-ctx tilde — leading assignments / declaration builtins.
    assert_eq!(
        expand_assignment(&word, &mut shell),
        "foo=bar:/usr/xyz".to_string()
    );
}

#[test]
fn expand_tilde_pwd_resolves_when_pwd_set() {
    let mut shell = Shell::new();
    shell.export_set("PWD", "/var/tmp".to_string());
    let word = Word(vec![WordPart::Tilde {
        spec: TildeSpec::Pwd,
        assign_ctx: false,
    }]);
    assert_eq!(expand_strings(&word, &mut shell), vec!["/var/tmp"]);
}

#[test]
fn expand_tilde_pwd_unset_falls_back_to_literal_plus() {
    let mut shell = Shell::new();
    shell.unset("PWD");
    let word = Word(vec![WordPart::Tilde {
        spec: TildeSpec::Pwd,
        assign_ctx: false,
    }]);
    assert_eq!(expand_strings(&word, &mut shell), vec!["~+"]);
}

#[test]
fn expand_tilde_oldpwd_unset_falls_back_to_literal_minus() {
    let mut shell = Shell::new();
    shell.unset("OLDPWD");
    let word = Word(vec![WordPart::Tilde {
        spec: TildeSpec::OldPwd,
        assign_ctx: false,
    }]);
    assert_eq!(expand_strings(&word, &mut shell), vec!["~-"]);
}

#[test]
fn expand_tilde_unknown_user_falls_back_to_literal() {
    let mut shell = Shell::new();
    let word = Word(vec![
        WordPart::Tilde {
            spec: TildeSpec::User("definitely_not_a_real_user_xyz_42".to_string()),
            assign_ctx: false,
        },
        WordPart::Literal {
            text: "/x".to_string(),
            quoted: false,
        },
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
        WordPart::Literal {
            text: "PATH=".to_string(),
            quoted: false,
        },
        WordPart::Tilde {
            spec: TildeSpec::Home,
            assign_ctx: false,
        },
        WordPart::Literal {
            text: "/bin".to_string(),
            quoted: false,
        },
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
    let word = Word(vec![WordPart::Literal {
        text: "abc".to_string(),
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![false, false, false]);
}

#[test]
fn expand_literal_quoted_marks_chars_quoted() {
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal {
        text: "abc".to_string(),
        quoted: true,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![true, true, true]);
}

#[test]
fn expand_mixed_quoted_unquoted_literal_parts() {
    let mut shell = Shell::new();
    let word = Word(vec![
        WordPart::Literal {
            text: "foo".to_string(),
            quoted: false,
        },
        WordPart::Literal {
            text: "*".to_string(),
            quoted: true,
        },
        WordPart::Literal {
            text: "bar".to_string(),
            quoted: false,
        },
    ]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "foo*bar");
    assert_eq!(
        fields[0].quoted,
        vec![false, false, false, true, false, false, false]
    );
}

#[test]
fn expand_quoted_var_marks_chars_quoted() {
    let mut shell = Shell::new();
    shell.export_set("HUCK_Q", "val".to_string());
    let word = Word(vec![WordPart::Var {
        name: "HUCK_Q".to_string(),
        quoted: true,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![true, true, true]);
}

#[test]
fn expand_unquoted_var_marks_chars_unquoted() {
    let mut shell = Shell::new();
    shell.export_set("HUCK_Q", "val".to_string());
    let word = Word(vec![WordPart::Var {
        name: "HUCK_Q".to_string(),
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![false, false, false]);
}

#[test]
fn expand_tilde_marks_chars_unquoted() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/h".to_string());
    let word = Word(vec![WordPart::Tilde {
        spec: TildeSpec::Home,
        assign_ctx: false,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields[0].chars, "/h");
    assert_eq!(fields[0].quoted, vec![false, false]);
}

// ---- glob_expand_fields tests (v10 Task 6) ----------------------------------

#[test]
fn glob_expand_no_metachar_returns_chars_as_string() {
    let f = Field::from_unquoted("plain.txt");
    let out = glob_expand_fields(vec![f], &Shell::new());
    assert_eq!(out, vec!["plain.txt".to_string()]);
}

#[test]
fn glob_expand_quoted_metachar_treated_as_literal() {
    // All chars quoted including the `*` → no globbing.
    let f = Field::from_quoted("*.txt");
    let out = glob_expand_fields(vec![f], &Shell::new());
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
    let out = glob_expand_fields(vec![f], &Shell::new());

    std::env::set_current_dir(saved).unwrap();

    // No matches in empty temp dir → literal fallback.
    assert_eq!(out, vec!["a?".to_string()]);
}

#[test]
fn glob_expand_preserves_field_order() {
    let f1 = Field::from_unquoted("first");
    let f2 = Field::from_unquoted("second");
    let out = glob_expand_fields(vec![f1, f2], &Shell::new());
    assert_eq!(out, vec!["first".to_string(), "second".to_string()]);
}

// ---- glob_expand_fields filesystem tests (v10 Task 7) ----------------------

// CWD is process-global; serialize tests that mutate it. The lock is
// shared crate-wide so completion / executor / builtins tests that
// also chdir take the same one.
use crate::test_support::CWD_LOCK;

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

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
    let out = glob_expand_fields(vec![f], &Shell::new());

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["[abc".to_string()]);
}

#[test]
fn expand_then_glob_end_to_end_for_literal() {
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal {
        text: "hello".to_string(),
        quoted: false,
    }]);
    let argv = glob_expand_fields(expand(&word, &mut shell), &shell);
    assert_eq!(argv, vec!["hello".to_string()]);
}

/// Helper: a `WordPart::Arith` whose body is a single literal (the
/// post-v93 deferred-parse shape; arithmetic is parsed at eval time).
fn arith_part(text: &str) -> WordPart {
    WordPart::Arith {
        body: Word(vec![WordPart::Literal {
            text: text.to_string(),
            quoted: true,
        }]),
        quoted: false,
    }
}

#[test]
fn expand_arith_part_renders_decimal_result() {
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("2 + 3")]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "5");
    assert_eq!(fields[0].quoted, vec![true]);
}

#[test]
fn expand_arith_part_division_by_zero_is_nonfatal() {
    // An arith eval error (e.g. division by zero) in $((…)) is NO LONGER
    // a fatal expansion error — bash script-file mode prints the error
    // and continues. The error still surfaces via stderr;
    // pending_fatal_status stays None so the surrounding command list
    // runs to completion. The `-c` mode divergence is tracked as L-55.
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("1 / 0")]);
    let _ = expand(&word, &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}

#[test]
fn expand_arith_error_is_posix_fatal() {
    let mut shell = Shell::new();
    shell.shell_options.posix = true;
    shell.is_interactive = false;
    let word = Word(vec![arith_part("1 + ")]);
    let _ = expand(&word, &mut shell);
    assert_eq!(shell.pending_fatal_status, Some(127));
}

#[test]
fn expand_arith_part_invalid_lhs_assignment_is_nonfatal() {
    // A parse-time arith error (e.g. assignment to a non-lvalue) is also
    // non-fatal. The expansion contributes empty; pending_fatal_status
    // stays None.
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("1 + 2 = 3")]);
    let _ = expand(&word, &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}

#[test]
fn expand_assignment_arith_part_renders_decimal() {
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("6 * 7")]);
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
            word: Word(vec![WordPart::Literal {
                text: "fallback".to_string(),
                quoted: false,
            }]),
            colon: true,
        },
        quoted: false,
        subscript: None,
        indirect: false,
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
            word: Word(vec![WordPart::Literal {
                text: "a b c".to_string(),
                quoted: false,
            }]),
            colon: true,
        },
        quoted: true,
        subscript: None,
        indirect: false,
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
        subscript: None,
        indirect: false,
    }]);
    let fields = expand(&word, &mut shell);
    let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    assert_eq!(
        strings,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn expand_assignment_param_expansion_no_split() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E4".to_string(),
        modifier: ParamModifier::UseDefault {
            word: Word(vec![WordPart::Literal {
                text: "a b c".to_string(),
                quoted: false,
            }]),
            colon: true,
        },
        quoted: false,
        subscript: None,
        indirect: false,
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
            word: Word(vec![WordPart::Literal {
                text: "missing".to_string(),
                quoted: false,
            }]),
            colon: true,
        },
        quoted: false,
        subscript: None,
        indirect: false,
    }]);
    let fields = expand(&word, &mut shell);
    // v34 (Task 4): expand() now bails early on Fatal, stashing status on
    // pending_fatal_status and returning the partial (empty) result
    // without the end-of-word push, so fields is empty.
    assert_eq!(fields.len(), 0);
    assert_eq!(shell.pending_fatal_status, Some(1));
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
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: lit("false"),
                args: vec![],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    };
    let word = Word(vec![
        WordPart::CommandSub {
            sequence: false_cmd,
            quoted: false,
        },
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
    let out = glob_expand_fields(vec![f], &Shell::new());

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["top.txt".to_string()]);
}

// ---- Positional parameter expander tests (v22 Task 4) -------------------

#[test]
fn expand_dollar_digit_reads_positional() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["alpha".to_string(), "beta".to_string()];
    let w = Word(vec![WordPart::Var {
        name: "1".to_string(),
        quoted: false,
    }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "alpha");
}

#[test]
fn expand_dollar_digit_unset_is_empty() {
    let mut shell = Shell::new();
    let w = Word(vec![WordPart::Var {
        name: "1".to_string(),
        quoted: false,
    }]);
    let fields = expand(&w, &mut shell);
    // Unset positional → no field (consistent with unset var behaviour)
    assert!(fields.is_empty());
}

#[test]
fn expand_dollar_hash_is_arg_count() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let w = Word(vec![WordPart::Var {
        name: "#".to_string(),
        quoted: false,
    }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "3");
}

#[test]
fn expand_dollar_at_quoted_produces_field_per_arg() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a a".to_string(), "b".to_string()];
    let w = Word(vec![WordPart::AllArgs {
        joined: false,
        quoted: true,
    }]);
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
    let w = Word(vec![WordPart::AllArgs {
        joined: true,
        quoted: true,
    }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "a b c");
}

#[test]
fn expand_dollar_star_quoted_joins_with_first_ifs_char() {
    // POSIX § 2.5.2: "$*" joins positional args with the first
    // character of IFS. With IFS=":" and args a b c → "a:b:c".
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    shell.set("IFS", ":".to_string());
    let w = Word(vec![WordPart::AllArgs {
        joined: true,
        quoted: true,
    }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "a:b:c");
}

#[test]
fn expand_dollar_at_empty_produces_no_fields() {
    let mut shell = Shell::new();
    let w = Word(vec![WordPart::AllArgs {
        joined: false,
        quoted: true,
    }]);
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
    let w = Word(vec![WordPart::AllArgs {
        joined: false,
        quoted: false,
    }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 3, "fields: {fields:?}");
    assert_eq!(fields[0].chars, "hello");
    assert_eq!(fields[1].chars, "world");
    assert_eq!(fields[2].chars, "x");
}

#[test]
fn case_modifier_on_indexed_array_at() {
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell
        .set_indexed_element("a", 0, "foo".to_string())
        .unwrap();
    shell
        .set_indexed_element("a", 1, "bar".to_string())
        .unwrap();
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true, // quoted
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["FOO", "BAR"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn case_modifier_on_indexed_array_star() {
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell
        .set_indexed_element("a", 0, "foo".to_string())
        .unwrap();
    shell
        .set_indexed_element("a", 1, "bar".to_string())
        .unwrap();
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::Star,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, "FOO BAR"),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn remove_suffix_per_element_indexed() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell
        .set_indexed_element("a", 0, "foo.txt".to_string())
        .unwrap();
    shell
        .set_indexed_element("a", 1, "bar.md".to_string())
        .unwrap();
    let pat = Word(vec![WordPart::Literal {
        text: ".*".into(),
        quoted: false,
    }]);
    let result = expand_array_param(
        "a",
        &PM::RemoveSuffix {
            pattern: pat,
            longest: false,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["foo", "bar"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn empty_array_per_element_modifier() {
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => {
            assert!(words.is_empty(), "expected empty WordList, got {words:?}")
        }
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn case_modifier_on_associative_array() {
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".to_string(), "foo".to_string())
        .unwrap();
    shell
        .set_associative_element("m", "j".to_string(), "bar".to_string())
        .unwrap();
    let result = expand_array_param(
        "m",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => {
            let mut sorted = words.clone();
            sorted.sort();
            assert_eq!(sorted, vec!["BAR", "FOO"]);
        }
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn substitute_per_element_assoc() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, SubstAnchor, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".to_string(), "foo".to_string())
        .unwrap();
    shell
        .set_associative_element("m", "j".to_string(), "boo".to_string())
        .unwrap();
    let pat = Word(vec![WordPart::Literal {
        text: "o".into(),
        quoted: false,
    }]);
    let repl = Word(vec![WordPart::Literal {
        text: "X".into(),
        quoted: false,
    }]);
    let result = expand_array_param(
        "m",
        &PM::Substitute {
            pattern: pat,
            replacement: repl,
            anchor: SubstAnchor::None,
            all: false,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => {
            let mut sorted = words.clone();
            sorted.sort();
            assert_eq!(sorted, vec!["bXo", "fXo"]);
        }
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn assign_default_on_array_still_errors() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell
        .set_indexed_element("a", 0, "foo".to_string())
        .unwrap();
    let word = Word(vec![WordPart::Literal {
        text: "default".into(),
        quoted: false,
    }]);
    let result = expand_array_param(
        "a",
        &PM::AssignDefault { word, colon: true },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, ""),
        other => panic!("expected empty Value (catchall rejection), got {other:?}"),
    }
}

#[test]
fn error_if_unset_on_array_still_errors() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal {
        text: "msg".into(),
        quoted: false,
    }]);
    let result = expand_array_param(
        "a",
        &PM::ErrorIfUnset { word, colon: true },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, ""),
        other => panic!("expected empty Value (catchall rejection), got {other:?}"),
    }
}

#[test]
fn transform_assign_decl_on_indexed_at() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, TransformOp};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform {
            op: TransformOp::AssignDecl,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, r#"declare -a a=([0]="x" [1]="y")"#),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn transform_kv_words_on_indexed_yields_wordlist() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, TransformOp};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform {
            op: TransformOp::KvWords,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["0", "x", "1", "y"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn transform_attr_flags_indexed_yields_a() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, TransformOp};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform {
            op: TransformOp::AttrFlags,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, "a"),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn transform_assign_decl_on_assoc_at() {
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, TransformOp};
    use crate::param_expansion::ExpansionResult;
    use crate::shell_state::Shell;
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".to_string(), "v1".to_string())
        .unwrap();
    let result = expand_array_param(
        "m",
        &PM::Transform {
            op: TransformOp::AssignDecl,
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, r#"declare -A m=([k]="v1" )"#),
        other => panic!("expected Value, got {other:?}"),
    }
}
