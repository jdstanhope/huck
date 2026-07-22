use super::*;
use crate::command::{Command, ExecCommand, IfClause, Pipeline, Sequence, SimpleCommand};
use crate::lexer::{Word, WordPart};
use crate::test_support::CWD_LOCK;

fn exec_args(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

#[test]
fn render_test_leaf_forms() {
    let mut shell = Shell::new();
    shell.set("v", "hi".into());
    let parse_expr = |src: &str| match crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse")
    .expect("seq")
    .first
    {
        crate::command::Command::DoubleBracket { expr, .. } => *expr,
        other => panic!("expected [[ ]], got {other:?}"),
    };
    assert_eq!(
        render_test_leaf(&parse_expr("[[ -n $v ]]"), &mut shell),
        "-n hi"
    );
    assert_eq!(
        render_test_leaf(&parse_expr("[[ -z \"\" ]]"), &mut shell),
        "-z ''"
    );
    assert_eq!(
        render_test_leaf(&parse_expr("[[ $v == h* ]]"), &mut shell),
        "hi == h*"
    );
    assert_eq!(
        render_test_leaf(&parse_expr("[[ 5 -gt 3 ]]"), &mut shell),
        "5 -gt 3"
    );
}

#[test]
fn parse_exec_flags_plain_command() {
    let f = parse_exec_flags(&exec_args(&["echo", "hi"])).unwrap();
    assert!(!f.clear_env && !f.login && f.argv0.is_none());
    assert_eq!(f.operand_start, 0);
}

#[test]
fn parse_exec_flags_c_l_and_a_separate() {
    let f = parse_exec_flags(&exec_args(&["-c", "-l", "-a", "NAME", "prog"])).unwrap();
    assert!(f.clear_env && f.login);
    assert_eq!(f.argv0.as_deref(), Some("NAME"));
    assert_eq!(f.operand_start, 4);
}

#[test]
fn parse_exec_flags_clustered_and_inline_a() {
    // `-cla NAME` clusters -c, -l, and -a with NAME as the next word.
    let f = parse_exec_flags(&exec_args(&["-cla", "NAME", "prog"])).unwrap();
    assert!(f.clear_env && f.login);
    assert_eq!(f.argv0.as_deref(), Some("NAME"));
    assert_eq!(f.operand_start, 2);
    // `-aZERO prog`: argv0 is the inline remainder of the word.
    let f2 = parse_exec_flags(&exec_args(&["-aZERO", "prog"])).unwrap();
    assert_eq!(f2.argv0.as_deref(), Some("ZERO"));
    assert_eq!(f2.operand_start, 1);
}

#[test]
fn parse_exec_flags_double_dash_and_bare_dash() {
    let f = parse_exec_flags(&exec_args(&["--", "-prog"])).unwrap();
    assert_eq!(f.operand_start, 1);
    // A bare `-` is an operand, not a flag.
    let f2 = parse_exec_flags(&exec_args(&["-"])).unwrap();
    assert_eq!(f2.operand_start, 0);
}

#[test]
fn parse_exec_flags_errors() {
    assert!(parse_exec_flags(&exec_args(&["-Z"])).is_err());
    assert!(parse_exec_flags(&exec_args(&["-a"])).is_err()); // -a needs an argument
}

#[test]
fn parse_exec_flags_no_command_only_flags() {
    let f = parse_exec_flags(&exec_args(&["-c"])).unwrap();
    assert!(f.clear_env);
    assert_eq!(f.operand_start, 1); // == args.len(): no operand
}

#[test]
fn ps4_cmdsub_preserves_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(7);
    shell.set("PS4", "$(false)+ ".to_string());
    let _ = ps4(&mut shell);
    assert_eq!(shell.last_status(), 7, "rendering PS4 must not clobber $?");
}

#[test]
fn ps4_cmdsub_under_xtrace_does_not_recurse() {
    let mut shell = Shell::new();
    shell.shell_options.xtrace = true;
    shell.set("PS4", "$(true) ".to_string());
    // Without the xtrace-suppression fix, expanding PS4 runs `true` which is
    // traced -> re-enters ps4() -> infinite recursion -> stack overflow.
    let _ = ps4(&mut shell);
    // Reaching here (no abort) IS the assertion. Also confirm xtrace restored:
    assert!(
        shell.shell_options.xtrace,
        "xtrace must be restored after ps4"
    );
}

#[test]
fn open_writable_guard_creates_new_file() {
    let dir = std::env::temp_dir().join(format!("huck_nc_new_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("new.txt");
    let _ = std::fs::remove_file(&p);
    let f = open_writable(p.to_str().unwrap(), true);
    assert!(f.is_ok(), "guarded open should create a nonexistent file");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn open_writable_guard_blocks_existing_regular_file() {
    let dir = std::env::temp_dir().join(format!("huck_nc_block_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("exists.txt");
    std::fs::write(&p, b"orig").unwrap();
    let f = open_writable(p.to_str().unwrap(), true);
    assert!(
        f.is_err(),
        "guarded open must refuse an existing regular file"
    );
    assert_eq!(
        f.err().unwrap().to_string(),
        "cannot overwrite existing file"
    );
    assert_eq!(std::fs::read(&p).unwrap(), b"orig");
    let _ = std::fs::remove_file(&p);
}

#[test]
fn open_writable_guard_exempts_dev_null() {
    let f = open_writable("/dev/null", true);
    assert!(
        f.is_ok(),
        "guarded open must allow non-regular files like /dev/null"
    );
}

#[test]
fn open_writable_unguarded_truncates_existing() {
    let dir = std::env::temp_dir().join(format!("huck_nc_trunc_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("trunc.txt");
    std::fs::write(&p, b"original-content").unwrap();
    {
        let _f = open_writable(p.to_str().unwrap(), false).unwrap();
    }
    assert_eq!(
        std::fs::read(&p).unwrap(),
        b"",
        "unguarded open should truncate"
    );
    let _ = std::fs::remove_file(&p);
}

/// A top-level sequence wrapping a single Command.
fn seq_of(cmd: Command) -> Sequence {
    Sequence {
        first: cmd,
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline Sequence running `echo <word>`.
fn echo_seq(word: &str) -> Sequence {
    let ww = |s: &str| {
        Word(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: false,
        }])
    };
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: ww("echo"),
                args: vec![ww(word)],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline condition Sequence with a known exit status: true
/// (exit 0) when `succeed`, false (exit 1) otherwise. Built from the
/// side-effect-free `test` builtin — `test 0 -eq 0` succeeds,
/// `test 1 -eq 0` fails.
fn cond_seq(succeed: bool) -> Sequence {
    let ww = |s: &str| {
        Word(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: false,
        }])
    };
    let lhs = if succeed { "0" } else { "1" };
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: ww("test"),
                args: vec![ww(lhs), ww("-eq"), ww("0")],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

fn lit_word(s: &str) -> Word {
    Word(vec![WordPart::Literal {
        text: s.to_string(),
        quoted: false,
    }])
}

fn bare_assign(name: &str, value: Word) -> crate::command::Assignment {
    crate::command::Assignment {
        target: crate::command::AssignTarget::Bare(name.to_string()),
        value,
        append: false,
    }
}

fn exec(program: &str, args: &[&str]) -> SimpleCommand {
    SimpleCommand::Exec(ExecCommand {
        inline_assignments: Vec::new(),
        program: lit_word(program),
        args: args.iter().map(|a| lit_word(a)).collect(),
        redirects: Vec::new(),
        line: 0,
    })
}

fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(cmd)],
        }),
        rest: vec![],
        background: false,
    }
}

#[test]
fn if_true_condition_runs_then_body() {
    let clause = IfClause {
        condition: cond_seq(true),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: None,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "yes");
    assert_eq!(status, 0);
}

#[test]
fn if_false_condition_runs_else_body() {
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: Some(echo_seq("no")),
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "no");
}

#[test]
fn if_false_no_else_runs_nothing_status_zero() {
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: None,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn if_elif_selects_matching_branch() {
    use crate::command::ElifBranch;
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("a"),
        elif_branches: vec![ElifBranch {
            condition: cond_seq(true),
            body: echo_seq("b"),
        }],
        else_body: Some(echo_seq("c")),
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "b");
}

#[test]
fn execute_capturing_echo_returns_raw_output_with_newline() {
    // execute_capturing does NOT strip; that happens in expand::run_substitution.
    let seq = one_command_sequence(exec("echo", &["hi"]));
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(out, "hi\n");
    assert_eq!(status, 0);
}

#[test]
fn execute_capturing_exit_returns_status() {
    let seq = one_command_sequence(exec("exit", &["7"]));
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(out, "");
    assert_eq!(status, 7);
}

#[test]
fn execute_capturing_empty_echo() {
    let seq = one_command_sequence(exec("echo", &[]));
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(out, "\n");
    assert_eq!(status, 0);
}

// NOTE: `execute_capturing_builtin_pipeline_captures_terminal_stage` moved to
// `tests/forking_execution_serial.rs` as
// `builtin_pipeline_capture_returns_terminal_stage_output` (a capture-context
// builtin pipeline forks its non-terminal stage in-process; unsafe to run
// concurrently with other tests — issue #184).

#[test]
fn execute_capturing_ignores_background_flag_runs_synchronously() {
    // `$(cmd &)` must wait and capture, not spawn an escaped bg job.
    let seq = Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(exec("echo", &["captured"]))],
        }),
        rest: vec![],
        background: true,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(out, "captured\n");
    assert_eq!(status, 0);
    // And nothing should have been registered in the job table.
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn give_terminal_to_silently_succeeds_on_non_tty() {
    // cargo test runs without a controlling terminal; tcsetpgrp returns
    // ENOTTY. The helper must swallow it.
    give_terminal_to(1); // bogus pgid; we only care that we don't panic
}

