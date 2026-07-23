use super::*;

#[test]
fn posix_fatal_is_gated_on_posix_and_noninteractive() {
    let mut sh = Shell::new();
    sh.is_interactive = false;
    // default mode → no-op
    sh.posix_fatal(127);
    assert_eq!(sh.pending_fatal_status, None);
    // posix + non-interactive → sets
    sh.shell_options.posix = true;
    sh.posix_fatal(127);
    assert_eq!(sh.pending_fatal_status, Some(127));
    // posix + interactive → no-op (clear first)
    sh.pending_fatal_status = None;
    sh.is_interactive = true;
    sh.posix_fatal(2);
    assert_eq!(sh.pending_fatal_status, None);
}

#[test]
fn funcname_assignment_is_silently_discarded() {
    let mut sh = Shell::new();
    // `set` path (used by `for`, internal writers).
    sh.set("FUNCNAME", "7".to_string());
    assert_eq!(
        sh.lookup_var("FUNCNAME"),
        None,
        "set must not write FUNCNAME"
    );
    // `assign` path (used by `FOO=v`, inline, declare, read via try_set).
    let _ = sh.try_set("FUNCNAME", "9".to_string());
    assert_eq!(
        sh.lookup_var("FUNCNAME"),
        None,
        "assign must not write FUNCNAME"
    );
}

#[test]
fn funcnest_limit_parses_positive_else_none() {
    let mut sh = Shell::new();
    assert_eq!(sh.funcnest_limit(), None); // unset
    sh.set("FUNCNEST", "0".to_string());
    assert_eq!(sh.funcnest_limit(), None); // 0 = unlimited
    sh.set("FUNCNEST", "-3".to_string());
    assert_eq!(sh.funcnest_limit(), None); // negative = unlimited
    sh.set("FUNCNEST", "abc".to_string());
    assert_eq!(sh.funcnest_limit(), None); // non-numeric = unlimited
    sh.set("FUNCNEST", "5".to_string());
    assert_eq!(sh.funcnest_limit(), Some(5));
    sh.set("FUNCNEST", " 5 ".to_string());
    assert_eq!(sh.funcnest_limit(), Some(5)); // trimmed
}

#[test]
fn non_protected_var_still_writes() {
    let mut sh = Shell::new();
    sh.set("FOO", "x".to_string());
    assert_eq!(sh.lookup_var("FOO"), Some("x".to_string()));
    let _ = sh.try_set("BAR", "y".to_string());
    assert_eq!(sh.lookup_var("BAR"), Some("y".to_string()));
}

#[test]
fn glob_opts_reads_globstar_shopt() {
    let mut sh = Shell::new();
    assert!(!sh.glob_opts().globstar, "globstar off by default");
    crate::shell::process_line("shopt -s globstar", &mut sh, false);
    assert!(sh.glob_opts().globstar, "globstar on after shopt -s");
}

#[test]
fn bind_p_shows_defaults_user_override_and_unbind() {
    let mut sh = Shell::new();
    // default present
    let p = sh.active_bind_lines();
    assert!(
        p.iter().any(|l| l == "\"\\C-a\": beginning-of-line"),
        "missing default C-a: {p:?}"
    );
    assert!(
        p.iter().any(|l| l == "# backward-kill-line (not bound)"),
        "missing not-bound line: {p:?}"
    );
    // -P format
    let pv = sh.active_bind_lines_verbose();
    assert!(
        pv.iter()
            .any(|l| l == "beginning-of-line can be found on \"\\C-a\"."),
        "{pv:?}"
    );
    assert!(
        pv.iter()
            .any(|l| l == "backward-kill-line is not bound to any keys"),
        "{pv:?}"
    );
    // user override via pending_binds (the -c-mode path)
    sh.add_bind("\"\\C-a\"", "kill-line");
    let p2 = sh.active_bind_lines();
    assert!(
        p2.iter().any(|l| l == "\"\\C-a\": kill-line"),
        "override not applied: {p2:?}"
    );
    assert!(
        !p2.iter().any(|l| l == "\"\\C-a\": beginning-of-line"),
        "default not overridden: {p2:?}"
    );
    // unbind a default
    let mut sh2 = Shell::new();
    sh2.add_unbind("\\C-e");
    let p3 = sh2.active_bind_lines();
    assert!(
        !p3.iter().any(|l| l.contains("\\C-e")),
        "C-e still shown after unbind: {p3:?}"
    );
}

