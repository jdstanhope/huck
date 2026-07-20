use super::*;

#[test]
fn declare_scalar_quote_matches_bash_listing() {
    // bash bare-declare / set -x style minimal quoting (verified vs bash 5.x)
    assert_eq!(declare_scalar_quote("hello"), "hello");
    assert_eq!(declare_scalar_quote(""), ""); // empty -> bare (name=)
    assert_eq!(declare_scalar_quote("a b"), "'a b'");
    assert_eq!(declare_scalar_quote("x;y"), "'x;y'");
    assert_eq!(declare_scalar_quote("gl*ob"), "'gl*ob'");
    assert_eq!(declare_scalar_quote("d$ollar"), "'d$ollar'");
    assert_eq!(declare_scalar_quote("bang!x"), "'bang!x'");
    assert_eq!(declare_scalar_quote("lt<gt>"), "'lt<gt>'");
    assert_eq!(declare_scalar_quote("br[ack]"), "'br[ack]'");
    assert_eq!(declare_scalar_quote("back`tick"), "'back`tick'");
    assert_eq!(declare_scalar_quote("qu'ote"), "'qu'\\''ote'");
    // not metacharacters in this context -> stay bare
    assert_eq!(declare_scalar_quote("ti~lde"), "ti~lde");
    assert_eq!(declare_scalar_quote("eq=ual"), "eq=ual");
    assert_eq!(declare_scalar_quote("hash#x"), "hash#x");
    // control char -> ANSI-C
    assert_eq!(declare_scalar_quote("ta\tb"), "$'ta\\tb'");
}

