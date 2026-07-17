use crate::shell_state::Shell;

fn run(shell: &mut Shell, line: &str) {
    crate::shell::process_line(line, shell, false);
}

#[test]
fn invalid_coproc_name_rejected_at_runtime() {
    // `coproc @ { :; }` PARSES (bash grammar `coproc WORD compound`), then at
    // runtime the invalid identifier is rejected: no coproc is created, no
    // `@`/`@_PID` variables are set, and `$?` is 1 — matching bash.
    let mut s = Shell::new();
    run(&mut s, "coproc @ { :; }");
    assert_eq!(s.last_status(), 1, "invalid coproc name → exit status 1");
    assert!(s.coprocs.is_empty(), "no coproc should be created");
    assert!(
        s.get("@_PID").is_none(),
        "no NAME_PID for a rejected coproc"
    );
}

// NOTE: `valid_coproc_name_still_starts` moved to
// `tests/forking_execution_serial.rs` as `valid_coproc_name_starts_and_publishes`
// (in-process fork; unsafe to run concurrently with other tests — issue #184).