#[test]
fn bind_p_groups_multiple_keyseqs_for_one_function() {
    let sh = Shell::new();
    // accept-line is bound to both C-j and C-m by default.
    let p = sh.active_bind_lines();
    assert!(
        p.iter().any(|l| l == "\"\\C-j\": accept-line"),
        "missing C-j: {p:?}"
    );
    assert!(
        p.iter().any(|l| l == "\"\\C-m\": accept-line"),
        "missing C-m: {p:?}"
    );
    // -P joins both keyseqs on one line (sorted): C-j before C-m.
    let pv = sh.active_bind_lines_verbose();
    assert!(
        pv.iter()
            .any(|l| l == "accept-line can be found on \"\\C-j\", \"\\C-m\"."),
        "multi-keyseq -P join wrong: {pv:?}"
    );
}

#[cfg(test)]
fn test_fn_body() -> Box<crate::command::Command> {
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        "f(){ echo hi; }",
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .unwrap()
    .unwrap();
    match seq.first {
        crate::command::Command::FunctionDef { body, .. } => body,
        other => panic!("expected FunctionDef, got {other:?}"),
    }
}

#[test]
fn import_accepts_clean_function() {
    let body = parse_imported_function("g", "() { echo hi; }");
    assert!(body.is_some(), "clean function body should parse");
    match *body.unwrap() {
        crate::command::Command::BraceGroup(_) => {}
        other => panic!("expected brace-group body, got {other:?}"),
    }
}

#[test]
fn import_rejects_trailing_command() {
    // Shellshock: a trailing command after the `}` must NOT be accepted.
    assert!(parse_imported_function("x", "() { :; }; touch /tmp/PWN").is_none());
}

#[test]
fn import_rejects_bare_command() {
    assert!(parse_imported_function("x", "echo not_a_function").is_none());
}

#[test]
fn import_rejects_parse_error() {
    assert!(parse_imported_function("x", "() { if; }").is_none());
}

#[test]
fn import_accepts_simple_and_rejects_invalid_name() {
    assert!(parse_imported_function("x", "() { :; }").is_some());
    assert!(parse_imported_function("bad name", "() { :; }").is_none());
    // a value smuggling its OWN name (so reconstruction isn't a lone FunctionDef) → rejected.
    assert!(parse_imported_function("x", "y () { :; }").is_none());
}

#[test]
fn exported_function_env_pairs() {
    let mut sh = Shell::new();
    sh.define_function("ef".to_string(), test_fn_body(), 0);
    sh.mark_function_exported("ef");
    let env = sh.exported_function_env();
    let (_, v) = env
        .iter()
        .find(|(k, _)| k == "BASH_FUNC_ef%%")
        .expect("BASH_FUNC_ef%% present");
    assert!(v.starts_with("() "), "value should be () {{...}}: {v:?}");
    assert!(v.contains("echo hi"), "{v:?}");
}

#[test]
fn mark_and_query_exported_function() {
    let mut sh = Shell::new();
    sh.define_function("f".to_string(), test_fn_body(), 0);
    assert!(!sh.is_function_exported("f"));
    sh.mark_function_exported("f");
    assert!(sh.is_function_exported("f"));
    assert_eq!(sh.exported_function_names(), vec!["f".to_string()]);
}

#[test]
fn remove_function_unexports() {
    let mut sh = Shell::new();
    sh.define_function("f".to_string(), test_fn_body(), 0);
    sh.mark_function_exported("f");
    assert!(sh.remove_function("f"));
    assert!(!sh.is_function_exported("f"), "unset -f must un-export");
}

#[test]
fn is_set_true_for_set_var_even_when_empty() {
    let mut sh = Shell::new();
    sh.set("X", String::new()); // Shell::set(&mut self, name: &str, value: String)
    assert!(sh.is_set("X"));
}

#[test]
fn array_var_names_lists_arrays_not_scalars() {
    let mut sh = Shell::new();
    sh.set("scal", "x".to_string());
    let mut elements = std::collections::BTreeMap::new();
    elements.insert(0usize, "a".to_string());
    elements.insert(1usize, "b".to_string());
    sh.replace_indexed("arr", elements).unwrap(); // existing pub Shell method (indexed array)
    let names = sh.array_var_names();
    assert!(names.contains(&"arr".to_string()));
    assert!(!names.contains(&"scal".to_string()));
}

#[test]
fn is_set_false_for_unset() {
    let sh = Shell::new();
    assert!(!sh.is_set("DEFINITELY_UNSET_VAR_XYZ"));
}

#[test]
fn is_set_positional_params() {
    let mut sh = Shell::new();
    sh.positional_args = vec!["a".into(), "b".into()];
    assert!(sh.is_set("1"));
    assert!(sh.is_set("2"));
    assert!(!sh.is_set("3"));
}