#[test]
fn format_declare_bare_line_scalar_and_array() {
    use crate::shell_state::{VarValue, Variable};
    // scalar needing quotes -> single-quoted
    let zs = Variable::scalar("a b".to_string());
    assert_eq!(format_declare_bare_line("zs", &zs), "zs='a b'");
    // bare scalar -> unquoted
    let zp = Variable::scalar("plain".to_string());
    assert_eq!(format_declare_bare_line("zp", &zp), "zp=plain");
    // indexed array -> name=([0]="p" [1]="q r") (matches declare -p minus prefix)
    let mut m = std::collections::BTreeMap::new();
    m.insert(0usize, "p".to_string());
    m.insert(1usize, "q r".to_string());
    let za = Variable {
        value: VarValue::Indexed(m),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    assert_eq!(
        format_declare_bare_line("za", &za),
        r#"za=([0]="p" [1]="q r")"#
    );
}

#[test]
fn assoc_key_bareword_for_identifier() {
    use crate::shell_state::{VarValue, Variable};
    let var = Variable {
        value: VarValue::Associative(vec![("foo".into(), "v".into())]),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    let out = render_declare_value_part(&var);
    assert_eq!(out, r#"=([foo]="v" )"#);
}

#[test]
fn declare_p_scalar_control_uses_ansi_c() {
    use crate::shell_state::Variable;
    // value is `i` + a real newline; bash renders it as $'i\n'
    let v = Variable::scalar("i\n".to_string());
    assert_eq!(render_declare_value_part(&v), "=$'i\\n'");
    let t = Variable::scalar("a\tb".to_string());
    assert_eq!(render_declare_value_part(&t), "=$'a\\tb'");
    // 0x01 SOH -> 3-digit octal
    let c = Variable::scalar("a\u{01}b".to_string());
    assert_eq!(render_declare_value_part(&c), "=$'a\\001b'");
}

#[test]
fn declare_p_scalar_plain_unchanged() {
    use crate::shell_state::Variable;
    assert_eq!(
        render_declare_value_part(&Variable::scalar("hello".to_string())),
        "=\"hello\""
    );
    // `$` and `"` stay in the double-quoted form (no control char -> no $'…')
    assert_eq!(
        render_declare_value_part(&Variable::scalar("a$b\"c".to_string())),
        "=\"a\\$b\\\"c\""
    );
}

#[test]
fn declare_p_indexed_control_uses_ansi_c() {
    use crate::shell_state::{VarValue, Variable};
    let mut m = std::collections::BTreeMap::new();
    m.insert(0usize, "x".to_string());
    m.insert(1usize, "i\n".to_string());
    let a = Variable {
        value: VarValue::Indexed(m),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    assert_eq!(render_declare_value_part(&a), "=([0]=\"x\" [1]=$'i\\n')");
}

#[test]
fn declare_p_assoc_control_uses_ansi_c() {
    use crate::shell_state::{VarValue, Variable};
    let var = Variable {
        value: VarValue::Associative(vec![("k".into(), "a\tb".into())]),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    assert_eq!(render_declare_value_part(&var), "=([k]=$'a\\tb' )");
}

#[test]
fn assoc_key_quoted_for_metachar() {
    use crate::shell_state::{VarValue, Variable};
    let var = Variable {
        value: VarValue::Associative(vec![("a b".into(), "v".into())]),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    let out = render_declare_value_part(&var);
    assert_eq!(out, r#"=(["a b"]="v" )"#);
}

#[test]
fn indexed_has_no_trailing_space() {
    use crate::shell_state::{VarValue, Variable};
    use std::collections::BTreeMap;
    let mut m = BTreeMap::new();
    m.insert(0usize, "x".to_string());
    m.insert(1usize, "y".to_string());
    let var = Variable {
        value: VarValue::Indexed(m),
        exported: false,
        readonly: false,
        integer: false,
        case_fold: None,
        nameref: false,
    };
    let out = render_declare_value_part(&var);
    assert_eq!(out, r#"=([0]="x" [1]="y")"#);
}

#[test]
fn bare_declare_lists_name_value_and_functions() {
    let mut shell = crate::shell_state::Shell::new();
    // Set a scalar and define a function via the normal command path.
    shell.set("zsv", "hello".to_string());
    let _ = crate::shell::process_line("zf(){ echo hi; }", &mut shell, false);
    let mut buf: Vec<u8> = Vec::new();
    let _ = run_declaration_builtin("declare", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    let s = String::from_utf8(buf).unwrap();
    assert!(
        s.contains("zsv=hello"),
        "bare declare should list zsv=hello: {s}"
    );
    assert!(
        !s.contains("declare -- zsv"),
        "bare declare must not use the -p form: {s}"
    );
    assert!(
        s.contains("zf ()"),
        "bare declare should list function zf: {s}"
    );
}

#[test]
fn printf_q_quoting() {
    assert_eq!(printf_q("plain"), "plain");
    assert_eq!(printf_q("a b"), "a\\ b");
    assert_eq!(printf_q("c'd"), "c\\'d");
    assert_eq!(printf_q("a$b"), "a\\$b");
    assert_eq!(printf_q("x\"y"), "x\\\"y");
    assert_eq!(printf_q("*"), "\\*");
    assert_eq!(printf_q(""), "''");
    assert_eq!(printf_q("p/q-r.s"), "p/q-r.s"); // /,-,. not escaped
    assert_eq!(printf_q("a\tb"), "$'a\\tb'"); // control -> $'...'
    assert_eq!(printf_q("ünï"), "ünï"); // UTF-8 as-is
    assert_eq!(printf_q("~a"), "\\~a"); // leading ~ escaped
    assert_eq!(printf_q("a~"), "a~"); // trailing ~ not escaped
    assert_eq!(printf_q("b~c"), "b~c"); // mid ~ not escaped
    assert_eq!(printf_q("#a"), "\\#a"); // leading # escaped
    assert_eq!(printf_q("a#"), "a#"); // trailing # not escaped
    assert_eq!(printf_q("a$b"), "a\\$b"); // $ special at any position
}

#[test]
fn seto_option_names_includes_errexit_in_table_order() {
    let names: Vec<&str> = seto_option_names().collect();
    assert!(names.contains(&"errexit"));
    assert_eq!(names.len(), 27);
    assert_eq!(names[0], "allexport"); // table order
}
#[test]
fn signal_names_are_sig_prefixed_and_exclude_pseudo() {
    let names = signal_names();
    assert!(names.contains(&"SIGINT".to_string()));
    assert!(names.iter().all(|n| n.starts_with("SIG")));
    assert!(!names.iter().any(|n| n.contains("EXIT")));
}
#[test]
fn help_topic_names_nonempty() {
    assert!(help_topic_names().count() >= 40);
}

#[test]
fn builtin_active_reflects_disabled_set() {
    let mut sh = crate::shell_state::Shell::new();
    assert!(super::builtin_active("test", &sh)); // enabled by default
    sh.disabled_builtins.insert("test".to_string());
    assert!(!super::builtin_active("test", &sh)); // now disabled
    assert!(super::is_builtin("test")); // still a KNOWN builtin
    assert!(!super::builtin_active("not_a_builtin", &sh));
}

#[test]
fn is_builtin_recognizes_builtins() {
    assert!(is_builtin("cd"));
    assert!(is_builtin("exit"));
    assert!(is_builtin("pwd"));
    assert!(is_builtin("echo"));
    assert!(is_builtin("export"));
    assert!(is_builtin("unset"));
    assert!(!is_builtin("ls"));
}

#[test]
fn exit_with_no_args() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&[], &mut std::io::stderr(), &shell),
        ExecOutcome::Exit(0)
    ));
}

#[test]
fn exit_with_code() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&["3".to_string()], &mut std::io::stderr(), &shell),
        ExecOutcome::Exit(3)
    ));
}

#[test]
fn exit_with_bad_code_continues() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&["abc".to_string()], &mut std::io::stderr(), &shell),
        ExecOutcome::Continue(_)
    ));
}

