use super::*;
use crate::shell_state::Shell;
use crate::test_support::CWD_LOCK;

/// What `cd <path>` will end up storing in PWD: `builtin_cd` reads
/// the post-chdir cwd via `env::current_dir()`, which canonicalizes
/// symlinks. On Linux `/tmp` is a real directory so this is `/tmp`;
/// on macOS `/tmp` is a symlink to `/private/tmp` and the kernel
/// returns the resolved path. Computing it at test time keeps the
/// assertions portable.
#[test]
fn cd_sets_pwd_to_target_directory() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["/tmp".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Restore for any other tests.
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    // Logical PWD (v162): `cd /tmp` stores the logical path, not the
    // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
    assert_eq!(shell.get("PWD"), Some("/tmp"));
    assert!(shell.exported_env().any(|(k, _)| k == "PWD"));
    assert!(out.is_empty());
}

#[test]
fn cd_sets_oldpwd_to_previous_pwd() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut shell = Shell::new();
    shell.export_set("PWD", "/var".to_string());
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["/tmp".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.get("OLDPWD"), Some("/var"));
    assert!(shell.exported_env().any(|(k, _)| k == "OLDPWD"));
}

#[test]
fn cd_with_pwd_initially_unset_does_not_set_oldpwd() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut shell = Shell::new();
    shell.unset("PWD");
    shell.unset("OLDPWD");
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["/tmp".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.get("OLDPWD"), None);
    // Logical PWD (v162): `cd /tmp` stores the logical path, not the
    // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
    assert_eq!(shell.get("PWD"), Some("/tmp"));
}

#[test]
fn cd_dash_uses_oldpwd_as_target() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut shell = Shell::new();
    shell.export_set("OLDPWD", "/tmp".to_string());
    shell.export_set("PWD", "/var".to_string());
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["-".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    // Logical PWD (v162): `cd /tmp` stores the logical path, not the
    // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
    assert_eq!(shell.get("PWD"), Some("/tmp"));
    assert_eq!(shell.get("OLDPWD"), Some("/var"));
}

#[test]
fn cd_dash_prints_new_pwd_on_stdout() {
    let _g = CWD_LOCK.lock().unwrap();
    let mut shell = Shell::new();
    shell.export_set("OLDPWD", "/tmp".to_string());
    shell.export_set("PWD", "/var".to_string());
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["-".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    // `cd -` echoes the logical PWD (v162): the OLDPWD value as-typed.
    assert_eq!(String::from_utf8(out).unwrap(), "/tmp\n");
}

#[test]
fn cd_dash_errors_when_oldpwd_unset() {
    let mut shell = Shell::new();
    shell.unset("OLDPWD");
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["-".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(out.is_empty());
}

#[test]
fn cd_dash_errors_when_oldpwd_empty() {
    let mut shell = Shell::new();
    shell.export_set("OLDPWD", String::new());
    let mut out: Vec<u8> = Vec::new();
    let prev = std::env::current_dir().unwrap();
    let outcome = builtin_cd(
        &["-".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    let _ = std::env::set_current_dir(&prev);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(out.is_empty());
}
