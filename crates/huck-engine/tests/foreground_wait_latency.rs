//! Coarse wall-clock guard for #120: with inherited stdout (Engine::run, not
//! capture), running many subshells / external commands must NOT pay the old
//! ~100ms-per-child poll-tick latency. Pre-fix each 50-child batch took ~5s
//! (50 x 100ms); post-fix it is well under 0.5s. The 3s ceiling is generous
//! enough to be robust on a loaded 1-core box while still failing loudly if
//! this exact latency regresses.
//!
//! The subshell scenario forks in-process (huck runs subshells by forking
//! WITHOUT exec — memory-safe only in a single-threaded process); a
//! concurrent sibling `#[test]` executing at the same instant trips the
//! exec_guard panic (issue #184). So both checks run sequentially as ONE
//! `#[test]`, the sole test in this binary, so no sibling execution overlaps
//! the fork.

use std::time::{Duration, Instant};

use huck_engine::Engine;

const CEILING: Duration = Duration::from_secs(3);

#[test]
fn foreground_wait_latency_scenarios() {
    fifty_subshells_are_prompt();
    fifty_external_commands_are_prompt();
}

fn fifty_subshells_are_prompt() {
    let mut e = Engine::new();
    let start = Instant::now();
    // 50 empty subshells with inherited stdio (no capture pipe).
    let code = e.run("for i in $(seq 50); do ( : ); done");
    let elapsed = start.elapsed();
    assert_eq!(code, 0, "script exit code");
    assert!(
        elapsed < CEILING,
        "50 subshells took {elapsed:?} (>= {CEILING:?}); the #120 100ms-per-child latency has regressed"
    );
}

/// An absolute path to a real `true(1)` binary. Probed rather than hardcoded:
/// merged-usr Linux has both `/bin/true` and `/usr/bin/true`, macOS ships only
/// `/usr/bin/true`, and a hardcoded `/bin/true` made the script exit 127 there
/// (#297). Absolute on purpose — a bare `true` would hit huck's builtin and
/// stop exercising the external-command wait path this test is guarding.
fn true_bin() -> &'static str {
    ["/bin/true", "/usr/bin/true"]
        .into_iter()
        .find(|p| std::path::Path::new(p).is_file())
        .expect("no true(1) binary found in /bin or /usr/bin")
}

fn fifty_external_commands_are_prompt() {
    let mut e = Engine::new();
    let start = Instant::now();
    // 50 external commands with inherited stdio (still no shell-side capture pipe).
    let code = e.run(&format!("for i in $(seq 50); do {}; done", true_bin()));
    let elapsed = start.elapsed();
    assert_eq!(code, 0, "script exit code");
    assert!(
        elapsed < CEILING,
        "50 external commands took {elapsed:?} (>= {CEILING:?}); the #120 latency has regressed"
    );
}
