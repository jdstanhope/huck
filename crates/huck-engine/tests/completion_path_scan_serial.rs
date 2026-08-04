//! Serial isolation for command completion's `$PATH` directory scan (#458).
//!
//! Completing a command word walks every `$PATH` entry with `read_dir`, so the
//! thread holds an open `DIR` for the length of the scan. A shell manipulates
//! descriptors by NUMBER, and under the parallel `--lib` harness a sibling
//! test's numbered redirect (`dup2(fd, 3)`) lands on whichever small descriptor
//! our `DIR` happens to hold — after which `closedir` fails:
//!
//! ```text
//! unexpected error during closedir: Os { code: 9, ... "Bad file descriptor" }
//! ```
//!
//! Isolating the OFFENDERS is not tractable — any test using a numbered
//! redirect dup2s onto a fixed small fd — so the victims move out instead. A
//! mutex does not help: the offender is libtest-scheduled sibling execution,
//! which takes none of our locks. As in `capture_tempfile_serial.rs`, the
//! checks are plain fns driven by ONE `#[test]`, so nothing in this binary runs
//! concurrently either. (Precedent: #90 / #184 / #297.)

use huck_engine::Engine;

/// `if whi` / a bare word / inside `$(` all put the cursor in command position,
/// where `while` is a candidate.
fn command_position_yields_commands() {
    let mut e = Engine::new();
    for line in ["if whi", "whi", "echo $(whi", "echo \"$(whi"] {
        let c = e.complete(line, line.len()).candidates;
        assert!(
            c.iter().any(|x| x.display == "while"),
            "{line:?}: {:?}",
            c.iter().map(|x| &x.display).collect::<Vec<_>>()
        );
    }
}

/// Completing is a read-only query: it must leave `$?` alone. `ec` sits in
/// command position, so this one scans `$PATH` too.
fn complete_does_not_modify_last_status() {
    let mut e = Engine::new();
    let _ = e.run("false");
    assert_eq!(e.last_status(), 1);
    let _ = e.complete("ec", 2);
    assert_eq!(e.last_status(), 1, "complete() must not alter $?");
}

#[test]
fn completion_path_scan_checks_run_serially() {
    command_position_yields_commands();
    complete_does_not_modify_last_status();
}