#[test]
fn stray_break_at_top_level_errors_with_status_0() {
    // `break` with no enclosing loop (loop_depth==0): emits diagnostic
    // to stderr and returns status 0 (matches bash 5.2 behavior —
    // bash returns 0, not 1, for break/continue outside a loop).
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| {
        Word(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: false,
        }])
    };
    let seq = Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: ww("break"),
                args: vec![],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(status, 0);
}

#[test]
fn brace_group_assignments_affect_current_shell() {
    // A brace group has NO subshell isolation — `x=value` inside it
    // is visible after.
    let assign = Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Assign(
                vec![bare_assign(
                    "BG_X",
                    Word(vec![WordPart::Literal {
                        text: "hello".to_string(),
                        quoted: false,
                    }]),
                )],
                0,
            ))],
        }),
        rest: vec![],
        background: false,
    };
    let group = Sequence {
        first: Command::BraceGroup(Box::new(assign)),
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_, status) = execute_capturing(&group, &mut shell);
    assert_eq!(status, 0);
    assert_eq!(shell.get("BG_X"), Some("hello"));
}

#[test]
fn redirect_target_does_not_glob() {
    let _g = CWD_LOCK.lock().unwrap();
    // Create a temp dir with a real file matching the literal pattern name.
    let tmp = tempfile::tempdir().unwrap();
    // The file is named literally "starfile" — `*` should not glob to it.
    std::fs::write(tmp.path().join("starfile"), b"hello\n").unwrap();
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    // Build a redirect target word containing unquoted `*` and verify expand_single
    // returns the literal "*" (not a glob match) — proving redirects bypass globbing.
    let word = crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
        text: "*".to_string(),
        quoted: false,
    }]);
    let mut shell = crate::shell_state::Shell::new();
    let result = expand_single(&word, &mut shell, &mut std::io::stderr());

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(result, Ok("*".to_string()));
}

use crate::command::WhileClause;

/// A Sequence wrapping a single `while`/`until` clause.
fn while_seq(clause: WhileClause) -> Sequence {
    Sequence {
        first: Command::While(Box::new(clause)),
        rest: vec![],
        background: false,
    }
}

use crate::command::ForClause;

/// A Sequence wrapping a single `for` clause.
fn for_seq(clause: ForClause) -> Sequence {
    Sequence {
        first: Command::For(Box::new(clause)),
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline Sequence running `echo $<var>` (the variable expanded).
fn echo_var_seq(var: &str) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: Word(vec![WordPart::Literal {
                    text: "echo".to_string(),
                    quoted: false,
                }]),
                args: vec![Word(vec![WordPart::Var {
                    name: var.to_string(),
                    quoted: false,
                }])],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline Sequence running the `continue` builtin.
fn continue_seq() -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: Word(vec![WordPart::Literal {
                    text: "continue".to_string(),
                    quoted: false,
                }]),
                args: vec![],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

#[test]
fn for_iterates_each_value_in_order() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        has_in: true,
        body: echo_var_seq("x"),
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a", "b", "c"]);
    assert_eq!(status, 0);
}

#[test]
fn for_empty_list_runs_body_zero_times() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![],
        has_in: true,
        body: echo_seq("hi"),
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn select_empty_list_runs_no_body_and_restores_depth() {
    let mut sh = Shell::new();
    // `select x in ; do exit 7; done` — empty `in` → body never runs → status 0.
    let outcome = crate::shell::process_line("select x in; do exit 7; done", &mut sh, false);
    assert_eq!(sh.loop_depth, 0, "loop_depth must be restored");
    // The shell did not exit 7 (body never ran):
    assert!(!matches!(outcome, ExecOutcome::Exit(7)));
}

#[test]
fn for_without_in_iterates_positionals() {
    // M-24a: `for x; do ... done` with no `in` iterates "$@".
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![],
        has_in: false,
        body: echo_var_seq("x"),
        line: 0,
    };
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a", "b", "c"]);
    assert_eq!(status, 0);
}

#[test]
fn for_variable_holds_last_value_after_loop() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        has_in: true,
        body: echo_var_seq("x"),
        line: 0,
    };
    let mut shell = Shell::new();
    execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(shell.get("x"), Some("c"));
}

#[test]
fn for_break_stops_iteration() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        has_in: true,
        body: break_seq(),
        line: 0,
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(shell.get("x"), Some("a"));
    assert_eq!(status, 0);
}

#[test]
fn for_continue_advances_through_all_values() {
    // body: `continue ; echo NOPE` — `continue` must skip the echo on
    // every iteration, so nothing prints, yet all values are visited.
    let echo_nope = Command::Pipeline(Pipeline {
        negate: false,
        commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![WordPart::Literal {
                text: "echo".to_string(),
                quoted: false,
            }]),
            args: vec![Word(vec![WordPart::Literal {
                text: "NOPE".to_string(),
                quoted: false,
            }])],
            redirects: Vec::new(),
            line: 0,
        }))],
    });
    let mut body = continue_seq();
    body.rest.push((crate::command::Connector::Semi, echo_nope));
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        has_in: true,
        body,
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.trim(), "", "continue should skip the echo: {out:?}");
    assert_eq!(shell.get("x"), Some("c"));
    assert_eq!(status, 0);
}