#[test]
fn exit_masks_value_greater_than_255() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&["300".to_string()], &mut std::io::stderr(), &shell),
        ExecOutcome::Exit(44)
    ));
}

#[test]
fn exit_masks_negative_value() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&["-1".to_string()], &mut std::io::stderr(), &shell),
        ExecOutcome::Exit(255)
    ));
}

#[test]
fn exit_masks_exact_256_to_zero() {
    let shell = crate::shell_state::Shell::new();
    assert!(matches!(
        builtin_exit(&["256".to_string()], &mut std::io::stderr(), &shell),
        ExecOutcome::Exit(0)
    ));
}

#[test]
fn echo_writes_args_joined_by_spaces() {
    let mut out: Vec<u8> = Vec::new();
    let outcome = builtin_echo(
        &["hello".to_string(), "world".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(out, b"hello world\n");
}

#[test]
fn echo_with_no_args_writes_a_blank_line() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &[],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"\n");
}

#[test]
fn echo_n_suppresses_trailing_newline() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-n".to_string(), "hello".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"hello");
}

#[test]
fn echo_n_alone_writes_nothing() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-n".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"");
}

#[test]
fn echo_e_processes_basic_escapes() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-e".to_string(), r"a\tb\nc".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"a\tb\nc\n");
}

#[test]
fn echo_capital_e_keeps_backslashes_literal() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-E".to_string(), r"a\tb".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"a\\tb\n");
}

#[test]
fn echo_default_keeps_backslashes_literal() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &[r"a\tb".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"a\\tb\n");
}

#[test]
fn echo_combined_ne_flag() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-ne".to_string(), r"a\tb".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"a\tb");
}

#[test]
fn echo_e_then_capital_e_disables_escapes() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-eE".to_string(), r"a\tb".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"a\\tb\n");
}

#[test]
fn echo_non_flag_arg_stops_flag_parsing() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &[
            "-n".to_string(),
            "foo".to_string(),
            "-n".to_string(),
            "bar".to_string(),
        ],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"foo -n bar");
}

#[test]
fn echo_unknown_flag_is_literal() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-x".to_string(), "foo".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"-x foo\n");
}

#[test]
fn echo_single_dash_is_literal() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"-\n");
}

#[test]
fn echo_double_dash_is_literal() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["--".to_string(), "foo".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"-- foo\n");
}

#[test]
fn echo_e_c_escape_terminates_output() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-e".to_string(), r"abc\cdef".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"abc");
}

#[test]
fn echo_e_octal_escape() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-e".to_string(), r"\0101".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"A\n");
}

#[test]
fn echo_e_hex_escape() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-e".to_string(), r"\x41".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"A\n");
}

