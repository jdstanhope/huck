use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("hash", &args_owned, &mut buf, &mut std::io::stderr(), shell);
    (outcome, String::from_utf8(buf).unwrap())
}

#[test]
fn hash_empty_lists_empty() {
    let mut shell = Shell::new();
    let (oc, out) = run(&[], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "hash: hash table empty\n");
}

#[test]
fn hash_p_adds_direct() {
    let mut shell = Shell::new();
    let (oc, _out) = run(&["-p", "/custom", "mycmd"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let entry = shell.command_hash.get("mycmd");
    assert!(entry.is_some());
    let (path, hits) = entry.unwrap();
    assert_eq!(path, &std::path::PathBuf::from("/custom"));
    assert_eq!(*hits, 0);
}

#[test]
fn hash_r_clears() {
    let mut shell = Shell::new();
    run(&["-p", "/custom", "mycmd"], &mut shell);
    let (oc, _) = run(&["-r"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.command_hash.is_empty());
}

#[test]
fn hash_d_removes() {
    let mut shell = Shell::new();
    run(&["-p", "/custom", "mycmd"], &mut shell);
    let (oc, _) = run(&["-d", "mycmd"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.command_hash.is_empty());
}

#[test]
fn hash_d_missing_on_fresh_shell_is_silent_success() {
    // #509: bash's `phash_remove` returns success when the hash table has
    // never been created, so deleting an unhashed name in a fresh shell is
    // rc 0 with no diagnostic. This test asserted rc 1 — it pinned the
    // divergence the issue was opened for.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-d", "mycmd"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
}

#[test]
fn hash_d_missing_errors_once_table_exists() {
    let mut shell = Shell::new();
    run(&["-p", "/custom", "other"], &mut shell);
    let (oc, _) = run(&["-d", "mycmd"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn hash_l_re_input_form() {
    let mut shell = Shell::new();
    run(&["-p", "/foo", "a"], &mut shell);
    let (oc, out) = run(&["-l"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "builtin hash -p /foo a\n");
}

#[test]
fn hash_t_single_name() {
    let mut shell = Shell::new();
    run(&["-p", "/foo", "a"], &mut shell);
    let (oc, out) = run(&["-t", "a"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "/foo\n");
}

#[test]
fn hash_t_multi_name_tabs() {
    let mut shell = Shell::new();
    run(&["-p", "/foo", "a"], &mut shell);
    run(&["-p", "/bar", "b"], &mut shell);
    let (oc, out) = run(&["-t", "a", "b"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // Order matches the input args, not HashMap order.
    assert_eq!(out, "a\t/foo\nb\t/bar\n");
}

#[test]
fn hash_t_missing_errors_status_1() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-t", "a"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn hash_path_like_name_skipped_silently() {
    // #509: bash's `absolute_program()` (a name CONTAINING a slash) is
    // skipped without a word — there is nothing to hash. This asserted the
    // rc 1 of huck's invented "must not contain `/'" diagnostic.
    let mut shell = Shell::new();
    let (oc, _) = run(&["a/b"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.command_hash.is_empty());
}

#[test]
fn hash_invalid_option_status_2() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-X"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}

#[test]
fn hash_p_no_arg_status_2() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-p"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}