/// A one-pipeline Sequence running the `break` builtin.
fn break_seq() -> Sequence {
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| {
        Word(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: false,
        }])
    };
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: ww("break"),
                args: vec![],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    }
}

#[test]
fn while_false_condition_runs_body_zero_times() {
    let clause = WhileClause {
        condition: cond_seq(false),
        body: echo_seq("x"),
        until: false,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn while_true_body_breaks_runs_once() {
    // while (true); do break; done — `break` ends the loop after one
    // iteration. Reaching the assertion at all proves termination.
    let clause = WhileClause {
        condition: cond_seq(true),
        body: break_seq(),
        until: false,
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(status, 0);
}

#[test]
fn until_true_condition_runs_body_zero_times() {
    // until (test 0 -eq 0 -> true); do echo x; done — `until` stops
    // immediately when the condition is true.
    let clause = WhileClause {
        condition: cond_seq(true),
        body: echo_seq("x"),
        until: true,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
}

// ----- case statement tests -----------------------------------------------

use crate::command::{CaseClause, CaseItem, CaseTerminator};

/// A Sequence wrapping a single `case` clause.
fn case_seq(clause: CaseClause) -> Sequence {
    Sequence {
        first: Command::Case(Box::new(clause)),
        rest: vec![],
        background: false,
    }
}

/// A CaseItem with a `;;` (Break) terminator.
fn item(patterns: &[&str], body: Option<Sequence>) -> CaseItem {
    CaseItem {
        patterns: patterns.iter().map(|p| lit_word(p)).collect(),
        body,
        terminator: CaseTerminator::Break,
    }
}

#[test]
fn case_runs_first_matching_clause() {
    let clause = CaseClause {
        subject: lit_word("foo"),
        items: vec![
            item(&["foo"], Some(echo_seq("matched"))),
            item(&["bar"], Some(echo_seq("other"))),
        ],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "matched");
    assert_eq!(status, 0);
}

#[test]
fn case_glob_pattern_matches() {
    let clause = CaseClause {
        subject: lit_word("report.txt"),
        items: vec![item(&["*.txt"], Some(echo_seq("text")))],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "text");
}

#[test]
fn case_alternation_matches_any() {
    let clause = CaseClause {
        subject: lit_word("b"),
        items: vec![item(&["a", "b", "c"], Some(echo_seq("hit")))],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "hit");
}

#[test]
fn case_no_match_is_status_zero_no_output() {
    let clause = CaseClause {
        subject: lit_word("x"),
        items: vec![item(&["y"], Some(echo_seq("nope")))],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn case_empty_body_is_status_zero() {
    let clause = CaseClause {
        subject: lit_word("x"),
        items: vec![item(&["x"], None)],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn case_fall_through_runs_next_body() {
    // a) echo one ;&  *) echo two ;;
    let clause = CaseClause {
        subject: lit_word("a"),
        items: vec![
            CaseItem {
                patterns: vec![lit_word("a")],
                body: Some(echo_seq("one")),
                terminator: CaseTerminator::FallThrough,
            },
            item(&["*"], Some(echo_seq("two"))),
        ],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
}

#[test]
fn case_continue_match_keeps_testing() {
    // a) echo one ;;&  a) echo two ;;
    let clause = CaseClause {
        subject: lit_word("a"),
        items: vec![
            CaseItem {
                patterns: vec![lit_word("a")],
                body: Some(echo_seq("one")),
                terminator: CaseTerminator::ContinueMatch,
            },
            item(&["a"], Some(echo_seq("two"))),
        ],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
}

#[test]
fn function_def_registers_and_returns_zero() {
    let body = Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: Word(vec![WordPart::Literal {
                    text: "echo".into(),
                    quoted: false,
                }]),
                args: vec![Word(vec![WordPart::Literal {
                    text: "hi".into(),
                    quoted: false,
                }])],
                redirects: Vec::new(),
                line: 0,
            }))],
        }),
        rest: vec![],
        background: false,
    };
    let def = Sequence {
        first: Command::FunctionDef {
            name: "f".to_string(),
            body: Box::new(Command::BraceGroup(Box::new(body))),
        },
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_, status) = execute_capturing(&def, &mut shell);
    assert_eq!(status, 0);
    assert!(shell.functions.contains_key("f"));
}

#[test]
fn case_quoted_metacharacter_matches_literally() {
    // pattern is a quoted `*` — matches the literal string "*", not "abc"
    let star_pattern = Word(vec![WordPart::Literal {
        text: "*".to_string(),
        quoted: true,
    }]);
    let make = |subj: &str| CaseClause {
        subject: lit_word(subj),
        items: vec![CaseItem {
            patterns: vec![star_pattern.clone()],
            body: Some(echo_seq("hit")),
            terminator: CaseTerminator::Break,
        }],
        line: 0,
    };
    let mut shell = Shell::new();
    let (out_star, _) = execute_capturing(&case_seq(make("*")), &mut shell);
    assert_eq!(
        out_star.trim(),
        "hit",
        "literal * should match the string \"*\""
    );
    let (out_abc, _) = execute_capturing(&case_seq(make("abc")), &mut shell);
    assert_eq!(out_abc.trim(), "", "quoted * must not act as a wildcard");
}

// ----- apply/restore inline assignment helper tests ----------------------

#[test]
fn apply_inline_assignments_sets_and_exports_left_to_right() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/home/test".to_string());
    let assigns = vec![
        bare_assign("A", lit_word("1")),
        bare_assign(
            "B",
            Word(vec![WordPart::Var {
                name: "A".to_string(),
                quoted: false,
            }]),
        ),
    ];
    let snap = {
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink)
    }
    .expect("ok");
    assert_eq!(shell.get("A"), Some("1"));
    assert_eq!(shell.get("B"), Some("1"));
    assert!(shell.is_exported("A"));
    assert!(shell.is_exported("B"));
    assert_eq!(snap.vars.len(), 2);
}

#[test]
fn restore_inline_assignments_restores_prior_unset_state() {
    let mut shell = Shell::new();
    let assigns = vec![bare_assign("FOO", lit_word("bar"))];
    let snap = {
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink)
    }
    .expect("ok");
    assert_eq!(shell.get("FOO"), Some("bar"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), None);
}

#[test]
fn restore_inline_assignments_restores_prior_value_unexported() {
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    assert!(!shell.is_exported("FOO"));
    let assigns = vec![bare_assign("FOO", lit_word("inner"))];
    let snap = {
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink)
    }
    .expect("ok");
    assert_eq!(shell.get("FOO"), Some("inner"));
    assert!(shell.is_exported("FOO"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
}

#[test]
fn restore_inline_assignments_restores_prior_value_exported() {
    let mut shell = Shell::new();
    shell.export_set("FOO", "outer".to_string());
    let assigns = vec![bare_assign("FOO", lit_word("inner"))];
    let snap = {
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink)
    }
    .expect("ok");
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(shell.is_exported("FOO"));
}

#[test]
fn restore_inline_assignments_handles_repeated_name() {
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    let assigns = vec![
        bare_assign("FOO", lit_word("a")),
        bare_assign("FOO", lit_word("b")),
    ];
    let snap = {
        let mut sink = StdoutSink::Terminal;
        let mut err_sink = StderrSink::Terminal;
        apply_inline_assignments(&assigns, &mut shell, &mut sink, &mut err_sink)
    }
    .expect("ok");
    assert_eq!(shell.get("FOO"), Some("b"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
}

// ----- run_exec_single inline assignment integration tests ---------------

#[test]
fn run_exec_single_external_command_inline_assignment_restores_after() {
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![bare_assign("FOO", lit_word("inner"))],
        program: lit_word("true"),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    });
    let pipeline = Pipeline {
        negate: false,
        commands: vec![Command::Simple(cmd)],
    };
    let seq = Sequence {
        first: Command::Pipeline(pipeline),
        rest: vec![],
        background: false,
    };
    let _ = execute(&seq, &mut shell, "FOO=inner true");
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
}