#[test]
fn echo_e_unknown_escape_keeps_backslash() {
    let mut out: Vec<u8> = Vec::new();
    builtin_echo(
        &["-e".to_string(), r"\z".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &crate::shell_state::Shell::new(),
    );
    assert_eq!(out, b"\\z\n");
}

struct RecordingWriter {
    calls: Vec<Vec<u8>>,
}
impl std::io::Write for RecordingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.calls.push(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn echo_calls(args: &[&str]) -> Vec<Vec<u8>> {
    let shell = Shell::new();
    // `args[0]` is the pseudo-argv0 ("echo"), for readability at call
    // sites; `builtin_echo` itself takes only the arguments after the
    // command name (see `run_builtin`'s `"echo" => builtin_echo(args,
    // ...)` dispatch, where `args` is already name-stripped).
    let owned: Vec<String> = args[1..].iter().map(|s| s.to_string()).collect();
    let mut rec = RecordingWriter { calls: Vec::new() };
    let mut sink: Vec<u8> = Vec::new();
    let _ = builtin_echo(&owned, &mut rec, &mut sink, &shell);
    rec.calls
}

#[test]
fn echo_writes_line_in_one_call() {
    // The whole line (content + newline) must arrive in ONE write() call, so
    // concurrent backgrounded echoes can't interleave between them (#208).
    assert_eq!(echo_calls(&["echo", "hi"]), vec![b"hi\n".to_vec()]);
}

#[test]
fn echo_n_writes_content_only_one_call() {
    assert_eq!(echo_calls(&["echo", "-n", "hi"]), vec![b"hi".to_vec()]);
}

#[test]
fn echo_no_args_writes_just_newline_one_call() {
    assert_eq!(echo_calls(&["echo"]), vec![b"\n".to_vec()]);
}

#[test]
fn echo_n_empty_issues_no_write() {
    // v308 zero-byte rule: empty output must not issue a write() at all.
    assert_eq!(echo_calls(&["echo", "-n", ""]), Vec::<Vec<u8>>::new());
}

#[test]
fn pwd_writes_the_current_directory() {
    let mut out: Vec<u8> = Vec::new();
    let mut shell = Shell::new();
    let outcome = builtin_pwd(&[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let written = String::from_utf8(out).unwrap();
    // With no $PWD set, logical mode falls back to getcwd.
    let expected = env::current_dir().unwrap();
    assert_eq!(written.trim_end(), expected.to_str().unwrap());
}

fn dp(s: &str) -> DeclArg {
    DeclArg::Plain(s.to_string())
}

#[test]
fn export_nf_unexports_function() {
    let mut shell = Shell::new();
    let _ = crate::shell::process_line("uf(){ echo hi; }", &mut shell, false);
    shell.mark_function_exported("uf");
    assert!(shell.is_function_exported("uf"));
    let mut out = Vec::new();
    // export -nf uf  -> remove the export mark
    let oc = builtin_export_decl(
        &[dp("-n"), dp("-f"), dp("uf")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(0)), "{oc:?}");
    assert!(
        !shell.is_function_exported("uf"),
        "export -nf must un-export the function"
    );
}

#[test]
fn declare_fx_marks_via_runtime_path() {
    let mut shell = Shell::new();
    let _ = crate::shell::process_line("dfn(){ echo hi; }", &mut shell, false);
    assert!(!shell.is_function_exported("dfn"));
    // `declare -fx NAME` must mark it exported (runtime declaration path).
    let _ = crate::shell::process_line("declare -fx dfn", &mut shell, false);
    assert!(
        shell.is_function_exported("dfn"),
        "declare -fx did not mark via the runtime path"
    );
}

#[test]
fn declare_fx_no_names_lists_via_runtime_path() {
    let mut shell = Shell::new();
    let _ = crate::shell::process_line("dfn2(){ echo hi; }", &mut shell, false);
    shell.mark_function_exported("dfn2");
    // capture stdout of `declare -fx`: route through builtin_declare_decl directly.
    let mut out = Vec::new();
    let oc = builtin_declare_decl(&[dp("-fx")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)), "{oc:?}");
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("dfn2 ()"), "{s}");
    assert!(s.contains("declare -fx dfn2"), "{s}");
}

#[test]
fn export_p_lists_in_declare_x_format() {
    let mut shell = Shell::new();
    shell.export_set("EXP_A", "1".to_string());
    shell.export_set("EXP_B", "two".to_string());
    let mut out = Vec::new();
    let oc = builtin_export_decl(&[dp("-p")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("declare -x EXP_A=\"1\""), "{s}");
    assert!(s.contains("declare -x EXP_B=\"two\""), "{s}");
    assert!(
        !s.contains("export EXP_A=1"),
        "old format must be gone: {s}"
    );
}

#[test]
fn bare_export_uses_declare_x_format() {
    let mut shell = Shell::new();
    shell.export_set("EXP_C", "z".to_string());
    let mut out = Vec::new();
    let _ = builtin_export_decl(&[], &mut out, &mut std::io::stderr(), &mut shell);
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("declare -x EXP_C=\"z\""), "{s}");
}

#[test]
fn export_n_unexports_keeps_value() {
    let mut shell = Shell::new();
    shell.export_set("EXP_D", "keep".to_string());
    assert!(shell.is_exported("EXP_D"));
    let mut out = Vec::new();
    let oc = builtin_export_decl(
        &[dp("-n"), dp("EXP_D")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.is_exported("EXP_D"), "must be unexported");
    assert_eq!(shell.get("EXP_D"), Some("keep"), "value kept");
}

#[test]
fn export_n_with_assignment_sets_then_unexports() {
    let mut shell = Shell::new();
    shell.export_set("EXP_E", "1".to_string());
    let mut out = Vec::new();
    let _ = builtin_export_decl(
        &[dp("-n"), dp("EXP_E=2")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(!shell.is_exported("EXP_E"));
    assert_eq!(shell.get("EXP_E"), Some("2"));
}

#[test]
fn export_n_unset_name_is_noop() {
    let mut shell = Shell::new();
    let mut out = Vec::new();
    let oc = builtin_export_decl(
        &[dp("-n"), dp("NOPE_X")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.is_exported("NOPE_X"));
}

#[test]
fn export_invalid_flag_rc2() {
    let mut shell = Shell::new();
    let mut out = Vec::new();
    let oc = builtin_export_decl(&[dp("-z")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)), "{oc:?}");
}

#[test]
fn export_p_with_operand_exports_it_no_listing() {
    let mut shell = Shell::new();
    shell.set("EXP_F", "v".to_string());
    assert!(!shell.is_exported("EXP_F"));
    let mut out = Vec::new();
    let oc = builtin_export_decl(
        &[dp("-p"), dp("EXP_F")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(
        shell.is_exported("EXP_F"),
        "operand with -p should be exported (bash)"
    );
    assert!(
        String::from_utf8(out).unwrap().is_empty(),
        "no listing when operands present"
    );
}

#[test]
fn export_f_does_not_create_variable() {
    let mut shell = Shell::new();
    let mut out = Vec::new();
    // `export -f somefunc` for a nonexistent function: rc 1, no variable.
    let oc = builtin_export_decl(
        &[dp("-f"), dp("somefunc")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(
        shell.get("somefunc").is_none(),
        "must NOT create a variable"
    );
    assert!(!shell.is_exported("somefunc"));
}

#[test]
fn export_f_marks_existing_function() {
    let mut shell = Shell::new();
    let _ = crate::shell::process_line("myfn(){ echo hi; }", &mut shell, false);
    let mut out = Vec::new();
    let oc = builtin_export_decl(
        &[dp("-f"), dp("myfn")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_function_exported("myfn"));
}

#[test]
fn export_f_not_a_function_rc1() {
    let mut shell = Shell::new();
    let mut out = Vec::new();
    let oc = builtin_export_decl(
        &[dp("-f"), dp("nope")],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(oc, ExecOutcome::Continue(1)), "{oc:?}");
    assert!(!shell.is_function_exported("nope"));
}

#[test]
fn export_f_no_operands_lists_functions() {
    let mut shell = Shell::new();
    let _ = crate::shell::process_line("af(){ echo hi; }", &mut shell, false);
    shell.mark_function_exported("af");
    let mut out = Vec::new();
    let oc = builtin_export_decl(&[dp("-f")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("af ()"), "{s}");
    assert!(s.contains("declare -fx af"), "{s}");
}

#[test]
fn export_a_bare_no_listing() {
    let mut shell = Shell::new();
    shell.export_set("EXP_HIDE", "1".to_string());
    let mut out = Vec::new();
    let oc = builtin_export_decl(&[dp("-a")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(
        String::from_utf8(out).unwrap().is_empty(),
        "export -a must NOT list"
    );
}

#[test]
fn export_f_bare_no_listing() {
    let mut shell = Shell::new();
    shell.export_set("EXP_HIDE2", "1".to_string());
    let mut out = Vec::new();
    let oc = builtin_export_decl(&[dp("-f")], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(
        String::from_utf8(out).unwrap().is_empty(),
        "export -f must NOT list vars"
    );
}

#[test]
fn unset_removes_variable() {
    let mut shell = Shell::new();
    shell.set("HUCK_RM", "v".to_string());
    let outcome = builtin_unset(&["HUCK_RM".to_string()], &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.get("HUCK_RM"), None);
}

#[test]
fn unset_invalid_name_is_error() {
    let mut shell = Shell::new();
    let outcome = builtin_unset(&["1BAD".to_string()], &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn unset_unknown_name_is_silent_ok() {
    let mut shell = Shell::new();
    let outcome = builtin_unset(
        &["NEVER_SET_HUCK_XYZ".to_string()],
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn jobs_with_empty_table_prints_nothing_and_returns_zero() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = builtin_jobs(&[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(out.is_empty());
}

#[test]
fn jobs_lists_synthetic_done_entry() {
    let mut shell = Shell::new();
    let _ = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
    let mut out: Vec<u8> = Vec::new();
    let outcome = builtin_jobs(&[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("[1]"));
    assert!(s.contains("Done"));
    assert!(s.contains("echo hi"));
}

#[test]
fn jobs_lists_stopped_without_ampersand_suffix() {
    let mut shell = Shell::new();
    shell.jobs.add(100, vec![100], "sleep 100".to_string());
    shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("jobs", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("Stopped"), "got: {out:?}");
    assert!(
        !out.trim_end().ends_with('&'),
        "Stopped line must NOT end with &; got: {out:?}"
    );
}

#[test]
fn jobs_l_includes_pid_for_single_stage() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "sleep 30".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("1234"), "expected pid 1234 in: {out:?}");
    assert!(out.contains("[1]"), "expected job number in: {out:?}");
}

#[test]
fn jobs_l_multistage_shows_all_pids() {
    let mut shell = Shell::new();
    shell
        .jobs
        .add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("1234"), "missing 1234 in: {out:?}");
    assert!(out.contains("1235"), "missing 1235 in: {out:?}");
    assert!(out.contains("1236"), "missing 1236 in: {out:?}");
    let line_count = out.lines().count();
    assert!(
        line_count >= 3,
        "expected >=3 lines, got {line_count}: {out:?}"
    );
}

#[test]
fn jobs_p_prints_pgids_only() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "a".to_string());
    shell.jobs.add(2345, vec![2345], "b".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-p".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines, got {lines:?}");
    for l in &lines {
        assert!(
            l.parse::<i32>().is_ok(),
            "expected each line to be an int, got {l:?}"
        );
    }
}

#[test]
fn jobs_r_filters_running() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
    shell.jobs.add_synthetic_done("done_cmd".to_string(), 0); // %2 Done
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-r".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("running_cmd"), "missing running_cmd: {out:?}");
    assert!(
        !out.contains("done_cmd"),
        "should not contain done_cmd: {out:?}"
    );
}

#[test]
fn jobs_s_filters_stopped() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
    shell.jobs.add(2345, vec![2345], "stopped_cmd".to_string()); // %2 then forced Stopped
    shell.jobs.jobs_mut()[1].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-s".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("stopped_cmd"), "missing stopped_cmd: {out:?}");
    assert!(
        !out.contains("running_cmd"),
        "should not contain running_cmd: {out:?}"
    );
}

#[test]
fn jobs_n_filters_notified_false_and_marks() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "a".to_string()); // notified=false default
    shell.jobs.add(2345, vec![2345], "b".to_string()); // notified=false default
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-n".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("[1]"), "first call should show [1]: {out:?}");
    assert!(out.contains("[2]"), "first call should show [2]: {out:?}");

    // Second call: both jobs are now marked notified -> empty output.
    let mut buf2: Vec<u8> = Vec::new();
    let outcome2 = run_builtin(
        "jobs",
        &["-n".to_string()],
        &mut buf2,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome2, ExecOutcome::Continue(0)));
    let out2 = String::from_utf8(buf2).unwrap();
    assert!(out2.is_empty(), "second call should be empty: {out2:?}");
}

#[test]
fn jobs_positional_spec_filters_to_target() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "first_cmd".to_string()); // %1
    shell.jobs.add(2345, vec![2345], "second_cmd".to_string()); // %2
    shell.jobs.add(3456, vec![3456], "third_cmd".to_string()); // %3
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["%2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("second_cmd"), "missing second_cmd: {out:?}");
    assert!(
        !out.contains("first_cmd"),
        "should not contain first_cmd: {out:?}"
    );
    assert!(
        !out.contains("third_cmd"),
        "should not contain third_cmd: {out:?}"
    );
}

#[test]
fn jobs_invalid_flag_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-x".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn jobs_p_overrides_l() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "sleep".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "jobs",
        &["-lp".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    // -p output is just digits + newline, no [N] prefix.
    assert!(!out.contains("[1]"), "expected -p override, got: {out:?}");
    assert_eq!(out.trim(), "1234");
}

#[test]
fn wait_with_no_jobs_returns_zero_immediately() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = builtin_wait(&[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn is_builtin_recognizes_jobs_and_wait() {
    assert!(is_builtin("jobs"));
    assert!(is_builtin("wait"));
}

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

#[test]
fn builtin_test_true_expression() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-n".to_string(), "x".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn builtin_test_false_expression() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-z".to_string(), "x".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn builtin_test_usage_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["3".to_string(), "-eq".to_string(), "abc".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn builtin_bracket_strips_trailing_bracket() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-n".to_string(), "x".to_string(), "]".to_string()];
    let outcome = run_builtin("[", &args, &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn builtin_bracket_missing_close_is_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-n".to_string(), "x".to_string()];
    let outcome = run_builtin("[", &args, &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn builtin_bracket_empty_is_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("[", &[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn builtin_break_returns_loop_break() {
    let mut shell = Shell::new();
    shell.loop_depth = 1;
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("break", &[], &mut out, &mut std::io::stderr(), &mut shell);
    assert_eq!(outcome, ExecOutcome::LoopBreak(1, 0));
}

#[test]
fn builtin_continue_returns_loop_continue() {
    let mut shell = Shell::new();
    shell.loop_depth = 1;
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "continue",
        &[],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert_eq!(outcome, ExecOutcome::LoopContinue(1));
}

#[test]
fn builtin_return_with_arg_returns_function_return() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin(
            "return",
            &["7".to_string()],
            &mut out,
            &mut std::io::stderr(),
            &mut shell
        ),
        ExecOutcome::FunctionReturn(7)
    );
}

#[test]
fn builtin_return_no_arg_returns_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(42);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin("return", &[], &mut out, &mut std::io::stderr(), &mut shell),
        ExecOutcome::FunctionReturn(42)
    );
}

#[test]
fn builtin_return_invalid_arg_falls_back_to_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(13);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin(
            "return",
            &["not-a-num".to_string()],
            &mut out,
            &mut std::io::stderr(),
            &mut shell
        ),
        ExecOutcome::FunctionReturn(13)
    );
}

#[test]
fn is_builtin_trap() {
    assert!(is_builtin("trap"));
}

#[test]
fn is_special_builtin_trap() {
    assert!(is_special_builtin("trap"));
}

#[test]
fn trap_exit_action_signal_registers() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo bye".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
}

#[test]
fn trap_empty_action_ignores_signal() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(
        shell.traps.get(&crate::traps::TrapSignal::Exit),
        Some(&None), // None = ignore
    );
}

#[test]
fn trap_dash_resets_signal() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // Install first.
    let _ = run_builtin(
        "trap",
        &["echo bye".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Then reset.
    let outcome = run_builtin(
        "trap",
        &["-".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(!shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
}

#[test]
fn trap_p_prints_active_traps_in_re_readable_form() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // Register a trap.
    let _ = run_builtin(
        "trap",
        &["echo bye".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Clear the buffer (the install printed nothing, but be defensive).
    buf.clear();
    // List.
    let outcome = run_builtin(
        "trap",
        &["-p".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.contains("trap -- 'echo bye' EXIT"),
        "expected trap -p to print 'trap -- echo bye EXIT', got: {out}"
    );
}

#[test]
fn trap_no_args_same_as_dash_p() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let _ = run_builtin(
        "trap",
        &["echo bye".to_string(), "EXIT".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    buf.clear();
    let outcome = run_builtin("trap", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("trap -- 'echo bye' EXIT"));
}

#[test]
fn trap_l_lists_signals() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["-l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.contains("2) SIGINT"), "stdout: {out}");
    assert!(out.contains("15) SIGTERM"), "stdout: {out}");
}

#[test]
fn trap_unknown_signal_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo bye".to_string(), "NOPE".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn trap_kill_signal_accepted_silently() {
    // bash accepts `trap … KILL` (rc 0, no error) and stores the
    // disposition; it just never fires (OS can't catch SIGKILL).
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo nope".to_string(), "KILL".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(
        buf.is_empty(),
        "no error output expected, got: {:?}",
        String::from_utf8_lossy(&buf)
    );
    assert!(
        shell
            .traps
            .contains_key(&crate::traps::TrapSignal::Real(libc::SIGKILL))
    );
}

#[test]
fn trap_no_signals_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo bye".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}
