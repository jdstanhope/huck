//! Integration test: arith errors in a script file print to stderr but
//! don't halt subsequent statements.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn run_script_file(script: &str, suffix: &str) -> (String, String, i32) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // pid too: `suffix` keeps callers within this process distinct, but without
    // pid a stale file from a concurrent run (or another UID on a shared /tmp)
    // makes the write below panic for a reason unrelated to the test (#297).
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("huck-v215-arith-{pid}-{stamp}-{suffix}.sh"));
    std::fs::write(&path, script).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .output()
        .expect("spawn");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn arith_division_by_zero_does_not_halt_script_file() {
    let script = "y=$((1/0))\necho POST\n";
    let (stdout, stderr, _rc) = run_script_file(script, "div0");
    assert!(
        stdout.contains("POST"),
        "POST not printed; script halted. stdout={stdout:?}"
    );
    assert!(
        stderr.contains("arithmetic") || stderr.contains("division"),
        "arith error not on stderr. stderr={stderr:?}"
    );
}

#[test]
fn arith_error_in_assignment_word_sets_status_one() {
    // #351: a failing `$(( ))` in an ASSIGNMENT-form word discards the command
    // and leaves `$?` = 1 (bash), where huck left 0. The command form already
    // gave 1; both must now report 1.
    let script = "v=$((1/0))\necho \"assign_rc=$?\"\necho $((2/0))\necho \"cmd_rc=$?\"\n";
    let (stdout, _stderr, _rc) = run_script_file(script, "assign_status");
    assert!(stdout.contains("assign_rc=1"), "stdout={stdout:?}");
    assert!(stdout.contains("cmd_rc=1"), "stdout={stdout:?}");
}

#[test]
fn arith_invalid_lhs_does_not_halt_script_file() {
    let script = "y=$((1 + 2 = 3))\necho POST\n";
    let (stdout, _stderr, _rc) = run_script_file(script, "lhs");
    assert!(
        stdout.contains("POST"),
        "POST not printed; script halted. stdout={stdout:?}"
    );
}
