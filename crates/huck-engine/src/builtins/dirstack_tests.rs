use super::*;
use crate::shell_state::Shell;
use std::path::PathBuf;

// ── parse_signed_index ────────────────────────────────────

#[test]
fn parse_signed_index_plus() {
    assert_eq!(parse_signed_index("+0", 10).unwrap(), 0);
    assert_eq!(parse_signed_index("+2", 10).unwrap(), 2);
    assert_eq!(parse_signed_index("+5", 10).unwrap(), 5);
}

#[test]
fn parse_signed_index_minus() {
    // length 10: -0 = last (9); -1 = 8; -9 = 0.
    assert_eq!(parse_signed_index("-0", 10).unwrap(), 9);
    assert_eq!(parse_signed_index("-1", 10).unwrap(), 8);
    assert_eq!(parse_signed_index("-9", 10).unwrap(), 0);
}

#[test]
fn parse_signed_index_out_of_range() {
    assert!(parse_signed_index("+10", 10).is_err());
    assert!(parse_signed_index("-10", 10).is_err());
}

#[test]
fn parse_signed_index_invalid() {
    assert!(parse_signed_index("+abc", 10).is_err());
}

#[test]
fn parse_signed_index_no_sign() {
    assert!(parse_signed_index("2", 10).is_err());
}

// ── dir_display ───────────────────────────────────────────

#[test]
fn dir_display_no_home_unchanged() {
    let mut shell = Shell::new();
    shell.set("HOME", String::new());
    // Also clear process env to be safe.
    let saved = std::env::var("HOME").ok();
    unsafe {
        std::env::remove_var("HOME");
    }
    let out = dir_display(&PathBuf::from("/etc"), &shell, true);
    unsafe {
        if let Some(h) = saved {
            std::env::set_var("HOME", h);
        }
    }
    assert_eq!(out, "/etc");
}

#[test]
fn dir_display_home_match_collapses() {
    let mut shell = Shell::new();
    shell.set("HOME", "/h/me".to_string());
    assert_eq!(dir_display(&PathBuf::from("/h/me"), &shell, true), "~",);
}

#[test]
fn dir_display_home_subdir_collapses() {
    let mut shell = Shell::new();
    shell.set("HOME", "/h/me".to_string());
    assert_eq!(dir_display(&PathBuf::from("/h/me/x"), &shell, true), "~/x",);
}

#[test]
fn dir_display_no_collapse_flag() {
    let mut shell = Shell::new();
    shell.set("HOME", "/h/me".to_string());
    assert_eq!(
        dir_display(&PathBuf::from("/h/me/x"), &shell, false),
        "/h/me/x",
    );
}

#[test]
fn dir_display_unrelated_path_passes_through() {
    let mut shell = Shell::new();
    shell.set("HOME", "/h/me".to_string());
    assert_eq!(
        dir_display(&PathBuf::from("/etc/foo"), &shell, true),
        "/etc/foo",
    );
}

// ── is_signed_index_arg ───────────────────────────────────

#[test]
fn is_signed_index_arg_recognizes_numeric_forms() {
    assert!(is_signed_index_arg("+0"));
    assert!(is_signed_index_arg("+12"));
    assert!(is_signed_index_arg("-0"));
    assert!(is_signed_index_arg("-5"));
}

#[test]
fn is_signed_index_arg_rejects_alpha_after_sign() {
    // Regression: previously the `+` branch had no digit guard,
    // so `+foo` (a literal directory name) was misclassified
    // as an index specifier. Match the symmetric `-foo` rule.
    assert!(!is_signed_index_arg("+foo"));
    assert!(!is_signed_index_arg("+bar"));
    assert!(!is_signed_index_arg("-foo"));
    assert!(!is_signed_index_arg("-bar"));
}

#[test]
fn is_signed_index_arg_rejects_bare_signs_and_paths() {
    assert!(!is_signed_index_arg("+"));
    assert!(!is_signed_index_arg("-"));
    assert!(!is_signed_index_arg("/tmp"));
    assert!(!is_signed_index_arg("relative"));
    assert!(!is_signed_index_arg(""));
}
