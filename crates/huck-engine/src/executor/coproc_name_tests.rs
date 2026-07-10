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

#[test]
fn valid_coproc_name_still_starts() {
    // Regression guard: a valid name is unaffected — the coprocess starts and
    // its record is published (status 0).
    let mut s = Shell::new();
    run(&mut s, "coproc MYCO { :; }");
    assert_eq!(s.last_status(), 0, "valid coproc name → exit status 0");
    assert_eq!(s.coprocs.len(), 1, "one coproc should be live");
    assert_eq!(s.coprocs[0].name, "MYCO");
    // Reap the child to avoid leaking it into other tests.
    run(&mut s, "wait 2>/dev/null");
}