#[test]
fn is_set_special_zero_is_true() {
    let sh = Shell::new();
    assert!(sh.is_set("0"));
}

#[test]
fn set_pipestatus_writes_indexed_array() {
    let mut sh = Shell::new();
    sh.set_pipestatus(&[0, 1, 0]);
    let arr = sh.get_indexed("PIPESTATUS").expect("PIPESTATUS array");
    assert_eq!(arr.get(&0).map(String::as_str), Some("0"));
    assert_eq!(arr.get(&1).map(String::as_str), Some("1"));
    assert_eq!(arr.get(&2).map(String::as_str), Some("0"));
    assert_eq!(arr.len(), 3);
}

fn make_func_frame(name: &str) -> Frame {
    Frame {
        funcname: name.to_string(),
        source: "environment".to_string(),
        call_line: 0,
        kind: FrameKind::Function,
    }
}

#[test]
fn sync_call_arrays_builds_reversed_stack() {
    let mut sh = Shell::new();
    sh.call_stack.push(make_func_frame("outer"));
    sh.call_stack.push(make_func_frame("inner"));
    sh.sync_call_arrays();
    let arr = sh.get_indexed("FUNCNAME").expect("FUNCNAME array");
    assert_eq!(arr.get(&0).map(String::as_str), Some("inner")); // [0] = current
    assert_eq!(arr.get(&1).map(String::as_str), Some("outer")); // [1] = caller
    assert_eq!(arr.len(), 2);
    assert_eq!(sh.lookup_var("FUNCNAME"), Some("inner".to_string()));
}

#[test]
fn sync_call_arrays_empty_stack_unsets() {
    let mut sh = Shell::new();
    sh.call_stack.push(make_func_frame("f"));
    sh.sync_call_arrays();
    assert!(sh.get_indexed("FUNCNAME").is_some());
    sh.call_stack.pop();
    sh.sync_call_arrays();
    assert!(
        sh.get_indexed("FUNCNAME").is_none(),
        "empty stack unsets FUNCNAME"
    );
    assert_eq!(sh.lookup_var("FUNCNAME"), None);
}

#[test]
fn sync_call_arrays_single_frame() {
    let mut sh = Shell::new();
    sh.call_stack.push(make_func_frame("solo"));
    sh.sync_call_arrays();
    assert_eq!(sh.lookup_var("FUNCNAME"), Some("solo".to_string()));
    assert_eq!(sh.get_indexed("FUNCNAME").expect("array").len(), 1);
}

#[test]
fn shell_clone_shares_functions_and_cow_isolates_defines() {
    let mut a = Shell::new();
    // Use the same minimal body shape as the builtins tests.
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    a.define_function("f".to_string(), body.clone(), 0);
    assert_eq!(Rc::strong_count(&a.functions), 1);
    let b = a.clone();
    // After clone both shells share the same Rc — O(1) clone, NOT a deep copy.
    assert_eq!(Rc::strong_count(&a.functions), 2);
    // COW: defining a new function in `a` must NOT affect `b`.
    a.define_function("g".to_string(), body, 0);
    assert!(a.functions.contains_key("g"));
    assert!(!b.functions.contains_key("g")); // isolation preserved
    // After make_mut the two Rcs are now independent.
    assert_eq!(Rc::strong_count(&a.functions), 1);
}

#[test]
fn shell_clone_shares_command_hash_history_completion_specs() {
    let mut a = Shell::new();
    let b = a.clone();
    // All three Rc fields are shared after clone — O(1), not deep copies.
    assert_eq!(Rc::strong_count(&a.command_hash), 2);
    assert_eq!(Rc::strong_count(&a.history), 2);
    assert_eq!(Rc::strong_count(&a.completion_specs), 2);

    // COW: a mutation on `a` must not affect `b`.
    Rc::make_mut(&mut a.command_hash)
        .insert("myls".to_string(), (std::path::PathBuf::from("/bin/ls"), 0));
    assert!(a.command_hash.contains_key("myls"), "a should have myls");
    assert!(
        !b.command_hash.contains_key("myls"),
        "b must not see a's mutation"
    );
    // After make_mut the two command_hash Rcs are now independent.
    assert_eq!(Rc::strong_count(&a.command_hash), 1);
    assert_eq!(Rc::strong_count(&b.command_hash), 1);
}

#[test]
fn new_captures_inherited_env_as_exported() {
    let shell = Shell::new();
    // PATH is reliably present in test environments.
    assert!(shell.get("PATH").is_some(), "PATH should be inherited");
    let path_exported = shell.exported_env().any(|(k, _)| k == "PATH");
    assert!(path_exported);
}

