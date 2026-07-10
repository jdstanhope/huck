#[test]
fn enable_toggle_updates_set() {
    let mut sh = crate::shell_state::Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    super::builtin_enable(&["-n".into(), "test".into()], &mut out, &mut err, &mut sh);
    assert!(sh.disabled_builtins.contains("test"));
    super::builtin_enable(&["test".into()], &mut out, &mut err, &mut sh);
    assert!(!sh.disabled_builtins.contains("test"));
}