#[test]
fn run_exec_single_function_call_inline_assignment_does_not_persist() {
    let mut shell = Shell::new();
    // Define a no-op function via the parser.
    if let Ok(Some(seq)) = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        "myfunc() { echo ok; }",
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    )) {
        let _ = execute(&seq, &mut shell, "myfunc() { echo ok; }");
    }
    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
        program: lit_word("myfunc"),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    });
    let pipeline = Pipeline {
        negate: false,
        commands: vec![Command::Simple(cmd)],
    };
    let seq = Sequence {
        first: Command::Pipeline(pipeline),
        rest: vec![],
        background: false,
    };
    let _ = execute(&seq, &mut shell, "FOO=val myfunc");
    // bash: a prefix assignment does NOT persist across a function call.
    assert_eq!(shell.get("FOO"), None);
}

#[test]
fn prefix_assign_restores_prior_value_over_function_global_mutation() {
    // Function's own global write to the same var is clobbered by the restore.
    let mut shell = Shell::new();
    exec_script("v=1\nf(){ v=99; }\nv=5 f\n", &mut shell);
    assert_eq!(shell.get("v"), Some("1"));
}

#[test]
fn prefix_assign_restores_prior_value_over_function_local() {
    let mut shell = Shell::new();
    exec_script("v=1\nf(){ local v=99; }\nv=5 f\n", &mut shell);
    assert_eq!(shell.get("v"), Some("1"));
}

#[test]
fn prefix_assign_restores_unset_over_function_unset() {
    // Function unsets the var; restore reinstates the prior value.
    let mut shell = Shell::new();
    exec_script("v=1\nf(){ unset v; }\nv=5 f\n", &mut shell);
    assert_eq!(shell.get("v"), Some("1"));
}

#[test]
fn prefix_assign_with_no_prior_var_is_unset_after_function() {
    let mut shell = Shell::new();
    exec_script("f(){ :; }\nv=5 f\n", &mut shell);
    assert_eq!(shell.get("v"), None);
}

#[test]
fn posix_special_persist_survives_enclosing_prefix() {
    // func3.sub line 155: outer prefix restore must NOT clobber the inner
    // posix special-builtin persist.
    let mut shell = Shell::new();
    exec_script(
        "set -o posix\nvar=0\nf(){ var=20 return 5; }\nvar=30 f\n",
        &mut shell,
    );
    assert_eq!(shell.get("var"), Some("20"));
    assert!(shell.inline_scopes.is_empty(), "scope stack balanced");
}

#[test]
fn export_under_enclosing_prefix_does_not_survive_restore_in_default_mode() {
    // export is persistent (absorbs its named var), but in DEFAULT mode that
    // persist must NOT propagate through an enclosing same-name prefix-restore.
    // bash 5.2.21: FOO=30 f restores FOO to 0 even though f did `FOO=20 export FOO`.
    let mut shell = Shell::new();
    exec_script("FOO=0\nf(){ FOO=20 export FOO; }\nFOO=30 f\n", &mut shell);
    assert_eq!(shell.get("FOO"), Some("0"));
    assert!(shell.inline_scopes.is_empty());
}

#[test]
fn export_under_enclosing_prefix_survives_in_posix_mode() {
    let mut shell = Shell::new();
    exec_script(
        "set -o posix\nFOO=0\nf(){ FOO=20 export FOO; }\nFOO=30 f\n",
        &mut shell,
    );
    assert_eq!(shell.get("FOO"), Some("20"));
    assert!(shell.inline_scopes.is_empty());
}

#[test]
fn exec_with_redirect_does_not_leak_inline_scope() {
    // exec returns via its own early path; the scope-stack push must sit
    // below the exec block so exec never pushes (else it leaks an entry).
    let mut shell = Shell::new();
    exec_script("FOO=bar exec 3>&1\n", &mut shell);
    assert!(
        shell.inline_scopes.is_empty(),
        "exec must not leak an inline scope"
    );
}

#[test]
fn default_special_persist_does_not_survive_enclosing_prefix() {
    let mut shell = Shell::new();
    exec_script("var=0\nf(){ var=20 return 5; }\nvar=30 f\n", &mut shell);
    assert_eq!(shell.get("var"), Some("0"));
    assert!(shell.inline_scopes.is_empty());
}

#[test]
fn posix_special_persist_survives_multi_level_enclosing() {
    let mut shell = Shell::new();
    exec_script(
        "set -o posix\na=0\nm(){ a=3 return; }\no(){ a=2 m; }\na=1 o\n",
        &mut shell,
    );
    assert_eq!(shell.get("a"), Some("3"));
    assert!(shell.inline_scopes.is_empty());
}

#[test]
fn run_exec_single_special_builtin_inline_assignment_persists() {
    let mut shell = Shell::new();
    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
        program: lit_word("export"),
        args: vec![lit_word("FOO")],
        redirects: Vec::new(),
        line: 0,
    });
    let pipeline = Pipeline {
        negate: false,
        commands: vec![Command::Simple(cmd)],
    };
    let seq = Sequence {
        first: Command::Pipeline(pipeline),
        rest: vec![],
        background: false,
    };
    let _ = execute(&seq, &mut shell, "FOO=val export FOO");
    assert_eq!(shell.get("FOO"), Some("val"));
    assert!(shell.is_exported("FOO"));
}

#[test]
fn special_builtin_prefix_does_not_persist_in_default_mode() {
    // `:` is a special builtin; in DEFAULT mode the prefix is temporary.
    let mut shell = Shell::new();
    exec_script("var=0\nvar=20 :\n", &mut shell);
    assert_eq!(
        shell.get("var"),
        Some("0"),
        "default mode restores the prefix"
    );
}

#[test]
fn special_builtin_prefix_persists_in_posix_mode() {
    let mut shell = Shell::new();
    exec_script("set -o posix\nvar=0\nvar=20 :\n", &mut shell);
    assert_eq!(
        shell.get("var"),
        Some("20"),
        "posix mode persists the prefix"
    );
}

#[test]
fn export_prefix_persists_in_default_mode() {
    // export/readonly absorb their named var even in default mode (regression
    // guard alongside run_exec_single_special_builtin_inline_assignment_persists).
    let mut shell = Shell::new();
    exec_script("FOO=val export FOO\n", &mut shell);
    assert_eq!(shell.get("FOO"), Some("val"), "export keeps its named var");
}

/// A `for`/`select` loop may use a `{ … }` brace group in place of
/// `do … done` (ksh-derived, accepted by bash). The loop must actually
/// iterate, and `break`/`continue` must work inside the brace body.
#[test]
#[cfg(unix)]
fn for_brace_body_iterates_with_break_continue() {
    let _g = CWD_LOCK.lock().unwrap();
    let run = |src: &str| -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        {
            let mut out = StdoutSink::Capture(&mut buf);
            let mut err = StderrSink::Capture(&mut Vec::new());
            let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
                src,
                &Default::default(),
                crate::lexer::LexerOptions::default(),
            ))
            .expect("parse")
            .expect("seq");
            execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
        }
        String::from_utf8_lossy(&buf).into_owned()
    };
    // Word-list brace body iterates.
    assert_eq!(run("for x in a b c; { echo $x; }"), "a\nb\nc\n");
    // C-style brace body iterates.
    assert_eq!(run("for ((i=0;i<3;i++)) { echo $i; }"), "0\n1\n2\n");
    // break/continue inside the brace body.
    assert_eq!(
        run("for x in 1 2 3 4; { [ $x = 3 ] && break; [ $x = 1 ] && continue; echo $x; }"),
        "2\n"
    );
    // Nested brace-body loops.
    assert_eq!(
        run("for x in 1 2; { for y in a b; { echo $x$y; } }"),
        "1a\n1b\n2a\n2b\n"
    );
}