#[test]
fn set_creates_unexported_var() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_SET", "value".to_string());
    assert_eq!(shell.get("HUCK_TEST_SET"), Some("value"));
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_SET");
    assert!(!in_exported);
}

#[test]
fn set_preserves_existing_exported_flag() {
    let mut shell = Shell::new();
    shell.export_set("HUCK_TEST_KEEP", "v1".to_string());
    shell.set("HUCK_TEST_KEEP", "v2".to_string());
    assert_eq!(shell.get("HUCK_TEST_KEEP"), Some("v2"));
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_KEEP");
    assert!(in_exported);
}

#[test]
fn export_marks_existing_exported() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_EX", "value".to_string());
    shell.export("HUCK_TEST_EX");
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EX");
    assert!(in_exported);
}

#[test]
fn exported_env_includes_inline_scalar_overlay() {
    // #28: the inline-prefix scalar overlay (a scalar exported to a child even
    // though the variable is array-typed) is chained into exported_env.
    let mut shell = Shell::new();
    shell
        .inline_scalar_export
        .push(("OVL".to_string(), "v".to_string()));
    assert!(
        shell
            .exported_env()
            .any(|(k, val)| k == "OVL" && val == "v"),
        "inline scalar overlay must appear in exported_env"
    );
}

#[test]
fn export_creates_empty_when_missing() {
    let mut shell = Shell::new();
    shell.export("HUCK_TEST_EMPTY");
    assert_eq!(shell.get("HUCK_TEST_EMPTY"), Some(""));
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EMPTY");
    assert!(in_exported);
}

#[test]
fn unset_removes_variable() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_REMOVE", "v".to_string());
    shell.unset("HUCK_TEST_REMOVE");
    assert_eq!(shell.get("HUCK_TEST_REMOVE"), None);
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_REMOVE");
    assert!(!in_exported);
}

#[test]
fn unset_var_enclosing_local_pops_and_reveals() {
    let mut s = Shell::new();
    s.set("x", "midval".into());
    let mut outer = std::collections::HashMap::new();
    outer.insert("x".to_string(), None); // outer `local x` shadowed an unset global
    let mut mid = std::collections::HashMap::new();
    mid.insert("x".to_string(), Some(Variable::scalar("orig".into())));
    s.local_scopes.push(outer);
    s.local_scopes.push(mid);
    s.local_scopes.push(std::collections::HashMap::new()); // top frame (inner): no local x
    s.unset_var("x");
    assert_eq!(s.get("x"), Some("orig")); // mid's snapshot revealed
    assert!(!s.local_scopes[1].contains_key("x")); // mid's snapshot popped
}

#[test]
fn unset_var_top_frame_local_plain_removes() {
    let mut s = Shell::new();
    s.set("x", "v".into());
    let mut top = std::collections::HashMap::new();
    top.insert("x".to_string(), Some(Variable::scalar("orig".into())));
    s.local_scopes.push(top);
    s.unset_var("x");
    assert_eq!(s.get("x"), None); // value removed
    assert!(s.local_scopes[0].contains_key("x")); // snapshot KEPT (restores on return)
}

#[test]
fn unset_var_no_frames_plain_removes() {
    let mut s = Shell::new();
    s.set("x", "v".into());
    s.unset_var("x");
    assert_eq!(s.get("x"), None);
}

#[test]
fn last_status_round_trip() {
    let mut shell = Shell::new();
    assert_eq!(shell.last_status(), 0);
    shell.set_last_status(42);
    assert_eq!(shell.last_status(), 42);
}

#[test]
fn exported_env_excludes_unexported() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_HIDDEN", "v".to_string());
    let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_HIDDEN");
    assert!(!in_exported);
}

#[test]
fn new_captures_shell_pgid_from_getpgrp() {
    let s = Shell::new();
    let expected = unsafe { libc::getpgrp() };
    assert_eq!(s.shell_pgid, expected);
    assert!(s.shell_pgid > 0, "pgrp should be positive");
}

