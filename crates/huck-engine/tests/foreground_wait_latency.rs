//! Coarse wall-clock guard for #120: with inherited stdout (Engine::run, not
//! capture), running many subshells / external commands must NOT pay the old
//! ~100ms-per-child poll-tick latency. Pre-fix each 50-child batch took ~5s
//! (50 x 100ms); post-fix it is well under 0.5s. The 3s ceiling is generous
//! enough to be robust on a loaded 1-core box while still failing loudly if
//! this exact latency regresses. Its own integration binary so it never shares
//! a process with other forking tests.

use std::time::{Duration, Instant};

use huck_engine::Engine;

const CEILING: Duration = Duration::from_secs(3);

#[test]
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

#[test]
fn fifty_external_commands_are_prompt() {
    let mut e = Engine::new();
    let start = Instant::now();
    // 50 external commands with inherited stdio (still no shell-side capture pipe).
    let code = e.run("for i in $(seq 50); do /bin/true; done");
    let elapsed = start.elapsed();
    assert_eq!(code, 0, "script exit code");
    assert!(
        elapsed < CEILING,
        "50 external commands took {elapsed:?} (>= {CEILING:?}); the #120 latency has regressed"
    );
}