// ----- external-process stderr capture / Merged --------------------------

/// `/bin/sh -c 'echo out; echo err >&2'` with split capture sinks:
/// stdout lands in `buf_out`, stderr lands in `buf_err`. Exercises the
/// `run_subprocess` Capture-stderr branch (Stdio::piped on fd 2 + threaded
/// drain). Bash-equivalent: `bash -c '...' 1>out 2>err`.
#[test]
#[cfg(unix)]
fn external_process_stderr_is_captured() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut buf_out: Vec<u8> = Vec::new();
    let mut buf_err: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    {
        let mut out = StdoutSink::Capture(&mut buf_out);
        let mut err = StderrSink::Capture(&mut buf_err);
        let src = "/bin/sh -c 'echo out; echo err >&2'";
        let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            src,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        ))
        .expect("parse")
        .expect("seq");
        execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
    }
    assert_eq!(String::from_utf8_lossy(&buf_out), "out\n");
    assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
}

/// `/bin/sh -c 'printf out; printf err 1>&2; printf out2'` with
/// `StderrSink::Merged` routes fd 2 onto fd 1 (the capture pipe) in the
/// child via a `pre_exec` dup2(1,2). Both streams hit the same kernel pipe;
/// kernel-level ordering matches the source-code writes.
/// Bash-equivalent: `bash -c '...' 2>&1`.
#[test]
#[cfg(unix)]
fn external_process_merged_stderr_interleaves_via_kernel() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    {
        let mut out = StdoutSink::Capture(&mut buf);
        let mut err = StderrSink::Merged;
        let src = "/bin/sh -c 'printf out; printf err 1>&2; printf out2'";
        let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            src,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        ))
        .expect("parse")
        .expect("seq");
        execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
    }
    assert_eq!(String::from_utf8_lossy(&buf), "outerrout2");
}

/// Multi-stage pipeline with a stage writing to stderr — the shared
/// `StderrSink::Capture` pipe (per-stage dup'd write-end) should collect
/// every stage's stderr into the same buffer. Bash-equivalent:
/// `bash -c 'echo a; echo err >&2 | cat' 1>out 2>err` (rough analog).
#[test]
#[cfg(unix)]
fn pipeline_stage_stderr_is_captured() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut buf_out: Vec<u8> = Vec::new();
    let mut buf_err: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    {
        let mut out = StdoutSink::Capture(&mut buf_out);
        let mut err = StderrSink::Capture(&mut buf_err);
        // First stage prints to stderr (visible in err buf), pipes nothing.
        // Second stage `cat` reads (empty) and writes nothing → stdout empty.
        let src = "/bin/sh -c 'echo err >&2' | cat";
        let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            src,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        ))
        .expect("parse")
        .expect("seq");
        execute_with_sink(&seq, &mut shell, src, &mut out, &mut err);
    }
    assert_eq!(String::from_utf8_lossy(&buf_out), "");
    assert_eq!(String::from_utf8_lossy(&buf_err), "err\n");
}

// ----- classify_stage unit tests (Task 4) ----------------------------------

/// Helper: builds `Command::Simple(SimpleCommand::Exec(...))` for `program`.
fn simple_exec_cmd(program: &str) -> Command {
    Command::Simple(SimpleCommand::Exec(ExecCommand {
        inline_assignments: Vec::new(),
        program: lit_word(program),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    }))
}

/// Helper: builds `Command::Simple(SimpleCommand::Exec(...))` with a
/// dynamic (Var) program word — simulates `$cmd args`.
fn dynamic_exec_cmd() -> Command {
    use crate::lexer::WordPart;
    Command::Simple(SimpleCommand::Exec(ExecCommand {
        inline_assignments: Vec::new(),
        program: Word(vec![WordPart::Var {
            name: "cmd".to_string(),
            quoted: false,
        }]),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    }))
}