#[test]
fn new_initializes_sigint_flag_to_false() {
    let s = Shell::new();
    assert!(!s.sigint_flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn new_initializes_timeout_flag_to_false() {
    let s = Shell::new();
    assert!(!s.timeout_flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn new_initializes_live_external_children_empty() {
    let s = Shell::new();
    assert!(s.live_external_children.lock().unwrap().is_empty());
}

#[test]
fn new_initializes_policy_to_unrestricted() {
    let s = Shell::new();
    assert_eq!(s.policy, crate::policy::Policy::Unrestricted);
    assert!(!s.policy.is_restricted());
}

#[test]
fn var_names_lists_all_variables() {
    let mut shell = Shell::new();
    shell.set("HUCK_TEST_VN", "value".to_string());
    let names: Vec<&str> = shell.var_names().collect();
    assert!(names.contains(&"HUCK_TEST_VN"));
}

#[test]
fn is_exported_unset_var_is_false() {
    let shell = Shell::new();
    assert!(!shell.is_exported("DEFINITELY_NOT_SET"));
}

#[test]
fn is_exported_after_set_is_false() {
    let mut shell = Shell::new();
    shell.set("FOO", "bar".to_string());
    assert!(!shell.is_exported("FOO"));
}

#[test]
fn is_exported_after_export_set_is_true() {
    let mut shell = Shell::new();
    shell.export_set("FOO", "bar".to_string());
    assert!(shell.is_exported("FOO"));
}

#[test]
fn shell_new_caches_pid_and_argv0() {
    let shell = Shell::new();
    assert!(shell.shell_pid > 0, "shell_pid should be positive");
    assert!(
        !shell.shell_argv0.is_empty(),
        "shell_argv0 should be non-empty"
    );
    assert_eq!(shell.last_bg_pid, None);
    assert!(shell.call_stack.is_empty());
}

#[test]
fn lookup_var_dollar_returns_cached_pid_as_string() {
    let mut shell = Shell::new();
    shell.shell_pid = 12345;
    assert_eq!(shell.lookup_var("$"), Some("12345".to_string()));
}

#[test]
fn lookup_var_bang_unset_returns_empty_string() {
    let shell = Shell::new();
    assert_eq!(shell.lookup_var("!"), Some(String::new()));
}

#[test]
fn lookup_var_bang_after_set_returns_pid_string() {
    let mut shell = Shell::new();
    shell.last_bg_pid = Some(54321);
    assert_eq!(shell.lookup_var("!"), Some("54321".to_string()));
}

#[test]
fn lookup_var_underscore_returns_last_arg() {
    let mut shell = Shell::new();
    // At startup `$_` mirrors the invocation path (shell_argv0).
    assert_eq!(shell.lookup_var("_"), Some(shell.shell_argv0.clone()));
    // After a command updates it, `$_` reflects the last argument.
    shell.last_arg = "world".to_string();
    assert_eq!(shell.lookup_var("_"), Some("world".to_string()));
    // `_` is always considered set (backs `${_-x}` / `[[ -v _ ]]`).
    assert!(shell.is_set("_"));
}

#[test]
fn lookup_var_zero_top_level_returns_shell_argv0() {
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
}

#[test]
fn lookup_var_zero_in_function_keeps_shell_argv0() {
    // bash: `$0` is NOT rebound on function entry — it stays the script /
    // shell invocation name, even nested. (Other shells differ; bash does not.)
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    shell.call_stack.push(make_func_frame("myfunc"));
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
}

#[test]
fn lookup_var_zero_nested_keeps_shell_argv0() {
    let mut shell = Shell::new();
    shell.shell_argv0 = "my-shell".to_string();
    shell.call_stack.push(make_func_frame("outer"));
    shell.call_stack.push(make_func_frame("inner"));
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    shell.call_stack.pop();
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    shell.call_stack.pop();
    assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
}

#[test]
fn should_hangup_skips_marked_and_done_jobs() {
    use crate::jobs::{JobState, JobTable};
    let mut t = JobTable::new();
    let id = t.add(0, vec![1234], "sleep 30".to_string());

    // Running + not marked → hangup
    let job = t.iter().find(|j| j.id == id).unwrap();
    assert!(super::should_hangup(job));

    // Running + marked → skip
    t.mark_for_nohup(id);
    let job = t.iter().find(|j| j.id == id).unwrap();
    assert!(!super::should_hangup(job));

    // Done + not marked → skip
    t.jobs_mut()[0].marked_for_nohup = false;
    t.jobs_mut()[0].state = JobState::Done(0);
    let job = t.iter().find(|j| j.id == id).unwrap();
    assert!(!super::should_hangup(job));

    // Stopped + not marked → hangup (Stopped is "live" for SIGHUP purposes)
    t.jobs_mut()[0].marked_for_nohup = false;
    t.jobs_mut()[0].state = JobState::Stopped(::libc::SIGTSTP);
    let job = t.iter().find(|j| j.id == id).unwrap();
    assert!(super::should_hangup(job));
}
