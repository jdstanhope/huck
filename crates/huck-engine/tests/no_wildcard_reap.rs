//! #183: huck-engine is a LIBRARY, so running a script must NOT reap children
//! the EMBEDDER spawned.
//!
//! `jobs::reap_completed` used to loop on `waitpid(-1, WNOHANG | ...)`, draining
//! EVERY child of the process. That is correct for a standalone shell — it owns
//! its process — but inside an embedder it silently reaped the embedder's
//! children and swallowed their exit status. v306 (#175) made this reachable on
//! every non-interactive command boundary (`executor.rs` `execute_sequence_body`
//! and `builtins.rs` `run_sourced_contents_in_sinks`), i.e. on every `Engine`
//! call.
//!
//! The same theft broke the multithreaded cargo test binary, where it surfaced
//! two ways: a one-shot `waitpid(pid)` getting ECHILD (the
//! `fork_and_run_in_subshell_echo_stage_writes_to_pipe` assertion), and an
//! infinite hang in `stream_loop`'s poll loop, which has no `-1` case and spins
//! forever once its child is gone.
//!
//! In its own binary (own process) so a concurrent test cannot perturb the
//! embedder-child bookkeeping this asserts on.

use huck_engine::Engine;
use std::process::Command;
use std::time::Duration;

/// Spawn an embedder-owned child, let it exit so it is sitting reapable, run a
/// non-interactive script through the engine, then require that WE can still
/// reap our own child and observe its exit status.
#[test]
fn engine_does_not_reap_the_embedders_child() {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 7")
        .spawn()
        .expect("spawn embedder child");

    // Let it exit, so it is a reapable zombie while the engine runs — the exact
    // window in which a waitpid(-1) would swallow it.
    std::thread::sleep(Duration::from_millis(200));

    // A multi-unit script: the #175 between-command maintenance runs at each
    // boundary, which is where the wildcard reap fired.
    let mut e = Engine::new();
    let out = e.capture("echo one; echo two");
    assert_eq!(out.stdout, "one\ntwo\n");

    // The engine must have left our child alone. If it reaped it, `wait` fails
    // with ECHILD and the status is gone for good.
    let status = child
        .wait()
        .expect("engine reaped the embedder's child (ECHILD) — #183 regression");
    assert_eq!(
        status.code(),
        Some(7),
        "embedder's child exit status was stolen — #183 regression"
    );
}

/// The engine must still reap its OWN background children — the bounded reap
/// must not regress #175's pruning into a zombie leak.
#[test]
fn engine_still_reaps_its_own_background_children() {
    let mut e = Engine::new();
    // Background job completes, then a later command boundary prunes it.
    let out = e.capture("sleep 0 & sleep 0.3; jobs | wc -l");
    assert_eq!(
        out.stdout.trim(),
        "0",
        "a completed background job must still be reaped and pruned (#175)"
    );
}