#[test]
fn classify_stage_external_for_unknown_command() {
    // `cat` is not a builtin and not in functions → External.
    let shell = Shell::new();
    let cmd = simple_exec_cmd("cat");
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::External(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_builtin() {
    // `cd` is a builtin → InProcess.
    let shell = Shell::new();
    let cmd = simple_exec_cmd("cd");
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_echo_builtin() {
    // `echo` is a builtin → InProcess.
    let shell = Shell::new();
    let cmd = simple_exec_cmd("echo");
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_function() {
    // A function named `myfunc` exists in shell.functions → InProcess.
    let mut shell = Shell::new();
    // Register myfunc in the function table via the parser.
    if let Ok(Some(seq)) = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        "myfunc() { :; }",
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    )) {
        let _ = execute(&seq, &mut shell, "myfunc() { :; }");
    }
    let cmd = simple_exec_cmd("myfunc");
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_compound_if() {
    // An `if` clause is never External.
    use crate::command::IfClause;
    let shell = Shell::new();
    let cmd = Command::If(Box::new(IfClause {
        condition: cond_seq(true),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: None,
    }));
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_assign_only_stage() {
    // Assignment-only stage (SimpleCommand::Assign) → InProcess.
    let shell = Shell::new();
    let cmd = Command::Simple(SimpleCommand::Assign(
        vec![bare_assign("FOO", lit_word("bar"))],
        0,
    ));
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

#[test]
fn classify_stage_inprocess_for_dynamic_program() {
    // `$cmd args` — program word is a Var → static text resolution fails → InProcess.
    let shell = Shell::new();
    let cmd = dynamic_exec_cmd();
    assert!(matches!(
        classify_stage(&cmd, &shell),
        StageKind::InProcess(_)
    ));
}

// ----- resolve_fd_target unit tests (Task 2 / v29) -------------------------

#[test]
fn resolve_fd_target_parses_literal_number() {
    let mut shell = Shell::new();
    let word = lit_word("1");
    assert_eq!(resolve_fd_target(&word, &mut shell).unwrap(), 1);
}

#[test]
fn resolve_fd_target_rejects_non_numeric() {
    let mut shell = Shell::new();
    let word = lit_word("notanumber");
    assert!(resolve_fd_target(&word, &mut shell).is_err());
}

// ----- program_static_text unit tests (Task 4) ----------------------------

#[test]
fn program_static_text_returns_some_for_plain_literal() {
    use crate::command::ExecCommand;
    let exec = ExecCommand {
        inline_assignments: Vec::new(),
        program: lit_word("cat"),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    };
    assert_eq!(exec.program_static_text(), Some("cat".to_string()));
}

#[test]
fn program_static_text_returns_none_for_quoted_literal() {
    use crate::command::ExecCommand;
    use crate::lexer::WordPart;
    let exec = ExecCommand {
        inline_assignments: Vec::new(),
        program: Word(vec![WordPart::Literal {
            text: "cat".to_string(),
            quoted: true,
        }]),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    };
    // Quoted literal → None (could be a function or builtin masked by quoting).
    assert_eq!(exec.program_static_text(), None);
}

#[test]
fn program_static_text_returns_none_for_var_word() {
    use crate::command::ExecCommand;
    use crate::lexer::WordPart;
    let exec = ExecCommand {
        inline_assignments: Vec::new(),
        program: Word(vec![WordPart::Var {
            name: "cmd".to_string(),
            quoted: false,
        }]),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    };
    assert_eq!(exec.program_static_text(), None);
}

#[test]
fn program_static_text_returns_none_for_multi_part_word() {
    use crate::command::ExecCommand;
    use crate::lexer::WordPart;
    // Two parts: e.g. `cat` + some suffix (weird, but defensive).
    let exec = ExecCommand {
        inline_assignments: Vec::new(),
        program: Word(vec![
            WordPart::Literal {
                text: "ca".to_string(),
                quoted: false,
            },
            WordPart::Literal {
                text: "t".to_string(),
                quoted: false,
            },
        ]),
        args: vec![],
        redirects: Vec::new(),
        line: 0,
    };
    assert_eq!(exec.program_static_text(), None);
}

// --- v26 special parameters: executor wiring ---

/// Helper: parse and execute a complete multi-statement script by
/// accumulating lines and executing each parseable sequence in turn,
/// mirroring how the interactive REPL processes input.
fn exec_script(src: &str, shell: &mut Shell) {
    // The shell's normal execution reads one token stream at a time from
    // the parser. We can simulate this by iterating over lines and
    // accumulating until we have a parseable sequence.
    let mut buf = String::new();
    for line in src.lines() {
        buf.push_str(line);
        buf.push('\n');
        match crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            &buf,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        )) {
            Ok(Some(seq)) => {
                let outcome = execute(&seq, shell, &buf);
                buf.clear();
                if matches!(outcome, ExecOutcome::Exit(_)) {
                    return;
                }
            }
            Ok(None) => {
                buf.clear();
            }
            Err(_) => {
                // Incomplete parse — keep accumulating.
                continue;
            }
        }
    }
    // Execute any remaining buffered content.
    if !buf.is_empty()
        && let Ok(Some(seq)) = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
            &buf,
            &Default::default(),
            crate::lexer::LexerOptions::default(),
        ))
    {
        let _ = execute(&seq, shell, &buf);
    }
}

#[test]
fn posix_source_not_found_is_fatal() {
    let mut shell = Shell::new();
    exec_script("set -o posix\n. /no/such/huck_file_xyz\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, Some(1));
}
#[test]
fn default_source_not_found_is_not_fatal() {
    let mut shell = Shell::new();
    exec_script(". /no/such/huck_file_xyz\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}
#[test]
fn posix_function_named_special_builtin_is_fatal() {
    let mut shell = Shell::new();
    exec_script("set -o posix\neval() { :; }\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, Some(2));
    assert!(
        !shell.functions.contains_key("eval"),
        "function not defined"
    );
}
#[test]
fn default_function_named_special_builtin_is_allowed() {
    let mut shell = Shell::new();
    exec_script("eval() { :; }\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
    assert!(shell.functions.contains_key("eval"));
}
#[test]
fn posix_readonly_for_var_is_fatal() {
    let mut shell = Shell::new();
    exec_script(
        "set -o posix\nreadonly i=1\nfor i in a b; do :; done\n",
        &mut shell,
    );
    assert_eq!(shell.pending_fatal_status, Some(127));
}
#[test]
fn default_readonly_for_var_is_not_fatal() {
    let mut shell = Shell::new();
    exec_script("readonly i=1\nfor i in a b; do :; done\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}
#[test]
fn posix_assignment_no_command_is_fatal() {
    let mut shell = Shell::new();
    exec_script("set -o posix\nreadonly x=1\nx=2\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, Some(127));
}
#[test]
fn posix_assignment_before_special_is_fatal() {
    let mut shell = Shell::new();
    exec_script("set -o posix\nreadonly x=1\nx=2 export y\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, Some(127));
}
#[test]
fn posix_assignment_before_regular_is_not_fatal() {
    // before a REGULAR command → abort-continue (deferred), NOT a shell exit.
    let mut shell = Shell::new();
    exec_script("set -o posix\nreadonly x=1\nx=2 true\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}
#[test]
fn default_assignment_no_command_is_not_fatal() {
    let mut shell = Shell::new();
    exec_script("readonly x=1\nx=2\n", &mut shell);
    assert_eq!(shell.pending_fatal_status, None);
}

// ----- Case #1: special-builtin usage / assignment errors are posix-fatal --
fn posix_run(src: &str) -> Option<i32> {
    let mut shell = Shell::new();
    exec_script(&format!("set -o posix\n{src}\n"), &mut shell);
    shell.pending_fatal_status
}
#[test]
fn posix_special_builtin_usage_errors_exit() {
    assert_eq!(posix_run("set -o nosuchopt"), Some(2), "set bad option");
    assert_eq!(posix_run("unset -z"), Some(2), "unset bad option");
    assert_eq!(posix_run("export -z"), Some(2), "export bad option");
    assert_eq!(
        posix_run("export AA[4]=1"),
        Some(1),
        "export bad assignment"
    );
    assert_eq!(
        posix_run("readonly AA[4]=1"),
        Some(1),
        "readonly bad assignment"
    );
    assert_eq!(posix_run("return 2"), Some(2), "return outside function");
    assert_eq!(posix_run("exec -z"), Some(2), "exec bad option");
}
#[test]
fn posix_set_unimplemented_option_does_not_exit() {
    // Valid-in-bash options huck hasn't implemented must NOT exit a posix shell.
    assert_eq!(posix_run("set -o emacs"), None, "set -o emacs");
    assert_eq!(posix_run("set -o vi"), None, "set -o vi");
    assert_eq!(posix_run("set -h"), None, "set -h single-char");
}
#[test]
fn posix_set_invalid_option_name_exits() {
    assert_eq!(
        posix_run("set -o nosuchopt"),
        Some(2),
        "genuinely invalid -o name"
    );
}
#[test]
fn posix_special_builtin_runtime_errors_do_not_exit() {
    assert_eq!(posix_run("shift 99"), None, "shift out of range");
    assert_eq!(posix_run("shift -z"), None, "shift bad option");
    assert_eq!(posix_run("break"), None, "break outside loop");
    assert_eq!(
        posix_run("unset RO; readonly RO=1; unset RO"),
        None,
        "unset readonly var"
    );
    assert_eq!(
        posix_run("eval false"),
        None,
        "eval propagates child status"
    );
    assert_eq!(posix_run("f(){ return 2; }; f"), None, "legit return 2");
    assert_eq!(posix_run("trap x NOSUCHSIG"), None, "trap bad signal");
    assert_eq!(posix_run("export \"AA[4]\""), None, "export bad name no =");
}
#[test]
fn posix_command_builtin_wrappers_strip_fatal() {
    assert_eq!(posix_run("command set -o bad"), None, "command strips");
    assert_eq!(posix_run("builtin set -o bad"), None, "builtin strips");
    assert_eq!(
        posix_run("command export AA[4]=1"),
        None,
        "command strips assignment"
    );
}

#[test]
fn set_o_posix_toggles_shell_option() {
    let mut shell = Shell::new();
    assert!(!shell.shell_options.posix, "posix defaults off");
    exec_script("set -o posix\n", &mut shell);
    assert!(shell.shell_options.posix, "set -o posix turns it on");
    exec_script("set +o posix\n", &mut shell);
    assert!(!shell.shell_options.posix, "set +o posix turns it off");
}

#[test]
fn funcnest_limit_refuses_call_past_depth() {
    let mut shell = Shell::new();
    // FUNCNEST=3 allows depth 1,2,3; the 4th call is refused (rc 1).
    exec_script("FUNCNEST=3\nn=0\nf(){ n=$((n+1)); f; }\nf\n", &mut shell);
    assert_eq!(shell.get("n"), Some("3"), "should stop after depth 3");
    assert_eq!(shell.last_status(), 1, "refused call propagates rc 1");
}

#[test]
fn funcnest_unlimited_allows_bounded_recursion() {
    let mut shell = Shell::new();
    // No FUNCNEST: a bounded 50-deep recursion completes without error.
    exec_script(
        "n=0\nf(){ n=$((n+1)); if (( n >= 50 )); then return 7; fi; f; }\nf\n",
        &mut shell,
    );
    assert_eq!(shell.get("n"), Some("50"));
    assert_eq!(shell.last_status(), 7);
}

#[test]
fn call_function_keeps_arg0_during_body() {
    // bash: `$0` is NOT rebound to the function name on entry — it stays the
    // shell/script invocation name throughout the function body.
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    exec_script("myfunc() { CAPTURED=$0; }\nmyfunc\n", &mut shell);
    assert_eq!(shell.get("CAPTURED"), Some("my-shell"));
}

#[test]
fn call_function_pops_arg0_after_return() {
    let mut shell = Shell::new();
    exec_script("myfunc() { :; }\nmyfunc\n", &mut shell);
    assert!(
        shell.call_stack.is_empty(),
        "call_stack should be empty after function returns, got: {:?}",
        shell.call_stack
    );
}

#[test]
fn function_with_local_does_not_leak_var() {
    let mut shell = Shell::new();
    exec_script("f() { local XYZ_LOCAL_E1=in; }\nf\n", &mut shell);
    assert!(shell.lookup_var("XYZ_LOCAL_E1").is_none());
}

#[test]
fn function_local_restores_outer_var() {
    let mut shell = Shell::new();
    shell.set("XYZ_LOCAL_E2", "outer".to_string());
    exec_script("f() { local XYZ_LOCAL_E2=inner; }\nf\n", &mut shell);
    assert_eq!(shell.lookup_var("XYZ_LOCAL_E2").as_deref(), Some("outer"));
}

#[test]
fn nested_function_calls_have_isolated_locals() {
    let mut shell = Shell::new();
    shell.set("XYZ_LOCAL_E3", "top".to_string());
    let script = "outer() { local XYZ_LOCAL_E3=outer_val; inner; }\n\
                      inner() { local XYZ_LOCAL_E3=inner_val; }\n\
                      outer\n";
    exec_script(script, &mut shell);
    // After both functions return, the outer `top` value is restored.
    assert_eq!(shell.lookup_var("XYZ_LOCAL_E3").as_deref(), Some("top"));
}

#[test]
fn run_background_sequence_sets_last_bg_pid() {
    // Background an external command and check that last_bg_pid is set.
    let mut shell = Shell::new();
    exec_script("/usr/bin/true &\n", &mut shell);
    assert!(
        shell.last_bg_pid.is_some(),
        "last_bg_pid should be set after background command"
    );
    // Reap the child to avoid zombies.
    if let Some(pid) = shell.last_bg_pid {
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(pid, &mut status, libc::WNOHANG);
        }
    }
}

// NOTE: `execute_bg_chain_returns_immediately_status_0` moved to
// `tests/forking_execution_serial.rs` as
// `background_chain_returns_immediately_status_zero` (a background `&&` chain
// forks in-process; unsafe to run concurrently with other tests — issue
// #184).

// ----- v54: readonly enforcement at executor-layer write paths ----------

#[test]
fn top_level_assign_to_readonly_errors() {
    let mut shell = Shell::new();
    shell.set("X", "outer".to_string());
    shell.mark_readonly("X");
    exec_script("X=new\n", &mut shell);
    assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
    assert_eq!(shell.last_status(), 1);
}

#[test]
fn inline_assignment_to_readonly_aborts_command() {
    let mut shell = Shell::new();
    shell.set("X", "outer".to_string());
    shell.mark_readonly("X");
    // Inline `X=new echo hi` — bash aborts the command. Use a
    // builtin (echo) to keep the assertion deterministic.
    exec_script("X=new echo hi\n", &mut shell);
    // X is still its original value (not changed by the failed
    // inline). The echo should NOT have run. Status is 1.
    assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
    assert_eq!(shell.last_status(), 1);
}

#[test]
fn for_loop_iter_var_readonly_aborts_at_first_iter() {
    let mut shell = Shell::new();
    shell.set("X", "outer".to_string());
    shell.mark_readonly("X");
    exec_script("for X in a b c; do echo got=$X; done\n", &mut shell);
    // X unchanged; status 1; body should not have executed.
    assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
    assert_eq!(shell.last_status(), 1);
}

#[test]
fn param_expansion_default_assign_to_readonly_errors() {
    let mut shell = Shell::new();
    shell.set("X", "".to_string());
    shell.mark_readonly("X");
    // `: ${X:=hello}` — colon command + AssignDefault that
    // tries to write hello to readonly X.
    exec_script(": ${X:=hello}\n", &mut shell);
    assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
    assert_eq!(shell.last_status(), 1);
}

#[test]
fn arith_assign_to_readonly_errors() {
    let mut shell = Shell::new();
    shell.set("X", "0".to_string());
    shell.mark_readonly("X");
    // The arith expansion machinery in expand.rs maps any
    // ArithError to "huck: arithmetic: <msg>" + set_last_status(1)
    // with empty substitution; the surrounding command may then
    // overwrite the status (echo returns 0). The load-bearing
    // assertion is that the readonly X was NOT clobbered.
    exec_script("echo $((X=5))\n", &mut shell);
    assert_eq!(shell.lookup_var("X").as_deref(), Some("0"));
}

#[test]
fn local_readonly_in_function_errors() {
    let mut shell = Shell::new();
    shell.set("X", "outer".to_string());
    shell.mark_readonly("X");
    exec_script("f() { local X=inner; }\nf\n", &mut shell);
    // local should have errored; X unchanged.
    assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
    assert_eq!(shell.last_status(), 1);
}

/// Smoke-test for `with_redirect_scope` via `run_redirected`: a brace
/// group redirected to a file writes its output there (not to stdout).
///
/// Cross-process FD-1 race: while this test has FD 1 dup2'd to the
/// target file, `cargo test`'s libtest runner may print sibling test
/// progress lines (`"test foo ... ok\n"`) to the same FD 1 from a
/// peer thread, and those land in our file too. We can't serialize
/// against libtest (it doesn't take our lock) and we can't redirect
/// libtest's writes (they go to the inherited real FD 1). So the
/// assertion verifies the actual claim — that the redirected
/// `echo HI` output is present as an exact line — and tolerates any
/// libtest noise that may have leaked in alongside it. (In real
/// shell use no other thread writes to FD 1 during the redirect
/// window; the noise is a `cargo test` artifact only.)
#[test]
fn compound_stdout_redirect_writes_to_file() {
    let dir = std::env::temp_dir().join(format!("huck_redir_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("out.txt");
    let _ = std::fs::remove_file(&p);

    let mut shell = Shell::new();
    exec_script(&format!("{{ echo HI; }} > {}\n", p.display()), &mut shell);

    let content = std::fs::read_to_string(&p).expect("redirect target file should exist");
    assert!(
        content.lines().any(|l| l == "HI"),
        "redirected `echo HI` should appear as a line in the file, got {content:?}",
    );
    let _ = std::fs::remove_file(&p);
}

#[test]
fn classify_runnability_bare_not_found_is_127() {
    let shell = Shell::new();
    match classify_command_runnability("definitely_no_such_cmd_xyz", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 127);
            assert_eq!(body, "definitely_no_such_cmd_xyz: command not found");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_slash_not_found_is_127_no_such_file() {
    let shell = Shell::new();
    match classify_command_runnability("/no/such/path/xyz", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 127);
            assert_eq!(body, "/no/such/path/xyz: No such file or directory");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_directory_is_126_is_a_directory() {
    let shell = Shell::new();
    match classify_command_runnability("/etc", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 126);
            assert_eq!(body, "/etc: Is a directory");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_non_executable_is_126_permission_denied() {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    let mut p = std::env::temp_dir();
    p.push("huck_classify_noexec_test");
    {
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "#x").unwrap();
    }
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    let shell = Shell::new();
    let ps = p.to_str().unwrap();
    match classify_command_runnability(ps, &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 126);
            assert_eq!(body, format!("{ps}: Permission denied"));
        }
        other => panic!("expected NotRunnable, got {other:?}"),
    }
    let _ = std::fs::remove_file(&p);
}

#[test]
fn classify_runnability_existing_binary_is_runnable() {
    let shell = Shell::new();
    // /bin/sh exists and is executable on the CI target.
    assert!(matches!(
        classify_command_runnability("/bin/sh", &shell),
        StageRunnability::Runnable
    ));
    // and a bare name found on PATH
    assert!(matches!(
        classify_command_runnability("sh", &shell),
        StageRunnability::Runnable
    ));
}

#[test]
fn classify_runnability_bare_non_executable_in_path_is_126() {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    // A non-executable regular file named `foo` in a PATH dir → 126 "Permission
    // denied", reported with the RESOLVED path (bash's first-match-in-PATH order).
    let dir = std::env::temp_dir().join("huck_classify_bare_noexec_dir");
    let _ = std::fs::create_dir_all(&dir);
    let foo = dir.join("foo");
    {
        let mut f = std::fs::File::create(&foo).unwrap();
        writeln!(f, "#x").unwrap();
    }
    std::fs::set_permissions(&foo, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut shell = Shell::new();
    shell.set("PATH", dir.to_str().unwrap().to_string());
    match classify_command_runnability("foo", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 126);
            assert_eq!(body, format!("{}: Permission denied", foo.display()));
        }
        other => panic!("expected NotRunnable, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn classify_runnability_bare_executable_later_in_path_wins() {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    // Non-executable `foo` in the first PATH dir, executable `foo` in the second:
    // the executable wins → Runnable (bash runs it), so no 126.
    let d1 = std::env::temp_dir().join("huck_classify_wins_d1");
    let d2 = std::env::temp_dir().join("huck_classify_wins_d2");
    let _ = std::fs::create_dir_all(&d1);
    let _ = std::fs::create_dir_all(&d2);
    {
        let mut f = std::fs::File::create(d1.join("foo")).unwrap();
        writeln!(f, "#x").unwrap();
    }
    std::fs::set_permissions(&d1.join("foo"), std::fs::Permissions::from_mode(0o644)).unwrap();
    {
        let mut f = std::fs::File::create(d2.join("foo")).unwrap();
        writeln!(f, "#!/bin/sh\n").unwrap();
    }
    std::fs::set_permissions(&d2.join("foo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut shell = Shell::new();
    shell.set("PATH", format!("{}:{}", d1.display(), d2.display()));
    assert!(matches!(
        classify_command_runnability("foo", &shell),
        StageRunnability::Runnable
    ));
    let _ = std::fs::remove_dir_all(&d1);
    let _ = std::fs::remove_dir_all(&d2);
}

#[test]
fn classify_runnability_bare_directory_in_path_is_not_found() {
    // A DIRECTORY named `foo` in PATH is not a command match → 127, not 126.
    let dir = std::env::temp_dir().join("huck_classify_dirmatch");
    let _ = std::fs::create_dir_all(dir.join("foo"));
    let mut shell = Shell::new();
    shell.set("PATH", dir.to_str().unwrap().to_string());
    match classify_command_runnability("foo", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 127);
            assert_eq!(body, "foo: command not found");
        }
        other => panic!("expected NotRunnable, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── #80: render_job_command AST→source deparser (jobs/fg/bg display) ─────────

/// Parse a snippet to a `Sequence` for deparser tests.
fn parse_job_seq(src: &str) -> Sequence {
    crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse")
    .expect("seq")
}

/// Deparse the first command of a snippet.
fn render_job_cmd(src: &str) -> String {
    render_job_command(&parse_job_seq(src).first)
}

#[test]
fn render_job_command_simple_with_args() {
    assert_eq!(render_job_cmd("sleep 0.3 aa bb"), "sleep 0.3 aa bb");
}

#[test]
fn render_job_command_collapses_whitespace() {
    // The lexer already drops inter-word whitespace, so the deparse is normalized.
    assert_eq!(render_job_cmd("sleep   0.3    aa"), "sleep 0.3 aa");
}

#[test]
fn render_job_command_pipeline() {
    assert_eq!(render_job_cmd("sleep 0.3 | cat"), "sleep 0.3 | cat");
    assert_eq!(
        render_job_cmd("sleep 0.3 | cat | cat"),
        "sleep 0.3 | cat | cat"
    );
}

#[test]
fn render_job_command_andor_sequence() {
    // The whole and-or group is rendered via render_job_sequence.
    assert_eq!(
        render_job_sequence(&parse_job_seq("sleep 0.3 && echo hi")),
        "sleep 0.3 && echo hi"
    );
    assert_eq!(render_job_sequence(&parse_job_seq("a || b")), "a || b");
}

#[test]
fn render_job_command_redirect_has_space() {
    // File redirects render with a space before the target (bash-normalized).
    assert_eq!(
        render_job_cmd("sleep 0.3 >/dev/null"),
        "sleep 0.3 > /dev/null"
    );
    assert_eq!(
        render_job_cmd("sleep 0.3 2>/dev/null"),
        "sleep 0.3 2> /dev/null"
    );
    // Dup redirects glue the source with no space; default fd made explicit.
    assert_eq!(render_job_cmd("sleep 0.3 2>&1"), "sleep 0.3 2>&1");
    assert_eq!(render_job_cmd("sleep 0.3 >&2"), "sleep 0.3 1>&2");
}

#[test]
fn render_job_command_double_quoted_arg() {
    assert_eq!(render_job_cmd("sleep 0.3 \"a b\""), "sleep 0.3 \"a b\"");
}

#[test]
fn render_job_command_variable_unexpanded() {
    // Rendered from the AST word, so `$x` is shown pre-expansion.
    assert_eq!(render_job_cmd("sleep $x"), "sleep $x");
}

#[test]
fn render_job_command_inline_assignments() {
    assert_eq!(render_job_cmd("A=1 B=2 sleep 0.3"), "A=1 B=2 sleep 0.3");
}

#[test]
fn render_job_command_subshell_and_brace_group() {
    assert_eq!(
        render_job_cmd("( sleep 0.3; echo hi )"),
        "( sleep 0.3; echo hi )"
    );
    assert_eq!(
        render_job_cmd("{ sleep 0.3; echo hi; }"),
        "{ sleep 0.3; echo hi; }"
    );
}
