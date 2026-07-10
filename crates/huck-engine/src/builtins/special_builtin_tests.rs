use super::*;

#[test]
fn is_special_builtin_recognises_posix_specials() {
    for name in [
        "break", "continue", "exit", "export", "return", "unset", "times",
    ] {
        assert!(is_special_builtin(name), "expected {name} to be special");
    }
}

#[test]
fn is_special_builtin_rejects_regular_builtins() {
    for name in [
        "cd", "pwd", "echo", "jobs", "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    ] {
        assert!(!is_special_builtin(name), "expected {name} to be regular");
    }
}

#[test]
fn is_special_builtin_rejects_unknowns() {
    assert!(!is_special_builtin("not_a_builtin"));
    assert!(!is_special_builtin(""));
}

#[test]
fn trap_err_pseudo_signal_registers() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo err".to_string(), "ERR".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Err));
}

#[test]
fn trap_debug_pseudo_signal_registers() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo dbg".to_string(), "DEBUG".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Debug));
}

#[test]
fn trap_return_pseudo_signal_registers() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "trap",
        &["echo ret".to_string(), "RETURN".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Return));
}

#[test]
fn trap_p_lists_pseudo_signals_in_order() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // Register four pseudo-signals (intentionally not in EXIT/ERR/DEBUG/RETURN order).
    for (action, sig) in [
        ("a-return", "RETURN"),
        ("a-debug", "DEBUG"),
        ("a-exit", "EXIT"),
        ("a-err", "ERR"),
    ] {
        let _ = run_builtin(
            "trap",
            &[action.to_string(), sig.to_string()],
            &mut buf,
            &mut std::io::stderr(),
            &mut shell,
        );
    }
    buf.clear();
    let outcome = run_builtin(
        "trap",
        &["-p".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    // The four pseudo-signals should appear in EXIT, ERR, DEBUG, RETURN order.
    let pseudo_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| {
            l.contains("EXIT") || l.contains("ERR") || l.contains("DEBUG") || l.contains("RETURN")
        })
        .collect();
    assert_eq!(
        pseudo_lines.len(),
        4,
        "expected 4 pseudo-signal lines, got: {out}"
    );
    assert!(
        pseudo_lines[0].contains("EXIT"),
        "first line should be EXIT: {}",
        pseudo_lines[0]
    );
    assert!(
        pseudo_lines[1].contains("ERR"),
        "second line should be ERR: {}",
        pseudo_lines[1]
    );
    assert!(
        pseudo_lines[2].contains("DEBUG"),
        "third line should be DEBUG: {}",
        pseudo_lines[2]
    );
    assert!(
        pseudo_lines[3].contains("RETURN"),
        "fourth line should be RETURN: {}",
        pseudo_lines[3]
    );
}
